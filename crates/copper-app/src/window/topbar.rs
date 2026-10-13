//! Tabs across the top: the title bar *is* the row of tabs (Mac:
//! `TabBar.swift`) — back / forward / reload, the space, the pins as small
//! squares, then the tabs sharing the width, the live one on a paper pill.
//!
//! No horizontal scrollbar, ever: tabs shrink to their mark when the row is
//! crowded, and when even marks do not fit, the ones furthest from the
//! live tab wait behind a "+N" that opens the tab list (`Ctrl+K`).

use super::{Nav, tabs};
use crate::shell::{Shell, TabView};
use crate::spaces::colour::Tint;
use copper_core::TabId;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Below this share, tabs show only their mark.
const COMPACT_BELOW: i32 = 100;
/// A tab showing only its mark.
const MIN_TAB: i32 = 38;
const GAP: i32 = 3;

struct Pill {
    root: gtk::Box,
    mark: gtk::Box,
    title: gtk::Label,
    pin: bool,
}

pub struct TopBar {
    pub widget: gtk::WindowHandle,
    nav: Nav,
    space: gtk::Box,
    pins: gtk::Box,
    strip: gtk::Box,
    scroller: gtk::ScrolledWindow,
    overflow: gtk::Button,
    pills: RefCell<HashMap<TabId, Pill>>,
    width: Cell<i32>,
    compact: Cell<bool>,
}

