//! Pinned tabs never unload; big ones may sleep (Mac `Fork/PinsAwake.swift`;
//! Copper#134, #181).
//!
//! A pin is a page kept open all day — Gmail, a calendar — and coming back
//! to one to find it loading again is the browser forgetting the one thing
//! it was asked to keep. So a pin has a live page from the moment it is
//! pinned, or from launch, until it is unpinned:
//!
//! - The idle sweep passes over pins ([`crate::sleep::Keep::Pinned`]);
//!   Ctrl+W on one steps away from it and leaves its page loaded
//!   ([`copper_core::CloseOutcome::SteppedAway`]); Close Other Tabs leaves
//!   pins alone.
//! - A pin with no page — every pin but the one on screen after a launch,
//!   one pinned while it slept, one another device sent — is loaded in the
//!   background, one at a time, once the launch has settled
//!   ([`Shell::look_at_pins`]).
//! - A pin whose process went away out of sight is loaded again at once,
//!   three times in ten minutes at most ([`crate::crash`]).
//!
//! Settings › Tabs › Let big pins sleep (on unless turned off; `copper pins
//! sleep off`): a pin over 300 MB that nobody has looked at for two hours
//! sleeps like any tab ([`may_doze`]) and waits, asleep, until you go to
//! it; a pin asleep at quit stays asleep at launch ([`asleep_at_launch`]).

use crate::crash::Limit;
use crate::main_loop::MainLoop;
use crate::{Platform, Shell};
use copper_core::TabId;
use copper_core::session::SessionShape;
use copper_engine::PageView;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::{Duration, Instant};

const MB: u64 = 1 << 20;
/// How long the background loader waits on one pin before the next.
const LOAD_AT_MOST: Duration = Duration::from_secs(8);

/// A pin whose page holds at least `least` bytes and that nobody has looked
/// at for `after` seconds may sleep, when big pins may (`on`).
pub fn may_doze(on: bool, hidden: f64, bytes: u64, after: f64, least: u64) -> bool {
    on && hidden >= after && bytes >= least
}

/// At launch, whether a pin stays asleep until it is looked at: it was
/// asleep when the session was written, or it is big and was left long
/// enough — and there is a picture to show where the platform draws them
/// (`covered`). Off, never.
pub fn asleep_at_launch(
    on: bool,
    covered: bool,
    dozed: bool,
    hidden: f64,
    bytes: u64,
    after: f64,
    least: u64,
) -> bool {
    on && covered && (dozed || may_doze(true, hidden, bytes, after, least))
}

/// A pin with somewhere to be and no page there. A blank pin has nothing
/// to load.
pub fn needs_page(pinned: bool, blank: bool, asleep: bool, built: bool) -> bool {
    pinned && !blank && (asleep || !built)
}

/// The shell's pins state.
pub(crate) struct Pins {
    /// Each pin's background reloads after its process went away.
    pub(crate) reloads: RefCell<Limit<TabId>>,
    /// Pins asleep because they were big and left: not loaded again until
    /// looked at.
    pub(crate) dozed: RefCell<HashSet<TabId>>,
    started: Cell<bool>,
    /// A look is due (pins changed; they come in bursts).
    looking: Cell<bool>,
    /// The loader is running.
    loading: Cell<bool>,
}

impl Default for Pins {
    fn default() -> Self {
        Pins {
            reloads: RefCell::new(Limit::pin_reloads()),
            dozed: RefCell::default(),
            started: Cell::new(false),
            looking: Cell::new(false),
            loading: Cell::new(false),
        }
    }
}

/// Settings' answers for big pins: on, after (s), least (bytes).
fn rules<P: Platform>(shell: &Shell<P>) -> (bool, f64, u64) {
    let s = shell.settings();
    (
        s.big_pins_sleep(),
        s.pins_sleep_after_secs(),
        s.pins_sleep_mb() * MB,
    )
}

impl<P: Platform> Shell<P> {
    /// Whether pin `id` may sleep now: big pins may, it has been left long
    /// enough and its page (last weighed) is big enough.
    pub(crate) fn pin_may_doze(&self, id: &TabId, hidden: f64) -> bool {
        let (on, after, least) = rules(self);
        let bytes = self.weight_of(id).map_or(0, |w| w.bytes);
        may_doze(on, hidden, bytes, after, least)
    }

