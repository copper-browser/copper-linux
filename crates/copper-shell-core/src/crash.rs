//! A page's web process went away: the record in `crashes.json` (Mac:
//! `Fork/Crashes.swift`), and what happens to the page (Mac
//! `webViewWebContentProcessDidTerminate`, [`Shell::recover`]).
//!
//! The app survived, so the entry is a `fault` (the Mac's word for a crash
//! it lived through), with `source: "web-process"` and the reason.
//!
//! In front of you the page is loaded again at once — a reload beats a
//! white page with a button on it. But a page that takes its process down
//! as it loads would be loaded for ever (sign-in pages refreshing every
//! second): the third death in half a minute stops it, and the page says
//! "This page keeps closing as it loads." with Try Again, which loads the
//! address afresh (Mac `Tab.recoverFromCrash`, Copper#127). A pin out of
//! sight is loaded again in the background, three times in ten minutes at
//! most (Mac `PinReloads`, Copper#134). Anything else — or a process Copper
//! ended itself — shows "This page stopped" with Reload
//! ([`crate::shell::pages::PageSlot::show_crash`]).

use crate::Platform;
use crate::shell::Shell;
use copper_core::TabId;
use copper_core::crashes::{CrashEntry, CrashIndex};
use copper_core::date::IsoDate;
use copper_engine::{CrashReason, PageView};
use std::collections::HashMap;
use std::hash::Hash;

/// How many entries `crashes.json` keeps.
pub const KEEP: usize = 100;

/// What the page says when it stopped being loaded again.
pub const LOOP: &str = "This page keeps closing as it loads.";

/// At most `limit` in any `window` seconds, per tab: a pin's background
/// reloads (Mac `PinReloads`) and the reloads after a crash in front of
/// you (Mac `Tab.recoverFromCrash`). A refusal is not counted.
#[derive(Debug, Clone)]
pub struct Limit<K> {
    pub limit: usize,
    pub window: f64,
    times: HashMap<K, Vec<f64>>,
}

impl<K: Hash + Eq + Clone> Limit<K> {
    pub fn new(limit: usize, window: f64) -> Self {
        Limit {
            limit,
            window,
            times: HashMap::new(),
        }
    }

    /// A pin's reloads after its process went away: three in ten minutes.
    pub fn pin_reloads() -> Self {
        Limit::new(3, 600.0)
    }

    /// Reloads after the page in front died: two in half a minute, so the
    /// third death stops it.
    pub fn recoveries() -> Self {
        Limit::new(2, 30.0)
    }

    /// Whether `key` may go again at `now` (seconds) — counted when it may.
    pub fn allow(&mut self, key: &K, now: f64) -> bool {
        let window = self.window;
        let recent = self.times.entry(key.clone()).or_default();
        recent.retain(|t| now - t < window);
        if recent.len() >= self.limit {
            return false;
        }
        recent.push(now);
        true
    }

    /// How many `key` has had inside the window.
    pub fn count(&self, key: &K, now: f64) -> usize {
        self.times
            .get(key)
            .map_or(0, |t| t.iter().filter(|t| now - **t < self.window).count())
    }

    pub fn forget(&mut self, key: &K) {
        self.times.remove(key);
    }
}

/// The entry for one web-process crash.
pub fn entry(url: &str, reason: CrashReason, now: IsoDate) -> CrashEntry {
    let host = crate::shell::host_of(url);
    CrashEntry {
        id: format!("web-{}-{}", now.unix(), std::process::id()),
        kind: "fault".into(),
        time: now,
        source: Some("web-process".into()),
        version: Some(copper_core::version::app().into()),
        os: Some(os_name()),
        reason: Some(
            match reason {
                CrashReason::Crashed => "crashed",
                CrashReason::ExceededMemoryLimit => "exceeded-memory-limit",
                CrashReason::TerminatedByApi => "terminated",
            }
            .into(),
        ),
        // The host, never the whole address: a crash log is shared.
        trail: (!host.is_empty()).then(|| vec![format!("page {host}")]),
        ..Default::default()
    }
}

/// Add `entry` to an index, newest last, keeping [`KEEP`].
pub fn append(index: &mut CrashIndex, entry: CrashEntry) {
    index.entries.push(entry);
    if index.entries.len() > KEEP {
        let drop = index.entries.len() - KEEP;
        index.entries.drain(..drop);
    }
}

/// Record a crash for `tab` (reads the file, appends, saves off the main
/// thread).
pub fn record<P: Platform>(shell: &Shell<P>, _tab: &TabId, url: &str, reason: CrashReason) {
    let mut index = match shell.store().load::<CrashIndex>() {
        Ok(l) => l.value,
        Err(e) => {
            tracing::warn!(error = %e, "crashes.json unreadable; starting a new one");
            CrashIndex::default()
        }
    };
    append(&mut index, entry(url, reason, IsoDate::now()));
    shell.saver().schedule(index);
    tracing::warn!(?reason, "web process crashed");
}