impl TopBar {
    pub fn new(shell: &Shell) -> Rc<TopBar> {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("copper-topbar-inner");
        bar.append(&gtk::WindowControls::new(gtk::PackType::Start));
        let nav = Nav::new();
        nav.widget.set_valign(gtk::Align::Center);
        bar.append(&nav.widget);
        let space = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        space.set_valign(gtk::Align::Center);
        bar.append(&space);
        let pins = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        pins.set_valign(gtk::Align::Center);
        pins.set_hexpand(false);
        bar.append(&pins);

        let strip = gtk::Box::new(gtk::Orientation::Horizontal, GAP);
        strip.set_homogeneous(true);
        strip.set_halign(gtk::Align::Start);
        strip.set_valign(gtk::Align::Center);
        let scroller = gtk::ScrolledWindow::new();
        // External: never a scrollbar, and the row never forces the window
        // wider; what does not fit is moved behind "+N" below.
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        scroller.set_hexpand(true);
        scroller.set_child(Some(&strip));
        bar.append(&scroller);

        let overflow = gtk::Button::with_label("");
        overflow.add_css_class("overflow-count");
        overflow.set_valign(gtk::Align::Center);
        overflow.set_visible(false);
        overflow.set_tooltip_text(Some("All tabs (Ctrl+K)"));
        overflow.set_action_name(Some("win.palette"));
        bar.append(&overflow);

        let plus = gtk::Button::from_icon_name("list-add-symbolic");
        plus.add_css_class("foot-button");
        plus.set_valign(gtk::Align::Center);
        plus.set_tooltip_text(Some("New Tab (Ctrl+T)"));
        plus.update_property(&[gtk::accessible::Property::Label("New tab")]);
        plus.set_action_name(Some("win.new-tab"));
        bar.append(&plus);
        let menu = gtk::MenuButton::new();
        menu.set_icon_name("open-menu-symbolic");
        menu.add_css_class("foot-button");
        menu.add_css_class("flat");
        menu.set_valign(gtk::Align::Center);
        menu.set_tooltip_text(Some("Menu"));
        let weak = shell.downgrade();
        menu.set_create_popup_func(move |m| {
            if let Some(s) = weak.upgrade() {
                m.set_menu_model(Some(&super::window_menu(&s)));
            }
        });
        bar.append(&crate::features::door(shell));
        bar.append(&menu);
        bar.append(&gtk::WindowControls::new(gtk::PackType::End));

        let widget = gtk::WindowHandle::new();
        widget.add_css_class("copper-topbar");
        widget.set_child(Some(&bar));
        widget.set_visible(false);

        let me = Rc::new(TopBar {
            widget,
            nav,
            space,
            pins,
            strip,
            scroller,
            overflow,
            pills: RefCell::default(),
            width: Cell::new(0),
            compact: Cell::new(false),
        });
        // The row's width changes with the window: re-lay the tabs.
        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        me.scroller.hadjustment().connect_page_size_notify(move |adj| {
            let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) else {
                return;
            };
            let w = adj.page_size() as i32;
            if w != me.width.get() {
                me.width.set(w);
                glib::idle_add_local_once(move || me.rebuild_tabs(&s));
            }
        });
        me
    }

    pub fn update_nav(&self, shell: &Shell) {
        self.nav.update(shell);
    }

    /// The space chip: its colour, its name, its menu.
    pub fn rebuild_space(&self, shell: &Shell) {
        while let Some(c) = self.space.first_child() {
            self.space.remove(&c);
        }
        let (sp, tint) = {
            let b = shell.browser();
            let sp = b.current_space().clone();
            let tint = Tint::of(&sp, shell.is_dark()).off_column();
            (sp, tint)
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        row.append(&crate::spaces::ui::mark_widget(&sp, tint.swatch(), 1.0, 14));
        let name = gtk::Label::new(Some(&sp.name));
        name.add_css_class("name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(16);
        row.append(&name);
        let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
        chevron.set_pixel_size(12);
        chevron.add_css_class("faint");
        row.append(&chevron);
        let button = gtk::Button::new();
        button.add_css_class("top-space");
        button.set_child(Some(&row));
        button.update_property(&[gtk::accessible::Property::Label(&format!("Space {}, menu", sp.name))]);
        let weak = shell.downgrade();
        let id = sp.id.clone();
        button.connect_clicked(move |b| {
            if let Some(s) = weak.upgrade() {
                crate::spaces::ui::popover(&s, &id, b).popup();
            }
        });
        self.space.append(&button);
    }

    /// Every pin and tab that fits, from scratch.
    pub fn rebuild_tabs(&self, shell: &Shell) {
        if !self.widget.get_visible() {
            self.pills.borrow_mut().clear();
            return;
        }
        let v = shell.visible();
        for b in [&self.pins, &self.strip] {
            while let Some(c) = b.first_child() {
                b.remove(&c);
            }
        }
        let mut pills = self.pills.borrow_mut();
        pills.clear();
        for t in &v.pins {
            let p = pin(shell, t);
            self.pins.append(&p.root);
            pills.insert(t.id.clone(), p);
        }
        let row: Vec<&TabView> = v.saved.iter().chain(&v.today).collect();
        let mut width = self.width.get().max(self.scroller.width());
        if width <= 0 {
            // Not laid out yet: a guess from the window, corrected on the
            // first allocation.
            width = (shell.window().width() - 420).max(200);
        }
        let (shown, compact) = fit(row.len(), row.iter().position(|t| t.live), width);
        self.compact.set(compact);
        for t in &row[shown.clone()] {
            let p = pill(shell, t, compact);
            self.strip.append(&p.root);
            pills.insert(t.id.clone(), p);
        }
        let hidden = row.len() - shown.len();
        self.overflow.set_visible(hidden > 0);
        self.overflow.set_label(&format!("+{hidden}"));
    }

    pub fn update_tab(&self, shell: &Shell, id: &TabId) {
        let pills = self.pills.borrow();
        let Some(p) = pills.get(id) else { return };
        let Some(t) = shell.tab_view(id) else { return };
        fill(shell, p, &t, self.compact.get());
    }
}

/// Which of `n` tabs fit in `width` (a window around the live one), and
/// whether they show only their marks.
pub fn fit(n: usize, live: Option<usize>, width: i32) -> (std::ops::Range<usize>, bool) {
    if n == 0 {
        return (0..0, false);
    }
    let width = if width <= 0 { 800 } else { width };
    let share = (width - GAP * (n as i32 - 1)) / n as i32;
    if share >= COMPACT_BELOW {
        return (0..n, false);
    }
    let cap = ((width + GAP) / (MIN_TAB + GAP)).max(1) as usize;
    if n <= cap {
        return (0..n, true);
    }
    let live = live.unwrap_or(0);
    let start = live.saturating_sub(cap / 2).min(n - cap);
    (start..start + cap, true)
}

fn pill(shell: &Shell, t: &TabView, compact: bool) -> Pill {
    let root = tabs::focusable_button(gtk::Orientation::Horizontal, 7);
    root.add_css_class("tab-pill");
    root.set_overflow(gtk::Overflow::Hidden);
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.set_valign(gtk::Align::Center);
    root.append(&mark);
    let title = gtk::Label::new(None);
    title.add_css_class("title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_width_chars(1);
    title.set_max_width_chars(22);
    root.append(&title);
    let close = gtk::Button::from_icon_name("window-close-symbolic");
    close.add_css_class("close");
    close.set_valign(gtk::Align::Center);
    close.set_focusable(false);
    close.set_tooltip_text(Some("Close Tab"));
    close.update_property(&[gtk::accessible::Property::Label("Close tab")]);
    let weak = shell.downgrade();
    let id = t.id.clone();
    close.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            s.close(&id);
        }
    });
    root.append(&close);
    if compact {
        root.add_css_class("compact");
        root.set_size_request(MIN_TAB, 30);
        title.set_visible(false);
        close.set_visible(false);
        mark.set_hexpand(true);
        mark.set_halign(gtk::Align::Center);
        root.set_hexpand(false);
        root.set_tooltip_text(Some(&t.label));
    } else {
        root.set_size_request(-1, 30);
    }
    tabs::wire_activation(shell, root.upcast_ref(), &t.id);
    tabs::wire_drag(shell, root.upcast_ref(), &t.id, false);
    let p = Pill {
        root,
        mark,
        title,
        pin: false,
    };
    fill(shell, &p, t, compact);
    p
}

