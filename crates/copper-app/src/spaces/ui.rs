//! Space widgets: the header in the column, the chips at its foot, the menu,
//! the editor and the delete question (Mac: `Fork/SpacesUI.swift`).
//!
//! - Header: the space's mark and name; click for the space menu.
//! - Chips: every space at the column's foot, the current one on a soft
//!   square; click to switch, right-click for its menu, drag to reorder,
//!   drop a tab on one to move it there. `+` makes a new space.
//! - Editor (`adw::Dialog`): name, icon, colour, own cookies.
//! - Delete: asks once, says where the tabs go.

use super::colour::{Rgb, SpaceColour, Tint};
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::TabId;
use copper_core::session::Space;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

/// The space menu for `space` (a `gio::MenuModel` over `win.*` actions).
pub fn menu(shell: &Shell, space: &str) -> gio::Menu {
    let b = shell.browser();
    let menu = gio::Menu::new();
    let many = b.spaces().count() > 1;
    if many {
        let switch = gio::Menu::new();
        for sp in b.spaces() {
            let label = if sp.id == b.current_space_id() {
                format!("{}  ✓", sp.name)
            } else {
                sp.name.clone()
            };
            switch.append(Some(&label), Some(&format!("win.space-switch::{}", sp.id)));
        }
        menu.append_section(None, &switch);
    }
    let edit = gio::Menu::new();
    edit.append(
        Some("Edit Space…"),
        Some(&format!("win.space-edit::{space}")),
    );
    edit.append(Some("New Space"), Some("win.new-space"));
    menu.append_section(None, &edit);
    if many {
        let order = gio::Menu::new();
        let at = b.spaces().position(|s| s.id == space).unwrap_or(0);
        if at > 0 {
            order.append(Some("Move Left"), Some(&format!("win.space-left::{space}")));
        }
        if at + 1 < b.spaces().count() {
            order.append(
                Some("Move Right"),
                Some(&format!("win.space-right::{space}")),
            );
        }
        order.append(
            Some("Delete Space…"),
            Some(&format!("win.space-delete::{space}")),
        );
        menu.append_section(None, &order);
    }
    menu
}

/// A popover menu for `space`, anchored on `parent`.
pub fn popover(shell: &Shell, space: &str, parent: &impl IsA<gtk::Widget>) -> gtk::PopoverMenu {
    let pop = gtk::PopoverMenu::from_model(Some(&menu(shell, space)));
    pop.set_parent(parent);
    pop.set_has_arrow(false);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop
}

/// The space's mark: its emoji, else a dot in its colour (`alpha` 0…1).
pub fn mark_widget(space: &Space, colour: Rgb, alpha: f64, size: i32) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_size_request(size, size);
    l.set_halign(gtk::Align::Center);
    l.set_valign(gtk::Align::Center);
    match super::emoji(space) {
        Some(e) => {
            l.set_text(&e);
            l.set_opacity(alpha.max(0.45));
        }
        None => l.set_markup(&dot_markup(colour, alpha)),
    }
    l
}

/// "●" in a colour, for a label.
pub fn dot_markup(colour: Rgb, alpha: f64) -> String {
    format!(
        "<span foreground=\"{}\" fgalpha=\"{}%\" size=\"8pt\">●</span>",
        colour.hex(),
        (alpha.clamp(0.0, 1.0) * 100.0).round() as u32
    )
}

