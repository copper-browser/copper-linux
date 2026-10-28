//! Tabs down the left: one surface washed in the space's colour, laid out
//! the Mac's way (`Side.swift`) — back / forward / reload, the favourites as
//! a grid of soft squares, the space's name, the rows you keep, a hairline,
//! "New Tab", then whatever was opened today; the spaces at the foot.
//!
//! Nothing here has a border. The live row is a paper pill a shade stronger
//! than the column; everything else is spacing.

use super::dropzone::{self, Band, Landing, Layout};
use super::{Nav, tabs};
use crate::palette::Mode;
use crate::shell::GtkShell;
use crate::shell::{Shell, TabView};
use copper_core::TabId;
use copper_core::settings::keys;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Default column width (points, like the Mac's `sidebar.width`).
pub const WIDTH: f64 = 256.0;

struct Row {
    root: gtk::Box,
    mark: gtk::Box,
    title: gtk::Label,
    badge: gtk::Image,
    pin: bool,
}

pub struct Column {
    pub widget: gtk::Box,
    nav: Nav,
    scroller: gtk::ScrolledWindow,
    /// Header, Saved, the seam, New Tab, Today: one drop target.
    body: gtk::Box,
    pins: gtk::FlowBox,
    head: gtk::Box,
    saved: gtk::Box,
    rule: gtk::Box,
    /// While a tab is dragged and Saved is empty: where to drop it to save.
    slot: gtk::Revealer,
    new_tab: gtk::Box,
    today: gtk::Box,
    /// The drop marker: a line where the dragged tab will land.
    marker: gtk::Box,
    chips: gtk::Box,
    rows: RefCell<HashMap<TabId, Row>>,
    /// The Saved and Today rows as drawn, in order.
    order: RefCell<(Vec<TabId>, Vec<TabId>)>,
    /// The tab being dragged, from the moment its row is picked up.
    dragging: RefCell<Option<TabId>>,
}

impl Column {
    pub fn new(shell: &Shell) -> Rc<Column> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("copper-column");
        // Rows expand inside the column; the column itself never takes
        // the page's room.
        widget.set_hexpand(false);
        let width = shell
            .settings()
            .get(keys::SIDEBAR_WIDTH)
            .and_then(|v| v.as_f64())
            .unwrap_or(WIDTH)
            .clamp(200.0, 420.0);
        widget.set_size_request(width as i32, -1);
        widget.update_property(&[gtk::accessible::Property::Label("Tabs")]);

        // The top: window buttons (when the desktop puts them on the left),
        // back, forward, reload.
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.add_css_class("column-top");
        let controls = gtk::WindowControls::new(gtk::PackType::Start);
        top.append(&controls);
        let nav = Nav::new();
        nav.widget.set_valign(gtk::Align::Center);
        top.append(&nav.widget);
        let handle = gtk::WindowHandle::new();
        handle.set_child(Some(&top));
        widget.append(&handle);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("column-body");

        let pins = gtk::FlowBox::new();
        pins.add_css_class("pins");
        pins.set_selection_mode(gtk::SelectionMode::None);
        pins.set_homogeneous(true);
        pins.set_max_children_per_line(5);
        pins.set_min_children_per_line(1);
        pins.set_column_spacing(6);
        pins.set_row_spacing(6);
        pins.set_halign(gtk::Align::Fill);
        pins.set_visible(false);
        let weak = shell.downgrade();
        let drop = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::MOVE);
        drop.connect_drop(move |_, value, _, _| {
            let (Some(s), Ok(id)) = (weak.upgrade(), value.get::<String>()) else {
                return false;
            };
            if id.starts_with("space:") {
                return false;
            }
            glib::idle_add_local_once(move || s.drop_on_pins(&TabId(id)));
            true
        });
        pins.add_controller(drop);
        body.append(&pins);

        let head = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&head);
        let saved = gtk::Box::new(gtk::Orientation::Vertical, 2);
        body.append(&saved);
        let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rule.add_css_class("hairline");
        body.append(&rule);
        // Saved is empty and a tab is being dragged: the seam opens into a
        // slot that says what dropping there does.
        let slot = gtk::Revealer::new();
        slot.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        slot.set_transition_duration(120);
        slot.set_can_target(false);
        let slot_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        slot_box.add_css_class("save-slot");
        let slot_mark = gtk::Image::from_icon_name("bookmark-new-symbolic");
        slot_mark.set_pixel_size(14);
        slot_box.append(&slot_mark);
        let slot_label = gtk::Label::new(Some("Drop here to save"));
        slot_label.add_css_class("title");
        slot_label.set_xalign(0.0);
        slot_box.append(&slot_label);
        slot_box.update_property(&[gtk::accessible::Property::Label("Drop here to save")]);
        slot.set_child(Some(&slot_box));
        body.append(&slot);
        let new_tab = new_tab_row(shell);
        body.append(&new_tab);
        let today = gtk::Box::new(gtk::Orientation::Vertical, 2);
        today.set_margin_top(2);
        body.append(&today);

        // The body and the drop marker over it, scrolled together.
        let marker = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        marker.add_css_class("drop-marker");
        marker.set_valign(gtk::Align::Start);
        marker.set_halign(gtk::Align::Fill);
        marker.set_can_target(false);
        marker.set_visible(false);
        let stack = gtk::Overlay::new();
        stack.set_child(Some(&body));
        stack.add_overlay(&marker);
        stack.set_clip_overlay(&marker, true);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&stack));
        widget.append(&scroller);

        // The foot: the spaces, and the window menu.
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        foot.add_css_class("column-foot");
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        chips.set_hexpand(true);
        chips.set_valign(gtk::Align::Center);
        foot.append(&chips);
        let menu = gtk::MenuButton::new();
        menu.set_icon_name("open-menu-symbolic");
        menu.add_css_class("foot-button");
        menu.add_css_class("flat");
        menu.set_valign(gtk::Align::Center);
        menu.set_tooltip_text(Some("Menu"));
        menu.set_direction(gtk::ArrowType::Up);
        let weak = shell.downgrade();
        menu.set_create_popup_func(move |m| {
            if let Some(s) = weak.upgrade() {
                m.set_menu_model(Some(&super::window_menu(&s)));
            }
        });
        foot.append(&crate::features::door(shell));
        foot.append(&menu);
        widget.append(&foot);

        let me = Rc::new(Column {
            widget,
            nav,
            scroller,
            body,
            pins,
            head,
            saved,
            rule,
            slot,
            new_tab,
            today,
            marker,
            chips,
            rows: RefCell::default(),
            order: RefCell::default(),
            dragging: RefCell::default(),
        });
        me.wire_drop(shell);
        me
    }

    /// One drop target for the whole body: the header, Saved, the seam,
    /// New Tab and Today. The marker shows where the tab will land, and the
    /// drop puts it exactly there ([`dropzone::land`]).
    fn wire_drop(self: &Rc<Self>, shell: &Shell) {
        let drop = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        drop.connect_motion(move |_, _, y| {
            if let Some(me) = weak.upgrade() {
                let landing = me.landing(y);
                me.show_marker(&landing);
            }
            gtk::gdk::DragAction::MOVE
        });
        let weak = Rc::downgrade(self);
        drop.connect_leave(move |_| {
            if let Some(me) = weak.upgrade() {
                me.hide_marker();
            }
        });
        let weak = Rc::downgrade(self);
        let weak_shell = shell.downgrade();
        drop.connect_drop(move |_, value, _, y| {
            let (Some(me), Some(s), Ok(id)) =
                (weak.upgrade(), weak_shell.upgrade(), value.get::<String>())
            else {
                return false;
            };
            if id.starts_with("space:") {
                return false;
            }
            let id = TabId(id);
            me.dragging.replace(Some(id.clone()));
            let landing = me.landing(y);
            me.drag_ended();
            tracing::debug!(tab = %id, saved = landing.saved, before = ?landing.before, "tab dropped in the column");
            // Not now: the rows under the drop are about to be rebuilt.
            glib::idle_add_local_once(move || {
                s.drop_at(&id, landing.saved, landing.before.as_ref())
            });
            true
        });
        self.body.add_controller(drop);
        // Over the favourites the drop is theirs (they take it to pin): no
        // marker in the rows meanwhile.
        let over_pins = gtk::DropControllerMotion::new();
        let weak = Rc::downgrade(self);
        over_pins.connect_enter(move |_, _, _| {
            if let Some(me) = weak.upgrade() {
                me.hide_marker();
            }
        });
        self.pins.add_controller(over_pins);
    }

    /// The column as a drop sees it, in the body's coordinates.
    fn layout(&self) -> Layout {
        let rows = self.rows.borrow();
        let band = |id: &TabId| -> Option<Band> {
            let b = rows.get(id)?.root.compute_bounds(&self.body)?;
            Some(Band {
                id: id.clone(),
                top: b.y() as f64,
                bottom: (b.y() + b.height()) as f64,
            })
        };
        let order = self.order.borrow();
        let new_tab = self
            .new_tab
            .compute_bounds(&self.body)
            .map(|b| (b.y() as f64, (b.y() + b.height()) as f64))
            .unwrap_or_default();
        let saved_top = self
            .head
            .compute_bounds(&self.body)
            .map(|b| (b.y() + b.height()) as f64)
            .unwrap_or_default();
        Layout {
            saved: order.0.iter().filter_map(band).collect(),
            today: order.1.iter().filter_map(band).collect(),
            saved_top,
            new_tab,
        }
    }

    fn landing(&self, y: f64) -> Landing {
        let dragged = self
            .dragging
            .borrow()
            .clone()
            .unwrap_or_else(|| TabId(String::new()));
        dropzone::land(&self.layout(), y, &dragged)
    }

    fn show_marker(&self, landing: &Landing) {
        if landing.stays {
            self.hide_marker();
            return;
        }
        let armed = landing.saved && landing.empty && self.slot.reveals_child();
        if armed {
            // The slot itself lights up; no line over it.
            self.slot.add_css_class("armed");
            self.marker.set_visible(false);
            return;
        }
        self.slot.remove_css_class("armed");
        self.marker
            .set_margin_top((landing.line - 1.5).max(0.0).round() as i32);
        self.marker.set_visible(true);
    }

    fn hide_marker(&self) {
        self.marker.set_visible(false);
        self.slot.remove_css_class("armed");
    }

    /// A tab's row (or square, or pill) was picked up: open the Saved slot
    /// if Saved is empty, and the favourites' if there are none, so there is
    /// somewhere visible to drop it.
    pub fn drag_began(&self, id: &TabId) {
        self.dragging.replace(Some(id.clone()));
        let empty = self.order.borrow().0.iter().all(|t| t == id);
        let shown = self.widget.get_visible();
        self.slot.set_reveal_child(empty && shown);
        let pinned = self.rows.borrow().get(id).is_some_and(|r| r.pin);
        if shown && !pinned && self.pins.first_child().is_none() {
            self.pins.insert(&pin_slot(), -1);
            self.pins.set_visible(true);
        }
    }

    /// The drag is over (dropped or not).
    pub fn drag_ended(&self) {
        self.dragging.replace(None);
        self.hide_marker();
        self.slot.set_reveal_child(false);
        // Only while there are no favourites (a rebuild meanwhile took it).
        if let Some(c) = self.pins.first_child()
            && c.first_child().is_some_and(|w| w.has_css_class("pin-slot"))
        {
            self.pins.remove(&c);
            self.pins.set_visible(false);
        }
    }

    pub fn update_nav(&self, shell: &Shell) {
        self.nav.update(shell);
    }

    /// The space header and the chips.
    pub fn rebuild_space(&self, shell: &Shell) {
        clear(&self.head);
        self.head.append(&crate::spaces::ui::header(shell));
        clear(&self.chips);
        self.chips.append(&crate::spaces::ui::chips(shell));
    }

    /// Every row and pin, from scratch.
    pub fn rebuild_tabs(&self, shell: &Shell) {
        if !self.widget.get_visible() {
            self.rows.borrow_mut().clear();
            return;
        }
        let v = shell.visible();
        while let Some(c) = self.pins.first_child() {
            self.pins.remove(&c);
        }
        clear(&self.saved);
        clear(&self.today);
        let mut rows = self.rows.borrow_mut();
        rows.clear();
        self.pins.set_visible(!v.pins.is_empty());
        self.pins
            .set_min_children_per_line(v.pins.len().clamp(1, 5) as u32);
        for t in &v.pins {
            let row = pin_square(shell, t);
            self.pins.insert(&row.root, -1);
            if let Some(child) = row.root.parent() {
                child.set_focusable(false);
            }
            rows.insert(t.id.clone(), row);
        }
        for t in &v.saved {
            let row = tab_row(shell, t);
            self.saved.append(&row.root);
            rows.insert(t.id.clone(), row);
        }
        self.rule.set_visible(!v.saved.is_empty());
        for t in &v.today {
            let row = tab_row(shell, t);
            self.today.append(&row.root);
            rows.insert(t.id.clone(), row);
        }
        drop(rows);
        self.order.replace((
            v.saved.iter().map(|t| t.id.clone()).collect(),
            v.today.iter().map(|t| t.id.clone()).collect(),
        ));
        self.reveal_live(shell);
    }

    /// Move the live pill without rebuilding.
    pub fn mark_live(&self, shell: &Shell) {
        let active = shell.active_id();
        for (id, row) in self.rows.borrow().iter() {
            if Some(id) == active.as_ref() {
                row.root.add_css_class("live");
            } else {
                row.root.remove_css_class("live");
            }
        }
        self.reveal_live(shell);
    }

    /// One tab's title, mark and state.
    pub fn update_tab(&self, shell: &Shell, id: &TabId) {
        let rows = self.rows.borrow();
        let Some(row) = rows.get(id) else { return };
        let Some(t) = shell.tab_view(id) else { return };
        fill(shell, row, &t);
    }

    /// Pin `id`'s square, while it is drawn.
    pub fn pin(&self, id: &TabId) -> Option<gtk::Widget> {
        let rows = self.rows.borrow();
        let row = rows.get(id).filter(|r| r.pin)?;
        Some(row.root.clone().upcast())
    }

    /// Keep the live row in sight.
    fn reveal_live(&self, shell: &Shell) {
        let Some(id) = shell.active_id() else { return };
        let rows = self.rows.borrow();
        let Some(row) = rows.get(&id) else { return };
        let root = row.root.clone();
        let scroller = self.scroller.clone();
        glib::idle_add_local_once(move || {
            let Some(child) = scroller.child() else {
                return;
            };
            let Some(b) = root.compute_bounds(&child) else {
                return;
            };
            let adj = scroller.vadjustment();
            let (top, bottom) = (b.y() as f64, (b.y() + b.height()) as f64);
            if top < adj.value() {
                adj.set_value(top - 8.0);
            } else if bottom > adj.value() + adj.page_size() {
                adj.set_value(bottom - adj.page_size() + 8.0);
            }
        });
    }
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// A row: mark, title, a quiet badge, the cross under the pointer.
fn tab_row(shell: &Shell, t: &TabView) -> Row {
    let root = tabs::focusable_button(gtk::Orientation::Horizontal, 9);
    root.add_css_class("copper-row");
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.set_size_request(16, 16);
    mark.set_valign(gtk::Align::Center);
    root.append(&mark);
    let title = gtk::Label::new(None);
    title.add_css_class("title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_width_chars(1);
    // The column's width is its own; a long title never widens it.
    title.set_max_width_chars(8);
    root.append(&title);
    let badge = gtk::Image::new();
    badge.add_css_class("badge");
    badge.set_pixel_size(12);
    root.append(&badge);
    // The cross: on a Saved row the one way to take it out of Saved (it
    // closes too; Reopen Closed Tab puts it back); on a Today row, close.
    // ⌘W, middle-click and Delete only unload a Saved row (Arc).
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("close");
    close.set_valign(gtk::Align::Center);
    close.set_focusable(false);
    let (tip, name) = if t.saved {
        ("Remove from Saved", "Remove from Saved")
    } else {
        ("Close Tab", "Close tab")
    };
    close.set_tooltip_text(Some(tip));
    close.update_property(&[gtk::accessible::Property::Label(name)]);
    let weak = shell.downgrade();
    let id = t.id.clone();
    close.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            s.cross(&id);
        }
    });
    root.append(&close);
    tabs::wire_activation(shell, root.upcast_ref(), &t.id);
    tabs::wire_drag_source(shell, root.upcast_ref(), &t.id);
    let row = Row {
        root,
        mark,
        title,
        badge,
        pin: false,
    };
    fill(shell, &row, t);
    row
}

