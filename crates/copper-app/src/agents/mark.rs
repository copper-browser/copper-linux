//! Who is driving which tab, drawn small (Mac: `HandsUI.swift`): a square
//! in the agent's colour on the tab row it is touching (or touched in the
//! last half minute), and a capsule over the page while it drives the tab
//! you are looking at — its name, what it is doing, and Stop.
//!
//! The model is `copper_mcp::drive::DriveEvent`s; the one thing that goes
//! back is Stop ([`super::Agents::stop_hand`]): a Jev run stops before its
//! next action, any other agent has its next calls refused for 30 s.

use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_mcp::Who;
use copper_mcp::drive::{DriveEvent, Status};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// A tab keeps its square this long after the agent's last call.
const BADGE_FOR: Duration = Duration::from_secs(30);
/// The capsule stays this long after the last call ends.
const PILL_FOR: Duration = Duration::from_secs(4);

/// The Mac's agent colours (`Drive.swatches`), as RGB.
const SWATCHES: [(u8, u8, u8); 9] = [
    (230, 71, 77),
    (237, 176, 33),
    (48, 163, 107),
    (18, 166, 148),
    (5, 158, 204),
    (61, 99, 222),
    (110, 87, 207),
    (143, 79, 199),
    (214, 64, 158),
];

/// Copper's own agent keeps Copper's colour.
const PANE: (u8, u8, u8) = (217, 119, 87);

/// An agent session's colour: FNV-1a of its seed into the swatches.
pub fn swatch(who: &Who) -> (u8, u8, u8) {
    if who.key == "pane" {
        return PANE;
    }
    let mut hash: u32 = 2_166_136_261;
    for b in who.seed.bytes() {
        hash = (hash ^ b as u32).wrapping_mul(16_777_619);
    }
    SWATCHES[(hash % SWATCHES.len() as u32) as usize]
}

/// One agent's hand on one tab.
#[derive(Clone, Debug)]
struct Hand {
    who: Who,
    /// What it is doing (the call's words, or the Jev goal / phase).
    action: String,
    /// Calls in flight.
    busy: u32,
    last: Instant,
    jev: bool,
    stopped: bool,
}

impl Hand {
    fn badge(&self) -> bool {
        self.busy > 0 || self.last.elapsed() < BADGE_FOR
    }

    fn pill(&self) -> bool {
        self.busy > 0 || self.last.elapsed() < PILL_FOR
    }
}

/// A capsule over one tab's page.
struct Pill {
    root: gtk::Box,
    square: gtk::Image,
    name: gtk::Label,
    action: gtk::Label,
    stop: gtk::Button,
    /// Whose Stop the button is.
    key: Rc<RefCell<String>>,
    frame: glib::WeakRef<gtk::Overlay>,
}

pub struct Marks {
    shell: WeakShell,
    hands: RefCell<HashMap<TabId, Vec<Hand>>>,
    /// Ticket → the tab the call began on.
    tickets: RefCell<HashMap<u64, TabId>>,
    /// The live Jev run's tab.
    run: RefCell<Option<TabId>>,
    pills: RefCell<HashMap<TabId, Pill>>,
    squares: RefCell<HashMap<(u8, u8, u8), gtk::gdk::Texture>>,
}