/// The header row in the column: mark, name, how many tabs on hover.
pub fn header(shell: &Shell) -> gtk::Button {
    let (space, tint, count) = {
        let b = shell.browser();
        let sp = b.current_space().clone();
        let count = b.row(&sp.id).len();
        (sp.clone(), Tint::of(&sp, shell.is_dark()), count)
    };
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mark = mark_widget(&space, tint.swatch(), 1.0, 18);
    mark.add_css_class("space-mark");
    row.append(&mark);
    let name = gtk::Label::new(Some(&space.name));
    name.add_css_class("name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_xalign(0.0);
    row.append(&name);
    let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
    chevron.add_css_class("faint");
    chevron.set_pixel_size(12);
    row.append(&chevron);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    if space.profile.is_some() {
        let lock = gtk::Image::from_icon_name("changes-prevent-symbolic");
        lock.add_css_class("muted");
        lock.set_tooltip_text(Some("Own cookies and sign-ins"));
        lock.set_pixel_size(12);
        row.append(&lock);
    }

    let button = gtk::Button::new();
    button.add_css_class("space-head");
    button.set_child(Some(&row));
    button.set_tooltip_text(Some(&match count {
        0 => "No tabs".to_string(),
        1 => "1 tab".to_string(),
        n => format!("{n} tabs"),
    }));
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "Space {}, menu",
        space.name
    ))]);
    let weak = shell.downgrade();
    let id = space.id.clone();
    button.connect_clicked(move |b| {
        if let Some(s) = weak.upgrade() {
            popover(&s, &id, b).popup();
        }
    });
    // A tab dropped on the header lands at the top of Saved, through the
    // column's own drop target (which shows the marker there).
    button
}

fn gdk_move() -> gtk::gdk::DragAction {
    gtk::gdk::DragAction::MOVE
}

/// The chips at the column's foot (and in the tab row's space menu).
pub fn chips(shell: &Shell) -> gtk::Box {
    let strip = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    strip.add_css_class("space-chips");
    let (spaces, current) = {
        let b = shell.browser();
        (
            b.spaces().cloned().collect::<Vec<_>>(),
            b.current_space_id().to_string(),
        )
    };
    let dark = shell.is_dark();
    let ink = {
        let b = shell.browser();
        Tint::of(b.current_space(), dark).ink()
    };
    // One space: nothing to switch between; only the "+".
    if spaces.len() > 1 {
        for sp in &spaces {
            strip.append(&chip(shell, sp, sp.id == current, dark, ink));
        }
    }
    let add = gtk::Button::from_icon_name("list-add-symbolic");
    add.add_css_class("foot-button");
    add.set_tooltip_text(Some("New Space"));
    add.update_property(&[gtk::accessible::Property::Label("New space")]);
    add.set_action_name(Some("win.new-space"));
    strip.append(&add);
    strip
}

fn chip(shell: &Shell, sp: &Space, current: bool, dark: bool, column_ink: Rgb) -> gtk::Button {
    let tint = Tint::of(sp, dark).off_column();
    let button = gtk::Button::new();
    button.add_css_class("space-chip");
    if current {
        button.add_css_class("current");
    }
    // The current space in colour; the others quiet until you look.
    let label = if current {
        mark_widget(sp, tint.swatch(), 1.0, 16)
    } else {
        mark_widget(sp, column_ink, 0.38, 16)
    };
    button.set_child(Some(&label));
    button.set_tooltip_text(Some(&sp.name));
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "{}{}",
        sp.name,
        if current { ", current space" } else { "" }
    ))]);
    let weak = shell.downgrade();
    let id = sp.id.clone();
    button.connect_clicked(move |b| {
        let Some(s) = weak.upgrade() else { return };
        if s.browser().current_space_id() == id {
            popover(&s, &id, b).popup();
        } else {
            s.switch_space(&id);
        }
    });
    let right = gtk::GestureClick::new();
    right.set_button(3);
    let weak = shell.downgrade();
    let id = sp.id.clone();
    right.connect_pressed(move |g, _, _, _| {
        let (Some(s), Some(w)) = (weak.upgrade(), g.widget()) else {
            return;
        };
        popover(&s, &id, &w).popup();
    });
    button.add_controller(right);

    // Drag a chip to reorder; drop a tab on it to move the tab there.
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk_move());
    let id = sp.id.clone();
    drag.connect_prepare(move |_, _, _| {
        Some(gtk::gdk::ContentProvider::for_value(
            &format!("space:{id}").to_value(),
        ))
    });
    button.add_controller(drag);
    let drop = gtk::DropTarget::new(glib::types::Type::STRING, gdk_move());
    let weak = shell.downgrade();
    let target = sp.id.clone();
    drop.connect_drop(move |_, value, _, _| {
        let (Some(s), Ok(v)) = (weak.upgrade(), value.get::<String>()) else {
            return false;
        };
        if let Some(space) = v.strip_prefix("space:") {
            let to = s.browser().spaces().position(|x| x.id == target);
            if let Some(to) = to {
                s.move_space(space, to);
            }
        } else {
            s.move_to_space(&TabId(v), &target);
        }
        true
    });
    button.add_controller(drop);
    button
}