/// While a tab is dragged and there are no favourites: where to drop it to
/// pin (the favourites' own drop target takes it).
fn pin_slot() -> gtk::Box {
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.add_css_class("pin");
    slot.add_css_class("pin-slot");
    slot.set_size_request(42, 42);
    slot.set_tooltip_text(Some("Drop here to pin"));
    slot.update_property(&[gtk::accessible::Property::Label("Drop here to pin")]);
    let mark = gtk::Image::from_icon_name("view-pin-symbolic");
    mark.set_pixel_size(14);
    mark.set_hexpand(true);
    slot.append(&mark);
    slot
}

/// A favourite: a soft square with the site's mark.
fn pin_square(shell: &Shell, t: &TabView) -> Row {
    let root = tabs::focusable_button(gtk::Orientation::Horizontal, 0);
    root.add_css_class("pin");
    root.set_size_request(42, 42);
    root.set_hexpand(true);
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    mark.set_hexpand(true);
    root.append(&mark);
    let title = gtk::Label::new(None);
    let badge = gtk::Image::new();
    tabs::wire_activation(shell, root.upcast_ref(), &t.id);
    tabs::wire_drag(shell, root.upcast_ref(), &t.id, false);
    super::peek::watch(shell, root.upcast_ref(), &t.id);
    let row = Row {
        root,
        mark,
        title,
        badge,
        pin: true,
    };
    fill(shell, &row, t);
    row
}