    /// The launch has settled: pins asleep at quit stay asleep, every other
    /// pin without a page is loaded in the background.
    pub(crate) fn start_pins(&self) {
        if self.pins_state().started.replace(true) {
            return;
        }
        let (on, after, least) = rules(self);
        let now = copper_core::UnixTime::now().0;
        let draws = self.draws_covers();
        let asleep: Vec<TabId> = {
            let b = self.browser();
            b.pins()
                .into_iter()
                .filter(|t| t.sleeping)
                .filter(|t| {
                    let dozed = t.extra.get("dozed").and_then(Value::as_bool) == Some(true);
                    let bytes = t.extra.get("mb").and_then(Value::as_u64).unwrap_or(0) * MB;
                    // Where nothing draws a picture, every wake is white:
                    // nothing to wait for.
                    let covered = !draws || self.cover_kept(&t.url, b.profile_of(&t.id).as_deref());
                    asleep_at_launch(on, covered, dozed, now - t.seen.0, bytes, after, least)
                })
                .map(|t| t.id.clone())
                .collect()
        };
        if !asleep.is_empty() {
            tracing::debug!(count = asleep.len(), "pins left asleep at launch");
        }
        self.pins_state().dozed.borrow_mut().extend(asleep);
        self.look_at_pins();
    }

    /// Pins or rows changed (a flush): one look half a second later, for
    /// the burst. Nothing before the launch has settled.
    pub(crate) fn pins_changed(&self) {
        let pins = self.pins_state();
        if !pins.started.get() || pins.looking.replace(true) {
            return;
        }
        let weak = self.downgrade();
        P::Loop::spawn_local(async move {
            P::Loop::sleep(Duration::from_millis(500)).await;
            if let Some(s) = weak.upgrade() {
                s.pins_state().looking.set(false);
                s.look_at_pins();
            }
        });
    }

    /// Load every pin that has no page, one at a time, each until it has
    /// loaded (or [`LOAD_AT_MOST`]).
    pub fn look_at_pins(&self) {
        if self.pins_state().loading.replace(true) {
            return;
        }
        let weak = self.downgrade();
        P::Loop::spawn_local(async move {
            while let Some(id) = weak.upgrade().and_then(|s| s.next_pin()) {
                let start = Instant::now();
                if let Some(s) = weak.upgrade() {
                    tracing::debug!(tab = %id.short(), "pin loaded in the background");
                    s.ensure_page(&id);
                    s.flush();
                }
                loop {
                    P::Loop::sleep(Duration::from_millis(200)).await;
                    let Some(s) = weak.upgrade() else { return };
                    let busy = s
                        .slot(&id)
                        .is_some_and(|x| x.loading.get() || x.page.is_loading());
                    if !busy || start.elapsed() > LOAD_AT_MOST {
                        break;
                    }
                }
            }
            if let Some(s) = weak.upgrade() {
                s.pins_state().loading.set(false);
            }
        });
    }

    /// The next pin with no page that isn't left asleep on purpose.
    fn next_pin(&self) -> Option<TabId> {
        let dozed = self.pins_state().dozed.borrow().clone();
        let b = self.browser();
        b.pins()
            .into_iter()
            .filter(|t| !dozed.contains(&t.id) && !t.private)
            .find(|t| {
                needs_page(
                    true,
                    crate::clear::is_blank(&t.url),
                    t.sleeping,
                    self.slot(&t.id).is_some(),
                )
            })
            .map(|t| t.id.clone())
    }

    /// A tab woke (it got a page): a pin that dozed is a pin again.
    pub(crate) fn pin_woke(&self, id: &TabId) {
        self.pins_state().dozed.borrow_mut().remove(id);
    }

    /// The session about to be written: each pin's weight (now, or the last
    /// known) and whether it dozed, for the next launch
    /// ([`asleep_at_launch`]; the Mac's `mb` and `dozed`).
    pub(crate) fn write_pins(&self, shape: &mut SessionShape) {
        let b = self.browser();
        // The same pins `to_session` writes, in its order.
        let ids: Vec<TabId> = b
            .pins()
            .into_iter()
            .filter(|t| t.persistable())
            .map(|t| t.id.clone())
            .collect();
        let dozed = self.pins_state().dozed.borrow();
        for (entry, id) in shape.pins.iter_mut().flatten().zip(&ids) {
            if let Some(w) = self.weight_of(id) {
                entry.extra.insert("mb".into(), json!(w.bytes / MB));
            }
            if dozed.contains(id) {
                entry.extra.insert("dozed".into(), json!(true));
            } else {
                entry.extra.remove("dozed");
            }
        }
    }

