//! The Agent sidebar slot on screen (Mirrors the Mac's
//! `Fork/Addons/AddonSlots.swift`, `AgentSlot`, which takes `AgentPane`'s
//! place in `Split.swift`).
//!
//! A revealer right of the stage (after the side panels), full height. Its
//! content is made the first time it is shown and kept while it is hidden,
//! so an addon's chat survives Ctrl+E. It holds either the addon's web view
//! ([`super::view`]) or Copper's own placeholder, and a grip on its leading
//! edge that sets its width (`agent.paneWidth`, 340…620, default 400).

use super::policy::{self, DEFAULT_WIDTH};
use super::view::AddonView;
use super::{core_glue, shell};
use crate::shell::{GtkShell, Shell};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// The settings key the width is kept under (Mac `agent.paneWidth`).
pub const WIDTH_KEY: &str = "agent.paneWidth";

const CSS: &str = r#"
.copper-addon-pane { background-color: @copper_ground; color: @copper_ink; box-shadow: inset 1px 0 alpha(@copper_ink, .09); }
.copper-addon-pane .addon-grip { min-width: 6px; background: transparent; transition: background-color 120ms ease-out; }
.copper-addon-pane .addon-grip:hover, .copper-addon-pane .addon-grip.dragging { background-color: alpha(@accent_color, .35); }
.copper-addon-pane .panel-head { min-height: 46px; padding: 0 8px 0 16px; }
.copper-addon-pane .panel-title { font-size: 15px; font-weight: 600; }
.copper-addon-pane button.flat-icon {
  min-width: 26px; min-height: 26px; padding: 0; border-radius: 7px;
  background: transparent; box-shadow: none; color: alpha(@copper_ink, .62);
}
.copper-addon-pane button.flat-icon:hover { background-color: alpha(@copper_ink, .07); color: @copper_ink; }
.copper-addon-pane statuspage { background: transparent; }
.copper-addon-pane statuspage .icon { -gtk-icon-size: 64px; margin-bottom: 18px; opacity: .45; }
.copper-addon-pane statuspage .title { font-size: 20px; }
.copper-addon-pane statuspage .description { opacity: .7; }
.copper-addon-pane statuspage > scrolledwindow > viewport > box { margin: 24px 28px; }
.copper-addon-pane .addon-web { background-color: @copper_ground; }
"#;

/// The slot's pieces. The revealer exists from the start; the rest is made
/// on first show.
struct Pane {
    revealer: gtk::Revealer,
    built: RefCell<Option<Built>>,
    width: Cell<i32>,
}

struct Built {
    frame: gtk::Box,
    stack: gtk::Stack,
    view: RefCell<Option<Rc<AddonView>>>,
}

thread_local! {
    static PANE: RefCell<Option<Rc<Pane>>> = const { RefCell::new(None) };
}

fn pane() -> Option<Rc<Pane>> {
    PANE.with(|p| p.borrow().clone())
}

/// The slot's stylesheet, once.
pub fn install_css() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

/// Put the (empty, hidden) slot into the stage's row.
pub fn mount(shell: &Shell, stage: &gtk::Box) {
    install_css();
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
    revealer.set_transition_duration(160);
    revealer.set_reveal_child(false);
    revealer.set_hexpand(false);
    revealer.set_vexpand(true);
    revealer.add_css_class("copper-addon-slot");
    let width = policy::clamp_width(shell.settings().number(WIDTH_KEY));
    let weak = shell.downgrade();
    revealer.connect_reveal_child_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            revealed(&s, r.reveals_child());
        }
    });
    stage.append(&revealer);
    PANE.with(|p| {
        *p.borrow_mut() = Some(Rc::new(Pane {
            revealer,
            built: RefCell::default(),
            width: Cell::new(width),
        }))
    });
}

/// Whether the slot is on screen (or sliding in).
pub fn showing() -> bool {
    pane().is_some_and(|p| p.revealer.reveals_child())
}

/// Ctrl+E.
pub fn toggle(shell: &Shell) {
    if showing() {
        hide(shell);
    } else {
        show(shell);
    }
}