thread_local! {
    static CSS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

const STYLE: &str = r#"
.copper-agent-pill {
  min-height: 26px; padding: 0 3px 0 6px; border-radius: 13px;
  background-color: alpha(@window_bg_color, .96);
  color: @window_fg_color;
  box-shadow: 0 0 0 1px alpha(@window_fg_color, .10), 0 2px 8px alpha(black, .14);
}
.copper-agent-pill .agent-name { font-size: 12px; font-weight: 600; }
.copper-agent-pill .agent-action { font-size: 12px; opacity: .62; }
.copper-agent-pill button.agent-stop {
  min-width: 20px; min-height: 20px; padding: 0; border-radius: 10px;
  background: transparent; box-shadow: none; opacity: .7;
}
.copper-agent-pill button.agent-stop:hover { opacity: 1; background-color: alpha(@window_fg_color, .08); }
.copper-agent-pill.stopped .agent-action { opacity: .9; }
.copper-row .badge.agent-mark { opacity: 1; }

/* Settings › Agents */
.agent-status-dot { min-width: 8px; min-height: 8px; border-radius: 4px; background-color: alpha(@window_fg_color, .22); }
.agent-status-dot.on { background-color: @success_color; }
.agent-status-dot.trouble { background-color: @warning_color; }
.agent-token { font-family: monospace; font-size: 12px; opacity: .62; margin-right: 4px; }
"#;

fn install_css() {
    if CSS.with(|c| c.replace(true)) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

/// The column's badge for a tab, when an agent is on it: call after the
/// row's own badge is set (`window/column.rs`, `fill`).
pub fn paint_badge(id: &TabId, badge: &gtk::Image) {
    let Some(marks) = super::agents().map(|a| a.marks.clone()) else {
        return;
    };
    match marks.top(id).filter(Hand::badge) {
        Some(hand) => {
            let tex = marks.square(swatch(&hand.who));
            badge.set_paintable(Some(&tex));
            badge.set_visible(true);
            badge.add_css_class("agent-mark");
            let tip = if hand.busy > 0 {
                format!("{} is working in this tab", hand.who.line())
            } else {
                format!("{} worked in this tab", hand.who.line())
            };
            badge.set_tooltip_text(Some(&tip));
        }
        None => badge.remove_css_class("agent-mark"),
    }
}

impl Marks {
    pub fn new(shell: &Shell) -> Rc<Marks> {
        install_css();
        Rc::new(Marks {
            shell: shell.downgrade(),
            hands: RefCell::default(),
            tickets: RefCell::default(),
            run: RefCell::default(),
            pills: RefCell::default(),
            squares: RefCell::default(),
        })
    }

    /// The most recent hand on a tab.
    fn top(&self, id: &TabId) -> Option<Hand> {
        self.hands
            .borrow()
            .get(id)?
            .iter()
            .max_by_key(|h| (h.busy > 0, h.last))
            .cloned()
    }

    /// Any agent busy or recent anywhere (the palette's Stop Agents).
    pub fn any_active(&self) -> bool {
        self.hands
            .borrow()
            .values()
            .flatten()
            .any(|h| h.busy > 0 || h.last.elapsed() < BADGE_FOR)
    }

    /// The tabs agents are on right now, with who and whether a call is in
    /// flight (QA and the palette).
    pub fn snapshot(&self) -> Vec<(TabId, String, bool)> {
        let mut out: Vec<(TabId, String, bool)> = self
            .hands
            .borrow()
            .iter()
            .flat_map(|(id, hs)| {
                hs.iter()
                    .filter(|h| h.badge())
                    .map(move |h| (id.clone(), h.who.agent.clone(), h.busy > 0))
            })
            .collect();
        out.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        out
    }

    /// Whether a capsule is showing over this tab's page.
    pub fn pill_visible(&self, id: &TabId) -> bool {
        self.pills
            .borrow()
            .get(id)
            .is_some_and(|p| p.root.is_visible())
    }

    /// Stop every agent that is busy or was lately.
    pub fn stop_all(&self) {
        let keys: Vec<String> = self
            .hands
            .borrow()
            .values()
            .flatten()
            .filter(|h| h.pill())
            .map(|h| h.who.key.clone())
            .collect();
        for k in keys {
            self.stop(&k);
        }
    }

    fn stop(&self, key: &str) {
        if let Some(a) = super::agents() {
            a.stop_hand(key);
        }
        let tabs: Vec<TabId> = {
            let mut hands = self.hands.borrow_mut();
            let mut tabs = Vec::new();
            for (id, hs) in hands.iter_mut() {
                for h in hs.iter_mut().filter(|h| h.who.key == key) {
                    h.stopped = true;
                    h.action = "Stopped — its next calls are turned away".into();
                    h.last = Instant::now();
                    tabs.push(id.clone());
                }
            }
            tabs
        };
        for id in tabs {
            self.redraw(&id);
            self.later(&id, PILL_FOR);
        }
    }

    /// Forget everything (the server stopped).
    pub fn clear(&self) {
        let ids: Vec<TabId> = self.hands.borrow_mut().drain().map(|(id, _)| id).collect();
        self.tickets.borrow_mut().clear();
        self.run.borrow_mut().take();
        for id in ids {
            self.redraw(&id);
        }
    }

    /// `Host::announce`: the capsule already says what the agent does when
    /// Settings › Agents › Say what the agent does is on.
    pub fn announce(&self, _text: &str) {}

    /// One drive event.
    pub fn event(&self, e: &DriveEvent) {
        match e {
            DriveEvent::CallBegan {
                ticket,
                who,
                title,
                tab,
                ..
            } => {
                let Some(tab) = tab.clone() else { return };
                self.tickets.borrow_mut().insert(*ticket, tab.clone());
                self.touch(&tab, who, Some(title), 1, false);
            }
            DriveEvent::CallEnded {
                ticket, who, tab, ..
            } => {
                let began = self.tickets.borrow_mut().remove(ticket);
                if let Some(b) = &began {
                    self.finish(b, &who.key);
                }
                if let Some(t) = tab
                    && began.as_ref() != Some(t)
                {
                    self.touch(t, who, None, 0, false);
                }
            }
            DriveEvent::CallRefused { who, .. } => {
                let tabs: Vec<TabId> = self
                    .hands
                    .borrow()
                    .iter()
                    .filter(|(_, hs)| hs.iter().any(|h| h.who.key == who.key))
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in tabs {
                    self.redraw(&id);
                }
            }
            DriveEvent::RunBegan { who, goal, tab } => {
                *self.run.borrow_mut() = Some(tab.clone());
                self.touch(tab, who, Some(goal), 1, true);
            }
            DriveEvent::Phase { title, .. } => {
                let tab = self.run.borrow().clone();
                if let Some(tab) = tab {
                    let mut changed = false;
                    if let Some(h) = self
                        .hands
                        .borrow_mut()
                        .get_mut(&tab)
                        .and_then(|hs| hs.iter_mut().find(|h| h.jev && h.busy > 0))
                    {
                        h.action = title.clone();
                        h.last = Instant::now();
                        changed = true;
                    }
                    if changed {
                        self.redraw_pill(&tab);
                    }
                }
            }
            DriveEvent::RunFinished { status, note } => {
                let Some(tab) = self.run.borrow_mut().take() else {
                    return;
                };
                if let Some(h) = self
                    .hands
                    .borrow_mut()
                    .get_mut(&tab)
                    .and_then(|hs| hs.iter_mut().find(|h| h.jev && h.busy > 0))
                {
                    h.busy = 0;
                    h.last = Instant::now();
                    h.action = match status {
                        Status::Done => "Done".into(),
                        Status::Stopped => "Stopped".into(),
                        _ if !note.trim().is_empty() => note.clone(),
                        _ => "Finished".into(),
                    };
                }
                self.redraw(&tab);
                self.later(&tab, PILL_FOR);
                self.later(&tab, BADGE_FOR);
            }
            DriveEvent::Cycle
            | DriveEvent::PhaseClosed { .. }
            | DriveEvent::Outcome(_)
            | DriveEvent::Page { .. } => {}
        }
    }

    fn touch(&self, tab: &TabId, who: &Who, action: Option<&String>, busy: u32, jev: bool) {
        {
            let mut hands = self.hands.borrow_mut();
            let list = hands.entry(tab.clone()).or_default();
            match list.iter_mut().find(|h| h.who.key == who.key) {
                Some(h) => {
                    h.busy += busy;
                    h.last = Instant::now();
                    h.jev |= jev;
                    h.stopped = false;
                    if let Some(a) = action {
                        h.action = a.clone();
                    }
                }
                None => list.push(Hand {
                    who: who.clone(),
                    action: action.cloned().unwrap_or_default(),
                    busy,
                    last: Instant::now(),
                    jev,
                    stopped: false,
                }),
            }
        }
        self.redraw(tab);
        if busy == 0 {
            self.later(tab, PILL_FOR);
            self.later(tab, BADGE_FOR);
        }
    }

    fn finish(&self, tab: &TabId, key: &str) {
        if let Some(h) = self
            .hands
            .borrow_mut()
            .get_mut(tab)
            .and_then(|hs| hs.iter_mut().find(|h| h.who.key == key))
        {
            h.busy = h.busy.saturating_sub(1);
            h.last = Instant::now();
        }
        self.redraw(tab);
        self.later(tab, PILL_FOR);
        self.later(tab, BADGE_FOR);
    }

    /// Look again once `after` has passed (a lapse needs no event).
    fn later(&self, tab: &TabId, after: Duration) {
        let tab = tab.clone();
        glib::timeout_add_local_once(after + Duration::from_millis(50), move || {
            if let Some(a) = super::agents() {
                a.marks.lapse(&tab);
            }
        });
    }

    fn lapse(&self, tab: &TabId) {
        let gone = {
            let mut hands = self.hands.borrow_mut();
            if let Some(list) = hands.get_mut(tab) {
                list.retain(Hand::badge);
            }
            hands.get(tab).is_some_and(Vec::is_empty)
        };
        if gone {
            self.hands.borrow_mut().remove(tab);
            // Capsules whose page went away.
            self.pills
                .borrow_mut()
                .retain(|_, p| p.frame.upgrade().is_some());
        }
        self.redraw(tab);
    }

    fn redraw(&self, tab: &TabId) {
        if let Some(shell) = self.shell.upgrade() {
            shell
                .ui()
                .refresh(&shell, &crate::window::Refresh::tab(tab));
        }
        self.redraw_pill(tab);
    }

    fn redraw_pill(&self, tab: &TabId) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let hand = self.top(tab).filter(Hand::pill);
        let Some(hand) = hand else {
            if let Some(p) = self.pills.borrow().get(tab) {
                p.root.set_visible(false);
            }
            return;
        };
        let Some(slot) = shell.slot(tab) else { return };
        let stale = self
            .pills
            .borrow()
            .get(tab)
            .is_some_and(|p| p.frame.upgrade().as_ref() != Some(&slot.frame));
        if stale {
            self.pills.borrow_mut().remove(tab);
        }
        if !self.pills.borrow().contains_key(tab) {
            let pill = self.build_pill(&slot.frame);
            self.pills.borrow_mut().insert(tab.clone(), pill);
        }
        let pills = self.pills.borrow();
        let Some(p) = pills.get(tab) else { return };
        let colour = swatch(&hand.who);
        p.square.set_paintable(Some(&self.square(colour)));
        p.name.set_text(&hand.who.line());
        let says = super::agents().is_none_or(|a| a.announces());
        let action = if hand.stopped || says {
            hand.action.trim().to_string()
        } else {
            String::new()
        };
        p.action.set_text(&action);
        p.action.set_visible(!action.is_empty());
        p.root.set_tooltip_text(Some(&if action.is_empty() {
            hand.who.line()
        } else {
            format!("{} — {action}", hand.who.line())
        }));
        if hand.stopped {
            p.root.add_css_class("stopped");
        } else {
            p.root.remove_css_class("stopped");
        }
        p.stop.set_visible(!hand.stopped);
        p.stop.set_tooltip_text(Some(if hand.jev {
            "Stop the run"
        } else {
            "Take the browser back — its next calls are refused"
        }));
        *p.key.borrow_mut() = hand.who.key.clone();
        p.root
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{} is driving this tab",
                hand.who.line()
            ))]);
        p.root.set_visible(true);
    }

    fn build_pill(&self, frame: &gtk::Overlay) -> Pill {
        let root: gtk::Box = glib::Object::builder()
            .property("orientation", gtk::Orientation::Horizontal)
            .property("spacing", 7)
            .property("accessible-role", gtk::AccessibleRole::Status)
            .build();
        root.add_css_class("copper-agent-pill");
        root.set_halign(gtk::Align::Center);
        root.set_valign(gtk::Align::Start);
        root.set_margin_top(10);
        let square = gtk::Image::new();
        square.set_pixel_size(14);
        square.set_valign(gtk::Align::Center);
        root.append(&square);
        let name = gtk::Label::new(None);
        name.add_css_class("agent-name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(28);
        root.append(&name);
        let action = gtk::Label::new(None);
        action.add_css_class("agent-action");
        action.set_ellipsize(gtk::pango::EllipsizeMode::End);
        action.set_max_width_chars(40);
        root.append(&action);
        let stop = gtk::Button::from_icon_name("media-playback-stop-symbolic");
        stop.add_css_class("agent-stop");
        stop.set_valign(gtk::Align::Center);
        stop.set_focusable(false);
        stop.update_property(&[gtk::accessible::Property::Label("Stop the agent")]);
        let key: Rc<RefCell<String>> = Rc::default();
        let k = key.clone();
        stop.connect_clicked(move |_| {
            let key = k.borrow().clone();
            if let Some(a) = super::agents() {
                a.marks.stop(&key);
            }
        });
        root.append(&stop);
        root.set_visible(false);
        frame.add_overlay(&root);
        Pill {
            root,
            square,
            name,
            action,
            stop,
            key,
            frame: frame.downgrade(),
        }
    }

    /// A rounded square in `rgb`, 2× for sharp edges at 14 px.
    fn square(&self, rgb: (u8, u8, u8)) -> gtk::gdk::Texture {
        if let Some(t) = self.squares.borrow().get(&rgb) {
            return t.clone();
        }
        let tex = square_texture(rgb, 28, 8.0);
        self.squares.borrow_mut().insert(rgb, tex.clone());
        tex
    }
}

