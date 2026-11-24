//! Tabs down the left: one surface washed in the space's colour, laid out
//! the Mac's way (`Side.swift`) — back / forward / reload, the favourites as
//! a grid of soft squares, the space's name, the rows you keep, a hairline,
//! "New Tab", then whatever was opened today; the spaces at the foot.
//!
//! Nothing here has a border. The live row is a paper pill a shade stronger
//! than the column; everything else is spacing.
//!
//! Saved and Today are drawn as folders and rows (`folders::plan`, Mac:
//! `Fork/GroupsUI.swift`): a folder's line, then its tabs stepped in under
//! it; a shut folder hides them. A dragged row lands where
//! [`sidedrop::target`] reads it off those lines, in or out of a folder.

use super::{Nav, tabs};
use crate::palette::Mode;
use crate::shell::GtkShell;
use crate::shell::{Shell, TabView};
use crate::spaces::colour::{Rgb, Tint};
use copper_core::TabId;
use copper_core::groups::TabGroup;
use copper_core::settings::keys;
use copper_shell_core::agents::groups;
use copper_shell_core::folders::{self, FolderRow};
use copper_shell_core::sidedrop::{self, Kind, Line, Target};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Default column width (points, like the Mac's `sidebar.width`).
pub const WIDTH: f64 = 256.0;

/// How far a folder's rows step in (Windows: `FOLDER_STEP`).
const FOLDER_STEP: i32 = 20;
/// The room between two lines (the blocks' spacing).
const GAP: f32 = 2.0;

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
    /// The hairline between Saved and Today, with "↓ Clear" over its end.
    seam: gtk::Overlay,
    clear: gtk::Button,
    /// While a tab is dragged and Saved is empty: where to drop it to save.
    slot: gtk::Revealer,
    new_tab: gtk::Box,
    today: gtk::Box,
    /// The drop marker: a line where the dragged tab will land.
    marker: gtk::Box,
    chips: gtk::Box,
    rows: RefCell<HashMap<TabId, Row>>,
    /// Saved's then Today's lines as drawn, in order: in Saved, the widget,
    /// and the line a drag reads (its place filled in when it asks).
    lines: RefCell<Vec<(bool, gtk::Widget, Line)>>,
    /// Each folder mark's colour (`.folder-mark.f-<n>`), rewritten with
    /// the rows.
    folder_css: gtk::CssProvider,
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
        // Mac `ClearTabsDivider`: at rest the plain hairline, same height;
        // hovering it shows a small round button over its end, so nothing
        // moves. Ctrl+Shift+K does the same from the keyboard.
        let clear = gtk::Button::new();
        let arrow = gtk::Image::from_icon_name("go-down-symbolic");
        arrow.set_pixel_size(8);
        clear.set_child(Some(&arrow));
        clear.add_css_class("clear-seam");
        clear.set_action_name(Some("win.clear-tabs"));
        clear.set_halign(gtk::Align::End);
        clear.set_valign(gtk::Align::Center);
        clear.set_margin_end(6);
        clear.set_focusable(false);
        clear.set_opacity(0.0);
        clear.set_can_target(false);
        let seam = gtk::Overlay::new();
        seam.set_child(Some(&rule));
        seam.add_overlay(&clear);
        let hover = gtk::EventControllerMotion::new();
        let c = clear.clone();
        hover.connect_enter(move |_, _, _| {
            c.set_opacity(1.0);
            c.set_can_target(true);
        });
        let c = clear.clone();
        hover.connect_leave(move |_| {
            c.set_opacity(0.0);
            c.set_can_target(false);
        });
        seam.add_controller(hover);
        body.append(&seam);
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

        let folder_css = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &WidgetExt::display(&widget),
            &folder_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
        );

        let me = Rc::new(Column {
            widget,
            nav,
            scroller,
            body,
            pins,
            head,
            saved,
            seam,
            clear,
            slot,
            new_tab,
            today,
            marker,
            chips,
            rows: RefCell::default(),
            lines: RefCell::default(),
            folder_css,
            dragging: RefCell::default(),
        });
        me.wire_drop(shell);
        me
    }

    /// One drop target for the whole body: the header, Saved, the seam,
    /// New Tab and Today. The marker shows where the tab will land, stepped
    /// in as a folder's rows are, and the drop puts it exactly there
    /// ([`sidedrop::target`], [`Shell::drop_at_spot`]).
    fn wire_drop(self: &Rc<Self>, shell: &Shell) {
        let drop = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        let weak_shell = shell.downgrade();
        drop.connect_motion(move |_, _, y| {
            if let (Some(me), Some(s)) = (weak.upgrade(), weak_shell.upgrade()) {
                let target = me.target(&s, y);
                me.show_marker(target.as_ref());
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
            let target = me.target(&s, y);
            me.drag_ended();
            tracing::debug!(tab = %id, ?target, "tab dropped in the column");
            // Not now: the rows under the drop are about to be rebuilt.
            glib::idle_add_local_once(move || match target {
                Some(Target::Pins) => s.drop_on_pins(&id),
                Some(Target::Spot(spot)) => s.drop_at_spot(&id, &spot),
                // Over its own place: it stays.
                None => {}
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
    fn layout(&self) -> sidedrop::Layout {
        let bounds = |w: &gtk::Widget| {
            w.compute_bounds(&self.body)
                .map(|b| (b.y(), b.y() + b.height()))
        };
        let (head_top, head_bottom) = bounds(self.head.upcast_ref()).unwrap_or_default();
        let mut layout = sidedrop::Layout {
            pins: head_top,
            saved_top: head_bottom,
            new_tab: bounds(self.new_tab.upcast_ref()).unwrap_or_default(),
            gap: GAP,
            ..Default::default()
        };
        for (saved, widget, line) in self.lines.borrow().iter() {
            let Some((top, bottom)) = bounds(widget) else {
                continue;
            };
            let line = Line {
                top,
                bottom,
                ..line.clone()
            };
            if *saved {
                layout.saved.push(line);
            } else {
                layout.today.push(line);
            }
        }
        layout
    }

    /// Where the dragged tab would land with the pointer at `y`; `None`
    /// over its own place, where nothing opens (Mac: `SavedDrop`).
    fn target(&self, shell: &Shell, y: f64) -> Option<Target> {
        let dragged = self
            .dragging
            .borrow()
            .clone()
            .unwrap_or_else(|| TabId(String::new()));
        let Target::Spot(spot) = sidedrop::target(&self.layout(), y as f32) else {
            return Some(Target::Pins);
        };
        let all = groups::all(shell);
        let b = shell.browser();
        let row: Vec<(TabId, bool)> = b
            .row(b.current_space_id())
            .iter()
            .map(|t| (t.id.clone(), t.saved))
            .collect();
        let group = b.tab(&dragged).and_then(|t| t.group.clone());
        let in_folder = |t: &TabId, folder: &str| {
            groups::in_folder(&all, b.tab(t).and_then(|t| t.group.as_deref()), folder)
        };
        let stays = sidedrop::stays(&spot, &dragged, group.as_deref(), &row, in_folder);
        (!stays).then_some(Target::Spot(spot))
    }

    fn show_marker(&self, target: Option<&Target>) {
        // Over the favourites they show the drop themselves.
        let Some(Target::Spot(spot)) = target else {
            self.hide_marker();
            return;
        };
        if spot.saved && self.slot.reveals_child() {
            // The slot itself lights up; no line over it.
            self.slot.add_css_class("armed");
            self.marker.set_visible(false);
            return;
        }
        self.slot.remove_css_class("armed");
        self.marker
            .set_margin_top((spot.y - 1.5).max(0.0).round() as i32);
        self.marker
            .set_margin_start(spot.depth as i32 * FOLDER_STEP);
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
        // Nothing in Saved but this row (a folder's line counts).
        let empty = self
            .lines
            .borrow()
            .iter()
            .filter(|(saved, _, _)| *saved)
            .all(|(_, _, l)| matches!(&l.kind, Kind::Tab(t) if t == id));
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
            self.lines.borrow_mut().clear();
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
        let all = groups::all(shell);
        let tint = Tint::of(shell.browser().current_space(), shell.is_dark());
        let mut css = String::new();
        let mut lines = Vec::new();
        for (saved, block, views) in [
            (true, &self.saved, &v.saved),
            (false, &self.today, &v.today),
        ] {
            let filed: Vec<(&TabId, Option<&str>)> =
                views.iter().map(|t| (&t.id, t.group.as_deref())).collect();
            for (drawn, line) in plan_lines(&filed, &all) {
                let widget: gtk::Widget = match &drawn {
                    FolderRow::Head { hue, .. } => {
                        let class = format!("f-{}", lines.len());
                        if let Some(c) = folder_colour(hue.or(tint.hue), tint.dark) {
                            css.push_str(&format!(
                                ".folder-mark.{class} {{ color: {}; }}\n",
                                c.hex()
                            ));
                        }
                        folder_head(shell, &drawn, &class).upcast()
                    }
                    FolderRow::Tab { index, depth } => {
                        let t = &views[*index];
                        let row = tab_row(shell, t);
                        row.root.set_margin_start(*depth as i32 * FOLDER_STEP);
                        let w = row.root.clone().upcast();
                        rows.insert(t.id.clone(), row);
                        w
                    }
                };
                block.append(&widget);
                lines.push((saved, widget, line));
            }
        }
        self.folder_css.load_from_string(&css);
        drop(rows);
        self.lines.replace(lines);
        self.update_seam(shell);
        self.reveal_live(shell);
    }

    /// The seam shows under Saved rows, or while there is something to
    /// clear; its button only then.
    fn update_seam(&self, shell: &Shell) {
        let clearable = shell.clearable_tabs().len();
        let saved = self.lines.borrow().iter().any(|(saved, ..)| *saved);
        self.seam.set_visible(saved || clearable > 0);
        self.clear.set_visible(clearable > 0);
        let words = format!(
            "Clear {clearable} tab{}",
            if clearable == 1 { "" } else { "s" }
        );
        self.clear.set_tooltip_text(Some(&words));
        self.clear
            .update_property(&[gtk::accessible::Property::Label(&words)]);
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
        drop(rows);
        // A blank tab that loaded a page can now be cleared.
        self.update_seam(shell);
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

/// A block's lines: what to draw (`folders::plan`) and what a drag reads
/// off it (`sidedrop::Line`; its place is filled in when the drag asks).
/// `rows` are the block's tabs, in order, each with its folder's id.
fn plan_lines(rows: &[(&TabId, Option<&str>)], groups: &[TabGroup]) -> Vec<(FolderRow, Line)> {
    let filed: Vec<Option<&str>> = rows.iter().map(|r| r.1).collect();
    // The folders open above the line, outermost first.
    let mut open: Vec<String> = Vec::new();
    folders::plan(&filed, groups)
        .into_iter()
        .map(|drawn| {
            let (kind, depth, group) = match &drawn {
                FolderRow::Head {
                    id,
                    depth,
                    collapsed,
                    ..
                } => {
                    open.truncate(*depth);
                    let parent = open.last().cloned();
                    open.push(id.clone());
                    let kind = Kind::Folder {
                        id: id.clone(),
                        collapsed: *collapsed,
                    };
                    (kind, *depth, parent)
                }
                FolderRow::Tab { index, depth } => {
                    let group = depth.checked_sub(1).and_then(|d| open.get(d)).cloned();
                    (Kind::Tab(rows[*index].0.clone()), *depth, group)
                }
            };
            let line = Line {
                kind,
                top: 0.0,
                bottom: 0.0,
                depth,
                group,
            };
            (drawn, line)
        })
        .collect()
}

/// A folder's colour, or the space's when it has none (`GroupHead.colour`):
/// deeper than the column either way round. `None`: grey, the muted ink.
fn folder_colour(hue: Option<f64>, dark: bool) -> Option<Rgb> {
    hue.map(|h| {
        if dark {
            Rgb::hsv(h, 0.52, 0.82)
        } else {
            Rgb::hsv(h, 0.82, 0.66)
        }
    })
}

/// A folder's line (`GroupHead`): a row like the tabs', the folder in the
/// mark's place — shut or open, in its colour (`class`) — its name, and,
/// under the pointer, how many tabs a shut one holds. A click (or Return,
/// Space) opens or shuts it.
fn folder_head(shell: &Shell, line: &FolderRow, class: &str) -> gtk::Box {
    let root = tabs::focusable_button(gtk::Orientation::Horizontal, 9);
    root.add_css_class("copper-row");
    root.add_css_class("folder-head");
    let FolderRow::Head {
        id,
        label,
        depth,
        count,
        collapsed,
        ..
    } = line
    else {
        return root;
    };
    root.set_margin_start(*depth as i32 * FOLDER_STEP);
    let mark = gtk::Image::from_icon_name(if *collapsed {
        "folder-symbolic"
    } else {
        "folder-open-symbolic"
    });
    mark.set_pixel_size(16);
    mark.add_css_class("folder-mark");
    mark.add_css_class("muted");
    mark.add_css_class(class);
    root.append(&mark);
    let title = gtk::Label::new(Some(label));
    title.add_css_class("title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_width_chars(1);
    title.set_max_width_chars(8);
    root.append(&title);
    if *collapsed && *count > 0 {
        let n = gtk::Label::new(Some(&count.to_string()));
        n.add_css_class("count");
        n.add_css_class("faint");
        root.append(&n);
    }
    let tabs = if *count == 1 { "tab" } else { "tabs" };
    root.update_property(&[gtk::accessible::Property::Label(&format!(
        "{label}, folder, {count} {tabs}"
    ))]);
    root.update_state(&[gtk::accessible::State::Expanded(Some(!*collapsed))]);
    let toggle = {
        let weak = shell.downgrade();
        let id = id.clone();
        move || {
            let Some(s) = weak.upgrade() else { return };
            let id = id.clone();
            // Not now: this line is about to be rebuilt.
            glib::idle_add_local_once(move || groups::toggle(&s, &id));
        }
    };
    let click = gtk::GestureClick::new();
    let t = toggle.clone();
    click.connect_released(move |_, n, _, _| {
        if n == 1 {
            t();
        }
    });
    root.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, _| {
        use gtk::gdk::Key;
        if matches!(key, Key::Return | Key::KP_Enter | Key::space) {
            toggle();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    root.add_controller(keys);
    root
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

#[cfg(test)]
mod tests {
    use super::*;

    fn g(id: &str, name: &str, collapsed: bool) -> TabGroup {
        TabGroup {
            id: id.into(),
            name: name.into(),
            collapsed,
            ..Default::default()
        }
    }

    fn drawn(lines: &[(FolderRow, Line)]) -> Vec<String> {
        lines
            .iter()
            .map(|(_, l)| {
                let what = match &l.kind {
                    Kind::Tab(t) => t.0.clone(),
                    Kind::Folder { id, collapsed } => {
                        format!("[{id}{}]", if *collapsed { " shut" } else { "" })
                    }
                };
                format!("{}:{what} in {:?}", l.depth, l.group.as_deref())
            })
            .collect()
    }

    #[test]
    fn a_blocks_plan_becomes_lines_a_drag_reads() {
        // Mac-synced: Work holds Work › Docs; Misc is shut.
        let groups = [
            g("W", "Work", false),
            g("D", "Work › Docs", false),
            g("M", "Misc", true),
        ];
        let (a, b, c, d, e) = (
            TabId("a".into()),
            TabId("b".into()),
            TabId("c".into()),
            TabId("d".into()),
            TabId("e".into()),
        );
        let rows = [
            (&a, None),
            (&b, Some("d")),
            (&c, Some("W")),
            (&d, Some("M")),
            (&e, None),
        ];
        let lines = plan_lines(&rows, &groups);
        assert_eq!(
            drawn(&lines),
            [
                "0:a in None",
                "0:[W] in None",
                "1:[D] in Some(\"W\")",
                "2:b in Some(\"D\")",
                "1:c in Some(\"W\")",
                // Shut: its line, not its tab.
                "0:[M shut] in None",
                "0:e in None",
            ]
        );
        // Each line keeps what the column draws for it.
        assert!(matches!(
            &lines[2].0,
            FolderRow::Head { label, count: 1, .. } if label == "Docs"
        ));
        assert!(matches!(lines[3].0, FolderRow::Tab { index: 1, depth: 2 }));
    }
}