impl<P: Platform> Shell<P> {
    /// Tab `id`'s page lost its process: load it again where the Mac would
    /// (see the module). `false` when nothing was done and the crashed page
    /// should show; `true` when it is loading again or says it stopped.
    pub(crate) fn recover(
        &self,
        id: &TabId,
        slot: &crate::shell::pages::PageSlot<P>,
        reason: CrashReason,
        active: bool,
    ) -> bool {
        // Ended on purpose (Copper itself, a test): nothing to make good.
        if reason == CrashReason::TerminatedByApi {
            return false;
        }
        let Some((url, pinned)) = self
            .browser()
            .tab(id)
            .map(|t| (t.url.clone(), t.is_pinned()))
        else {
            return false;
        };
        if crate::clear::is_blank(&url) {
            return false;
        }
        let host = crate::shell::host_of(&url);
        let now = copper_core::UnixTime::now().0;
        if active {
            if !self.deaths().borrow_mut().allow(id, now) {
                self.deaths().borrow_mut().forget(id);
                tracing::warn!(target: "copper::crumb", %host, "webcontent-loop");
                slot.show_loop(&url);
                return true;
            }
        } else if pinned {
            let mut reloads = self.pins_state().reloads.borrow_mut();
            let allowed = reloads.allow(id, now);
            let n = reloads.count(id, now);
            drop(reloads);
            if !allowed {
                tracing::warn!(target: "copper::crumb", %host, n, "pin-reload-stop");
                return false;
            }
            tracing::info!(target: "copper::crumb", %host, n, "pin-reload");
        } else {
            return false;
        }
        slot.clear_status();
        slot.page.load(&url);
        true
    }
}

/// The system's name for the record: `/etc/os-release`'s `PRETTY_NAME`
/// where there is one, else the family (`Linux`, `Windows`).
fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "Windows".into()
            } else {
                "Linux".into()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mac `PinReloadsTests`.
    #[test]
    fn three_reloads_then_none_inside_ten_minutes() {
        let mut limit = Limit::pin_reloads();
        let answers: Vec<bool> = [0.0, 60.0, 120.0, 180.0, 599.0]
            .iter()
            .map(|t| limit.allow(&1, *t))
            .collect();
        assert_eq!(answers, [true, true, true, false, false]);
        // A refusal is not counted: the window still runs from the first.
        assert_eq!(limit.count(&1, 599.0), 3);
    }

    #[test]
    fn once_the_oldest_is_ten_minutes_old_there_is_room_for_one_more() {
        let mut limit = Limit::pin_reloads();
        let answers: Vec<bool> = [0.0, 60.0, 120.0, 540.0, 600.0, 630.0, 660.0]
            .iter()
            .map(|t| limit.allow(&1, *t))
            .collect();
        assert_eq!(answers, [true, true, true, false, true, false, true]);
    }

    #[test]
    fn each_tab_has_its_own_count_and_forgetting_one_clears_it() {
        let mut limit = Limit::pin_reloads();
        let first: Vec<bool> = (0..4).map(|_| limit.allow(&"gmail", 0.0)).collect();
        assert_eq!(first, [true, true, true, false]);
        assert!(limit.allow(&"calendar", 0.0));
        assert_eq!(limit.count(&"calendar", 0.0), 1);
        limit.forget(&"gmail");
        assert!(limit.allow(&"gmail", 0.0));
    }

    // Mac `FramedExtensionTests` (the crash loop): the third death in half
    // a minute stops; Try Again starts the count afresh.
    #[test]
    fn the_third_death_in_half_a_minute_stops() {
        let mut deaths = Limit::recoveries();
        assert!(deaths.allow(&1, 0.0));
        assert!(deaths.allow(&1, 1.0));
        assert!(!deaths.allow(&1, 2.0));
        deaths.forget(&1);
        assert!(deaths.allow(&1, 3.0));
        // Spread out, it never stops.
        let mut slow = Limit::recoveries();
        assert!((0..10).all(|i| slow.allow(&1, f64::from(i) * 16.0)));
    }

    #[test]
    fn entries_are_faults_without_the_full_address() {
        let e = entry(
            "https://mail.example.com/inbox?secret=1",
            CrashReason::Crashed,
            IsoDate(1_700_000_000),
        );
        assert_eq!(e.kind, "fault");
        assert!(!e.fatal());
        assert_eq!(e.reason.as_deref(), Some("crashed"));
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert!(json.contains("mail.example.com"));
    }

    #[test]
    fn index_is_capped_and_round_trips_with_the_mac_file() {
        let mac = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mac/crashes.json"
        ))
        .unwrap();
        let mut index: CrashIndex = serde_json::from_slice(&mac).unwrap();
        let before = index.entries.len();
        append(
            &mut index,
            entry(
                "https://a.b",
                CrashReason::ExceededMemoryLimit,
                IsoDate::now(),
            ),
        );
        assert_eq!(index.entries.len(), (before + 1).min(KEEP));
        for _ in 0..(KEEP + 5) {
            append(
                &mut index,
                entry("https://a.b", CrashReason::Crashed, IsoDate::now()),
            );
        }
        assert_eq!(index.entries.len(), KEEP);
        let again: CrashIndex =
            serde_json::from_str(&serde_json::to_string(&index).unwrap()).unwrap();
        assert_eq!(again.entries.len(), KEEP);
    }
}