/// Show the slot and give it the keyboard.
pub fn show(shell: &Shell) {
    let Some(p) = pane() else { return };
    build(shell, &p);
    sync(shell);
    p.revealer.set_reveal_child(true);
    focus_inside(&p);
}

/// Put the slot away; the page gets the keyboard back if the slot had it.
pub fn hide(shell: &Shell) {
    let Some(p) = pane() else { return };
    let had_focus = p
        .revealer
        .root()
        .and_then(|r| r.focus())
        .is_some_and(|f| f.is_ancestor(&p.revealer));
    p.revealer.set_reveal_child(false);
    if had_focus && let Some(page) = shell.active_page() {
        page.widget().grab_focus();
    }
}

/// The pane's current width.
pub fn width() -> i32 {
    pane().map_or(DEFAULT_WIDTH, |p| p.width.get())
}

/// The slot's addon view, once made.
pub fn view() -> Option<Rc<AddonView>> {
    let p = pane()?;
    let b = p.built.borrow();
    b.as_ref()?.view.borrow().clone()
}

/// Which of the pane's faces is showing: `"addon"`, `"placeholder"`, or
/// `None` before the first show.
pub fn face() -> Option<String> {
    let p = pane()?;
    let b = p.built.borrow();
    b.as_ref()?
        .stack
        .visible_child_name()
        .map(|s| s.to_string())
}

/// The pane's root widget (for the self-test's screenshots).
pub fn widget() -> Option<gtk::Widget> {
    pane().map(|p| p.revealer.clone().upcast())
}

/// Bring the slot in line with the addon the core says it shows: nothing
/// (placeholder), the same one (settings may have changed), a new version
/// of it (reload), or another addon (a new view).
pub fn sync(shell: &Shell) {
    let Some(p) = pane() else { return };
    let built = p.built.borrow();
    let Some(b) = built.as_ref() else {
        // Not shown yet: it is made from the current state on first show.
        return;
    };
    let active = core_glue::active();
    let current = b.view.borrow().clone();
    match (active, current) {
        (None, Some(v)) => {
            tracing::info!(id = %v.addon_id(), "addon left the slot; showing the placeholder");
            v.retire();
            b.view.borrow_mut().take();
            b.stack.set_visible_child_name("placeholder");
            if let Some(w) = b.stack.child_by_name("addon") {
                b.stack.remove(&w);
            }
        }
        (None, None) => b.stack.set_visible_child_name("placeholder"),
        (Some(a), Some(v)) if v.addon_id() == a.id => {
            v.update(&a);
            b.stack.set_visible_child_name("addon");
        }
        (Some(a), current) => {
            if let Some(v) = current {
                v.retire();
                if let Some(w) = b.stack.child_by_name("addon") {
                    b.stack.remove(&w);
                }
            }
            let v = AddonView::new(shell, &a);
            v.widget().add_css_class("addon-web");
            b.stack.add_named(&v.widget(), Some("addon"));
            b.stack.set_visible_child_name("addon");
            *b.view.borrow_mut() = Some(v.clone());
            v.set_visible(p.revealer.reveals_child());
        }
    }
}

fn build(shell: &Shell, p: &Rc<Pane>) {
    if p.built.borrow().is_some() {
        return;
    }
    let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    frame.add_css_class("copper-addon-pane");
    frame.set_size_request(p.width.get(), -1);
    frame.set_vexpand(true);
    frame.update_property(&[gtk::accessible::Property::Label(super::SLOT_NAME)]);

    let grip = gtk::Box::new(gtk::Orientation::Vertical, 0);
    grip.add_css_class("addon-grip");
    grip.set_vexpand(true);
    grip.set_cursor_from_name(Some("col-resize"));
    grip.set_tooltip_text(Some("Drag to resize"));
    frame.append(&grip);
    drag_to_resize(shell, p, &grip);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_named(&placeholder(shell), Some("placeholder"));
    frame.append(&stack);
    p.revealer.set_child(Some(&frame));
    *p.built.borrow_mut() = Some(Built {
        frame,
        stack,
        view: RefCell::default(),
    });
}