/// Make a new space and open its editor.
pub fn new_space(shell: &Shell) {
    let name = super::fresh_name(shell.browser().spaces());
    let id = shell.add_space(&name);
    edit_space(shell, &id);
}

/// The editor: name, icon, colour, own cookies.
pub fn edit_space(shell: &Shell, id: &str) {
    editor(shell, id);
}

/// [`edit_space`], returning the dialog (tests close it).
pub fn editor(shell: &Shell, id: &str) -> Option<adw::Dialog> {
    let space = shell.browser().space(id).cloned()?;
    let dialog = adw::Dialog::new();
    dialog.set_title("Edit Space");
    dialog.set_content_width(420);

    let header = adw::HeaderBar::new();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);

    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::new();
    let name = adw::EntryRow::new();
    name.set_title("Name");
    name.set_text(&space.name);
    group.add(&name);
    let icon = adw::EntryRow::new();
    icon.set_title("Emoji (optional)");
    if let Some(i) = space.icon.as_deref().filter(|i| !i.starts_with("sf:")) {
        icon.set_text(i);
    }
    group.add(&icon);
    page.add(&group);

    let colours = adw::PreferencesGroup::new();
    colours.set_title("Colour");
    let swatches = gtk::FlowBox::new();
    swatches.set_selection_mode(gtk::SelectionMode::None);
    swatches.set_max_children_per_line(10);
    swatches.set_min_children_per_line(5);
    swatches.set_column_spacing(6);
    swatches.set_row_spacing(6);
    swatches.set_homogeneous(true);
    let chosen = SpaceColour::of(&space);
    let dark = shell.is_dark();
    let mut buttons = Vec::new();
    let mut swatch_css = String::new();
    for c in SpaceColour::ALL {
        let mut probe = Space::new("x");
        c.apply(&mut probe);
        let tint = Tint::of(&probe, dark).off_column();
        let b = gtk::Button::new();
        b.add_css_class("swatch");
        b.set_tooltip_text(Some(c.name()));
        b.update_property(&[gtk::accessible::Property::Label(c.name())]);
        swatch_css.push_str(&format!(
            ".swatch.c-{} {{ background-color: {}; }}\n",
            c.name().to_lowercase(),
            tint.swatch().hex()
        ));
        b.add_css_class(&format!("c-{}", c.name().to_lowercase()));
        if c == chosen {
            b.add_css_class("chosen");
        }
        swatches.insert(&b, -1);
        buttons.push((c, b));
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&swatch_css);
    let display = WidgetExt::display(shell.window());
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
    );
    let all: Vec<gtk::Button> = buttons.iter().map(|(_, b)| b.clone()).collect();
    for (c, b) in &buttons {
        let weak = shell.downgrade();
        let id = id.to_string();
        let all = all.clone();
        let c = *c;
        b.connect_clicked(move |me| {
            for x in &all {
                x.remove_css_class("chosen");
            }
            me.add_css_class("chosen");
            if let Some(s) = weak.upgrade() {
                s.set_space_colour(&id, c);
            }
        });
    }
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    holder.set_margin_top(4);
    holder.append(&swatches);
    colours.add(&holder);
    page.add(&colours);

    let cookies = adw::PreferencesGroup::new();
    cookies.set_title("Sign-ins");
    let own = adw::SwitchRow::new();
    own.set_title("Own cookies and sign-ins");
    own.set_subtitle("Sites signed in here stay signed out in other spaces.");
    own.set_active(space.profile.is_some());
    cookies.add(&own);
    page.add(&cookies);

    if shell.browser().spaces().count() > 1 {
        let danger = adw::PreferencesGroup::new();
        let delete = gtk::Button::with_label("Delete Space…");
        delete.add_css_class("flat");
        delete.add_css_class("error");
        delete.set_halign(gtk::Align::Center);
        let weak = shell.downgrade();
        let sid = id.to_string();
        let dlg = dialog.downgrade();
        delete.connect_clicked(move |_| {
            if let Some(d) = dlg.upgrade() {
                d.close();
            }
            if let Some(s) = weak.upgrade() {
                ask_delete(&s, &sid);
            }
        });
        danger.add(&delete);
        page.add(&danger);
    }

    content.append(&page);
    view.set_content(Some(&content));
    dialog.set_child(Some(&view));

    // Return in the name or the emoji is done: close, which saves.
    for row in [&name, &icon] {
        let dlg = dialog.downgrade();
        row.connect_entry_activated(move |_| {
            if let Some(d) = dlg.upgrade() {
                d.close();
            }
        });
    }

    // Apply on the way out: name, icon, profile.
    let weak = shell.downgrade();
    let sid = id.to_string();
    let original_profile = space.profile.clone();
    dialog.connect_closed(move |_| {
        gtk::style_context_remove_provider_for_display(&display, &provider);
        let Some(s) = weak.upgrade() else { return };
        s.rename_space(&sid, &name.text());
        let icon_text = icon.text().to_string();
        s.set_space_icon(
            &sid,
            Some(&icon_text)
                .filter(|t| !t.trim().is_empty())
                .map(|t| t.as_str()),
        );
        let wanted = if own.is_active() {
            Some(original_profile.clone().unwrap_or_else(|| {
                let n = name.text().trim().to_string();
                if n.is_empty() { sid.clone() } else { n }
            }))
        } else {
            None
        };
        s.set_space_profile(&sid, wanted.as_deref());
    });
    dialog.present(Some(shell.window()));
    Some(dialog)
}