fn fill(shell: &Shell, row: &Row, t: &TabView) {
    let set = |on: bool, class: &str| {
        if on {
            row.root.add_css_class(class);
        } else {
            row.root.remove_css_class(class);
        }
    };
    set(t.live, "live");
    set(t.sleeping, "sleeping");
    set(t.crashed, "crashed");
    set(t.private, "private-tab");
    while let Some(c) = row.mark.first_child() {
        row.mark.remove(&c);
    }
    if row.pin {
        let m = if tabs::shows_icons(shell) && t.favicon.is_some() {
            tabs::mark(shell, t, 18)
        } else {
            let l = gtk::Label::new(Some(&pin_letter(t)));
            l.add_css_class("letter");
            l.add_css_class("muted");
            l.upcast()
        };
        row.mark.append(&m);
        // A Calendar pin's card says more than its name would.
        let tip = (!copper_shell_core::calendar::peeks(&t.url)).then_some(t.label.as_str());
        row.root.set_tooltip_text(tip);
        row.root
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{}, pinned",
                t.label
            ))]);
        return;
    }
    row.mark.append(&tabs::mark(shell, t, 16));
    row.title.set_text(&t.label);
    let (badge, tip) = if t.audio {
        (Some("audio-volume-high-symbolic"), "Playing sound")
    } else if t.private {
        (Some("view-conceal-symbolic"), "Private")
    } else {
        (None, "")
    };
    row.badge.set_icon_name(badge);
    row.badge.set_visible(badge.is_some());
    row.badge
        .set_tooltip_text(Some(tip).filter(|t| !t.is_empty()));
    crate::agents::paint_badge(&t.id, &row.badge);
    let mut name = t.label.clone();
    if t.sleeping {
        name.push_str(", asleep");
    }
    if t.private {
        name.push_str(", private");
    }
    row.root
        .update_property(&[gtk::accessible::Property::Label(&name)]);
}

