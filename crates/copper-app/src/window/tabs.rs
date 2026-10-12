//! What every drawn tab shares, in the column or the row: its mark, its
//! menu, clicking, the keyboard, and dragging.

use crate::shell::{Shell, TabView};
use copper_core::TabId;
use copper_core::settings::Glyph;
use copper_core::settings::keys;
use gtk4 as gtk;
use gtk4::prelude::*;

/// Site icons beside titles (Linux default) or letters (Settings › Tabs).
pub fn shows_icons(shell: &Shell) -> bool {
    let s = shell.settings();
    s.get(keys::GLYPH).is_none() || s.glyph() == Glyph::Icons
}

/// A tab's mark: its icon, or its letter on a chip.
pub fn mark(shell: &Shell, t: &TabView, size: i32) -> gtk::Widget {
    if shows_icons(shell)
        && let Some(tex) = &t.favicon
    {
        let p = icon_image(tex, size);
        if t.sleeping {
            p.set_opacity(0.7);
        }
        return p.upcast();
    }
    if t.private && t.url.is_empty() {
        let i = gtk::Image::from_icon_name("view-conceal-symbolic");
        i.set_pixel_size(size.min(16));
        return i.upcast();
    }
    let letter = if t.pin.is_some() { t.letter.clone() } else { t.host.clone() };
    let source = if letter.is_empty() { t.label.clone() } else { letter };
    super::letter_chip(&source, size).upcast()
}

/// A site icon at exactly `size` pixels.
pub fn icon_image(tex: &gtk::gdk::Texture, size: i32) -> gtk::Image {
    let i = gtk::Image::from_paintable(Some(tex));
    i.set_pixel_size(size);
    i.set_halign(gtk::Align::Center);
    i.set_valign(gtk::Align::Center);
    i.add_css_class("favicon");
    i
}

/// The menu for one tab.
pub fn menu(shell: &Shell, id: &TabId) -> gio::Menu {
    let b = shell.browser();
    let Some(t) = b.tab(id) else {
        return gio::Menu::new();
    };
    let menu = gio::Menu::new();
    let page = gio::Menu::new();
    page.append(Some("Reload"), Some(&format!("win.tab-reload::{id}")));
    page.append(Some("Duplicate"), Some(&format!("win.tab-duplicate::{id}")));
    page.append(Some("Copy Address"), Some(&format!("win.tab-copy-address::{id}")));
    menu.append_section(None, &page);

    let place = gio::Menu::new();
    if !t.private {
        place.append(
            Some(if t.is_pinned() { "Unpin" } else { "Pin" }),
            Some(&format!("win.tab-pin::{id}")),
        );
    }
    if !t.is_pinned() && !t.private {
        place.append(
            Some(if t.saved { "Move to Today" } else { "Keep" }),
            Some(&format!("win.tab-keep::{id}")),
        );
        let others: Vec<_> = b.spaces().filter(|s| s.id != t.space).collect();
        if !others.is_empty() {
            let to = gio::Menu::new();
            for s in others {
                to.append(Some(&s.name), Some(&format!("win.tab-to-space::{id}|{}", s.id)));
            }
            place.append_submenu(Some("Move to Space"), &to);
        }
    }
    menu.append_section(None, &place);

    let close = gio::Menu::new();
    close.append(Some("Close Other Tabs"), Some(&format!("win.tab-close-others::{id}")));
    close.append(Some("Close Tab"), Some(&format!("win.tab-close::{id}")));
    menu.append_section(None, &close);
    menu
}