/// Ask before deleting a space; its tabs go to the neighbouring space
/// unless the user says otherwise.
pub fn ask_delete(shell: &Shell, id: &str) {
    let (name, count, dest) = {
        let b = shell.browser();
        let Some(sp) = b.space(id) else { return };
        if b.spaces().count() < 2 {
            return;
        }
        let dest = shell
            .neighbour_space(id)
            .and_then(|d| b.space(&d).map(|s| s.name.clone()))
            .unwrap_or_default();
        (sp.name.clone(), b.row(id).len(), dest)
    };
    let body = match count {
        0 => "It has no tabs.".to_string(),
        1 => format!("Its tab moves to {dest}."),
        n => format!("Its {n} tabs move to {dest}."),
    };
    let dialog = adw::AlertDialog::new(Some(&format!("Delete {name}?")), Some(&body));
    dialog.add_response("cancel", "Cancel");
    if count > 0 {
        dialog.add_response("close", "Close Its Tabs");
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
    }
    dialog.add_response("keep", if count > 0 { "Delete Space" } else { "Delete" });
    dialog.set_response_appearance(
        "keep",
        if count > 0 {
            adw::ResponseAppearance::Suggested
        } else {
            adw::ResponseAppearance::Destructive
        },
    );
    dialog.set_default_response(Some("keep"));
    dialog.set_close_response("cancel");
    let weak = shell.downgrade();
    let id = id.to_string();
    dialog.connect_response(None, move |_, r| {
        let Some(s) = weak.upgrade() else { return };
        match r {
            "keep" => {
                s.remove_space(&id, true);
            }
            "close" => {
                s.remove_space(&id, false);
            }
            _ => {}
        }
    });
    dialog.present(Some(shell.window()));
}