fn pin(shell: &Shell, t: &TabView) -> Pill {
    let root = tabs::focusable_button(gtk::Orientation::Horizontal, 0);
    root.add_css_class("top-pin");
    root.set_size_request(MIN_TAB, 30);
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    mark.set_hexpand(true);
    root.append(&mark);
    root.set_hexpand(false);
    root.set_tooltip_text(Some(&t.label));
    tabs::wire_activation(shell, root.upcast_ref(), &t.id);
    tabs::wire_drag(shell, root.upcast_ref(), &t.id, false);
    let p = Pill {
        root,
        mark,
        title: gtk::Label::new(None),
        pin: true,
    };
    fill(shell, &p, t, true);
    p
}

fn fill(shell: &Shell, p: &Pill, t: &TabView, compact: bool) {
    for (on, class) in [(t.live, "live"), (t.sleeping, "sleeping")] {
        if on {
            p.root.add_css_class(class);
        } else {
            p.root.remove_css_class(class);
        }
    }
    while let Some(c) = p.mark.first_child() {
        p.mark.remove(&c);
    }
    let show_mark = p.pin || compact || tabs::shows_icons(shell);
    if show_mark {
        p.mark.append(&tabs::mark(shell, t, 16));
    }
    p.mark.set_visible(show_mark);
    p.title.set_text(&t.label);
    let mut name = t.label.clone();
    if p.pin {
        name.push_str(", pinned");
    }
    if t.sleeping {
        name.push_str(", asleep");
    }
    p.root.update_property(&[gtk::accessible::Property::Label(&name)]);
    if compact || p.pin {
        p.root.set_tooltip_text(Some(&t.label));
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn tabs_share_the_row_then_shrink_then_wait() {
        assert_eq!(fit(4, Some(0), 800), (0..4, false));
        let (r, compact) = fit(12, Some(0), 800);
        assert_eq!(r, 0..12);
        assert!(compact);
        // 40 tabs in a 560-px row: some wait, the live one is always shown.
        let (r, compact) = fit(40, Some(30), 560);
        assert!(compact);
        assert!(r.contains(&30));
        assert!(r.len() < 40);
        assert!(r.len() as i32 * (super::MIN_TAB + super::GAP) <= 560 + super::GAP);
        let (r, _) = fit(40, Some(39), 560);
        assert_eq!(r.end, 40);
        assert_eq!(fit(0, None, 500), (0..0, false));
    }
}