/// Pop the tab's menu up on `widget` at (`x`, `y`).
pub fn popup_menu(shell: &Shell, id: &TabId, widget: &gtk::Widget, at: Option<(f64, f64)>) {
    let pop = gtk::PopoverMenu::from_model(Some(&menu(shell, id)));
    pop.set_parent(widget);
    pop.set_has_arrow(false);
    if let Some((x, y)) = at {
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    }
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

/// Click to select, middle-click to close, right-click for the menu,
/// Return / Space / Delete / Menu from the keyboard.
pub fn wire_activation(shell: &Shell, widget: &gtk::Widget, id: &TabId) {
    let click = gtk::GestureClick::new();
    click.set_button(0);
    let weak = shell.downgrade();
    let tab = id.clone();
    click.connect_released(move |g, n, x, y| {
        let Some(s) = weak.upgrade() else { return };
        match g.current_button() {
            1 if n == 1 => {
                s.select(&tab);
                if let Some(p) = s.active_page() {
                    crate::palette::p_focus(&p);
                }
            }
            2 => {
                s.close(&tab);
            }
            3 => {
                if let Some(w) = g.widget() {
                    popup_menu(&s, &tab, &w, Some((x, y)));
                }
            }
            _ => {}
        }
    });
    widget.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    let weak = shell.downgrade();
    let tab = id.clone();
    let w = widget.downgrade();
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(s) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        use gtk::gdk::Key;
        match key {
            Key::Return | Key::KP_Enter | Key::space => {
                s.select(&tab);
                glib::Propagation::Stop
            }
            Key::Delete | Key::BackSpace => {
                s.close(&tab);
                glib::Propagation::Stop
            }
            Key::Menu => {
                if let Some(w) = w.upgrade() {
                    popup_menu(&s, &tab, &w, None);
                }
                glib::Propagation::Stop
            }
            Key::F10 if state.contains(gtk::gdk::ModifierType::SHIFT_MASK) => {
                if let Some(w) = w.upgrade() {
                    popup_menu(&s, &tab, &w, None);
                }
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    widget.add_controller(keys);
}

/// Where along an axis a drop lands: the far half means "after".
pub fn after_half(pos: f64, extent: i32) -> bool {
    pos > extent as f64 / 2.0
}

/// Make `widget` a drag source for tab `id`, and a drop target that
/// reorders onto it (`vertical`: before/after by y, else by x).
pub fn wire_drag(shell: &Shell, widget: &gtk::Widget, id: &TabId, vertical: bool) {
    let drag = gtk::DragSource::new();
    drag.set_actions(gtk::gdk::DragAction::MOVE);
    let tab = id.clone();
    drag.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(&tab.0.to_value()))
    });
    let w = widget.downgrade();
    drag.connect_drag_begin(move |src, _| {
        if let Some(w) = w.upgrade() {
            let icon = gtk::WidgetPaintable::new(Some(&w));
            src.set_icon(Some(&icon), 12, 12);
        }
    });
    widget.add_controller(drag);

    let drop = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::MOVE);
    let w = widget.downgrade();
    drop.connect_motion(move |_, x, y| {
        if let Some(w) = w.upgrade() {
            let after = if vertical {
                after_half(y, w.height())
            } else {
                after_half(x, w.width())
            };
            w.remove_css_class(if after { "drop-before" } else { "drop-after" });
            w.add_css_class(if after { "drop-after" } else { "drop-before" });
        }
        gtk::gdk::DragAction::MOVE
    });
    let w = widget.downgrade();
    drop.connect_leave(move |_| {
        if let Some(w) = w.upgrade() {
            w.remove_css_class("drop-before");
            w.remove_css_class("drop-after");
        }
    });
    let weak = shell.downgrade();
    let target = id.clone();
    let w = widget.downgrade();
    drop.connect_drop(move |_, value, x, y| {
        let (Some(s), Ok(dragged)) = (weak.upgrade(), value.get::<String>()) else {
            return false;
        };
        if dragged.starts_with("space:") {
            return false;
        }
        let Some(w) = w.upgrade() else { return false };
        w.remove_css_class("drop-before");
        w.remove_css_class("drop-after");
        let after = if vertical {
            after_half(y, w.height())
        } else {
            after_half(x, w.width())
        };
        let dragged = TabId(dragged);
        // Not now: the drop's own widget is about to be rebuilt.
        let target = target.clone();
        glib::idle_add_local_once(move || s.drop_tab(&dragged, &target, after));
        true
    });
    widget.add_controller(drop);
}

/// A widget as a focusable, named button for assistive technology.
pub fn focusable_button(orientation: gtk::Orientation, spacing: i32) -> gtk::Box {
    let b: gtk::Box = glib::Object::builder()
        .property("orientation", orientation)
        .property("spacing", spacing)
        .property("accessible-role", gtk::AccessibleRole::Button)
        .build();
    b.set_focusable(true);
    b.set_cursor_from_name(Some("default"));
    b
}

#[cfg(test)]
mod tests {
    #[test]
    fn halves() {
        assert!(!super::after_half(10.0, 34));
        assert!(super::after_half(20.0, 34));
    }
}