/// Premultiplied RGBA of a rounded square (`size` px, corner `radius`)
/// with a soft white rim, as a texture.
fn square_texture(rgb: (u8, u8, u8), size: usize, radius: f64) -> gtk::gdk::Texture {
    let bytes = square_pixels(rgb, size, radius);
    let gbytes = glib::Bytes::from_owned(bytes);
    gtk::gdk::MemoryTexture::new(
        size as i32,
        size as i32,
        gtk::gdk::MemoryFormat::R8g8b8a8Premultiplied,
        &gbytes,
        size * 4,
    )
    .upcast()
}

fn square_pixels(rgb: (u8, u8, u8), size: usize, radius: f64) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    let s = size as f64;
    for y in 0..size {
        for x in 0..size {
            // Signed distance to the rounded square's edge (negative inside).
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let qx = (px - s / 2.0).abs() - (s / 2.0 - radius);
            let qy = (py - s / 2.0).abs() - (s / 2.0 - radius);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - radius;
            let alpha = (0.5 - d).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            // A 1.5 px rim at 55 % white over the fill.
            let rim = (d + 2.0).clamp(0.0, 1.0) * 0.55;
            let mix = |c: u8| (c as f64 * (1.0 - rim) + 255.0 * rim) * alpha;
            let i = (y * size + x) * 4;
            out[i] = mix(rgb.0).round() as u8;
            out[i + 1] = mix(rgb.1).round() as u8;
            out[i + 2] = mix(rgb.2).round() as u8;
            out[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_stable_per_seed() {
        let a = Who {
            key: "k".into(),
            seed: "Claude Code·repo".into(),
            ..Who::default()
        };
        assert_eq!(swatch(&a), swatch(&a.clone()));
        assert!(SWATCHES.contains(&swatch(&a)));
        assert_eq!(swatch(&Who::pane()), PANE);
    }

    #[test]
    fn square_is_opaque_inside_and_clear_at_the_corner() {
        let px = square_pixels((10, 20, 30), 28, 8.0);
        let at = |x: usize, y: usize| px[(y * 28 + x) * 4 + 3];
        assert_eq!(at(14, 14), 255);
        assert_eq!(at(0, 0), 0);
        assert_eq!(
            &px[(14 * 28 + 14) * 4..(14 * 28 + 14) * 4 + 3],
            &[10, 20, 30]
        );
    }
}