    /// `copper pins` (the loopback `copper/pins`): whether big pins may
    /// sleep (`on` turns it on or off first) and every pin's state.
    pub fn pins_status(&self, on: Option<bool>) -> Value {
        if let Some(on) = on {
            self.update_settings(|s| s.set(copper_core::settings::keys::PINS_SLEEP, on));
        }
        let (on, after, least) = rules(self);
        let now = copper_core::UnixTime::now().0;
        let active = self.active_id();
        let dozed = self.pins_state().dozed.borrow().clone();
        let reloads = self.pins_state().reloads.borrow().clone();
        let pins: Vec<Value> = self
            .browser()
            .pins()
            .into_iter()
            .map(|t| {
                let slot = self.slot(&t.id);
                json!({
                    "id": t.id.short(),
                    "pin": t.pin,
                    "title": t.title,
                    "url": t.url,
                    "loaded": slot.is_some(),
                    "asleep": t.sleeping,
                    "dozed": dozed.contains(&t.id),
                    "crashed": slot.as_ref().is_some_and(|s| s.crashed.get()),
                    "mb": self.weight_of(&t.id).map(|w| w.bytes / MB),
                    "front": active.as_ref() == Some(&t.id),
                    "reloads": reloads.count(&t.id, now),
                })
            })
            .collect();
        json!({
            "bigPinsSleep": on,
            "afterSeconds": after as u64,
            "overMB": least / MB,
            "pins": pins,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    // Mac `PinRuleTests.needsPage`.
    #[test]
    fn a_pin_without_a_page_is_loaded_and_nothing_else() {
        assert!(needs_page(true, false, true, false));
        assert!(needs_page(true, false, false, false));
        assert!(!needs_page(true, false, false, true));
        assert!(!needs_page(true, true, false, false));
        assert!(!needs_page(false, false, true, false));
    }

    // Mac `BigPinsTests`.
    #[test]
    fn off_a_pin_never_dozes() {
        assert!(!may_doze(false, 99.0 * 3600.0, 4 * GB, 7200.0, 300 * MB));
    }

    #[test]
    fn on_over_300_mb_and_two_hours_unlooked_at_and_only_then() {
        assert!(may_doze(true, 7200.0, 300 * MB, 7200.0, 300 * MB));
        assert!(!may_doze(true, 7199.0, 900 * MB, 7200.0, 300 * MB));
        assert!(!may_doze(true, 9.0 * 3600.0, 299 * MB, 7200.0, 300 * MB));
    }

    // Mac `BigPinsLaunchTests`.
    fn launch(on: bool, covered: bool, dozed: bool, hidden: f64, bytes: u64) -> bool {
        asleep_at_launch(on, covered, dozed, hidden, bytes, 7200.0, 300 * MB)
    }

    #[test]
    fn dozed_or_big_and_left_with_a_picture_stays_asleep_recent_or_small_loads() {
        assert!(launch(true, true, true, 0.0, 0));
        assert!(launch(true, true, false, 3.0 * 3600.0, 900 * MB));
        assert!(!launch(true, true, false, 60.0, 900 * MB));
        assert!(!launch(true, true, false, 9.0 * 3600.0, 100 * MB));
        assert!(!launch(true, false, true, 0.0, 0));
    }

    #[test]
    fn setting_off_every_pin_loads_whatever_the_session_said() {
        assert!(!launch(false, true, true, 0.0, 0));
        assert!(!launch(false, true, false, 9.0 * 3600.0, 900 * MB));
    }

    #[test]
    fn on_unless_turned_off() {
        let mut s = copper_core::Settings::default();
        assert!(s.big_pins_sleep());
        s.set(copper_core::settings::keys::PINS_SLEEP, false);
        assert!(!s.big_pins_sleep());
        assert_eq!(s.pins_sleep_after_secs(), 7200.0);
        assert_eq!(s.pins_sleep_mb(), 300);
    }
}