/// A pin's letter: its own, else the site's first letter.
fn pin_letter(t: &TabView) -> String {
    match &t.pin {
        Some(p) if !p.trim().is_empty() => p.chars().take(2).collect(),
        _ => t
            .host
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "·".into()),
    }
}

/// "New Tab", between the kept rows and today's.
fn new_tab_row(shell: &Shell) -> gtk::Box {
    let root = tabs::focusable_button(gtk::Orientation::Horizontal, 9);
    root.add_css_class("copper-row");
    root.add_css_class("new-tab-row");
    let plus = gtk::Image::from_icon_name("list-add-symbolic");
    plus.set_pixel_size(14);
    plus.set_size_request(16, 16);
    root.append(&plus);
    let label = gtk::Label::new(Some("New Tab"));
    label.add_css_class("title");
    label.set_hexpand(true);
    label.set_xalign(0.0);
    root.append(&label);
    root.update_property(&[gtk::accessible::Property::Label("New tab")]);
    let click = gtk::GestureClick::new();
    let weak = shell.downgrade();
    click.connect_released(move |_, _, _, _| {
        if let Some(s) = weak.upgrade() {
            s.ui().card.open(&s, Mode::Launch);
        }
    });
    root.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    let weak = shell.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        use gtk::gdk::Key;
        if matches!(key, Key::Return | Key::KP_Enter | Key::space) {
            if let Some(s) = weak.upgrade() {
                s.ui().card.open(&s, Mode::Launch);
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    root.add_controller(keys);
    // A tab dropped on it lands through the column's own target: its upper
    // half ends Saved, its lower half starts Today.
    root
}