/// Copper's own face for the slot, when no addon fills it.
fn placeholder(shell: &Shell) -> gtk::Widget {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    head.add_css_class("panel-head");
    let title = gtk::Label::new(Some(super::SLOT_NAME));
    title.add_css_class("panel-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    head.append(&title);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("flat-icon");
    close.set_valign(gtk::Align::Center);
    close.set_tooltip_text(Some("Close (Ctrl+E)"));
    close.update_property(&[gtk::accessible::Property::Label("Close")]);
    let weak = shell.downgrade();
    close.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            hide(&s);
        }
    });
    head.append(&close);
    root.append(&head);

    let status = adw::StatusPage::new();
    status.set_icon_name(Some("sidebar-show-right-symbolic"));
    status.set_title("No agent here yet.");
    status.set_description(Some("Addons from your Copper Cloud can add one here."));
    status.add_css_class("compact");
    status.set_vexpand(true);
    let open = gtk::Button::with_label("Open Addons Settings");
    open.add_css_class("pill");
    open.add_css_class("suggested-action");
    open.set_halign(gtk::Align::Center);
    let weak = shell.downgrade();
    open.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            super::open_settings(&s);
        }
    });
    status.set_child(Some(&open));
    root.append(&status);
    root.upcast()
}

/// Dragging the leading edge: left is wider. The width is kept when the
/// drag ends.
fn drag_to_resize(shell: &Shell, p: &Rc<Pane>, grip: &gtk::Box) {
    let drag = gtk::GestureDrag::new();
    let start = Rc::new(Cell::new(p.width.get()));
    let (s, w) = (start.clone(), Rc::downgrade(p));
    let g = grip.clone();
    drag.connect_drag_begin(move |_, _, _| {
        if let Some(p) = w.upgrade() {
            s.set(p.width.get());
        }
        g.add_css_class("dragging");
    });
    let (s, w) = (start.clone(), Rc::downgrade(p));
    drag.connect_drag_update(move |_, dx, _| {
        if let Some(p) = w.upgrade() {
            let next = policy::clamp_width(Some(f64::from(s.get()) - dx));
            if next != p.width.get() {
                p.width.set(next);
                if let Some(b) = p.built.borrow().as_ref() {
                    b.frame.set_size_request(next, -1);
                }
            }
        }
    });
    let weak = shell.downgrade();
    let (w, g) = (Rc::downgrade(p), grip.clone());
    drag.connect_drag_end(move |_, _, _| {
        g.remove_css_class("dragging");
        if let (Some(s), Some(p)) = (weak.upgrade(), w.upgrade()) {
            let width = p.width.get();
            s.update_settings(|st| st.set(WIDTH_KEY, f64::from(width)));
        }
    });
    grip.add_controller(drag);
}

/// The slot came in or went away: the addon hears about it.
fn revealed(_shell: &Shell, visible: bool) {
    if let Some(v) = view() {
        v.set_visible(visible);
    }
    super::view::emit_all("visibility", json!({ "visible": visible }));
}

fn focus_inside(p: &Rc<Pane>) {
    let b = p.built.borrow();
    let Some(b) = b.as_ref() else { return };
    if let Some(v) = b.view.borrow().as_ref() {
        v.widget().grab_focus();
        return;
    }
    if let Some(w) = b.stack.visible_child() {
        w.child_focus(gtk::DirectionType::TabForward);
    }
}

/// Settings › Addons changed the width (or tests): apply `w` now.
pub fn set_width(shell: &Shell, w: f64) {
    let width = policy::clamp_width(Some(w));
    if let Some(p) = pane() {
        p.width.set(width);
        if let Some(b) = p.built.borrow().as_ref() {
            b.frame.set_size_request(width, -1);
        }
    }
    shell.update_settings(|st| st.set(WIDTH_KEY, f64::from(width)));
}

/// The shell's window, for code that needs a parent.
pub fn window() -> Option<gtk::Window> {
    shell().map(|s| s.window().clone().upcast())
}
