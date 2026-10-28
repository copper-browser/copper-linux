//! Settings pages (Mac: `Settings.swift`, `Fork/SettingsFork.swift`,
//! `Fork/PrivacySettings.swift`): General, Tabs (with the Calendar pin's
//! calendars), Appearance, Privacy, Extensions, Updates. Every key is the
//! Mac's (copper-core `settings`).
//! The shell skips its own General page because this lane registers one.

pub mod calendar;
pub mod general;
pub mod privacy;

use crate::actions::{SettingsPage, register_settings_page};
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::settings::{Glyph, Look, TabSwitching, TabsPosition, keys};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

pub fn register(shell: &Shell) {
    page(
        shell,
        "general",
        "General",
        "preferences-system-symbolic",
        general::page,
    );
    page(shell, "tabs", "Tabs", "view-dual-symbolic", tabs);
    page(
        shell,
        "appearance",
        "Appearance",
        "applications-graphics-symbolic",
        appearance,
    );
    page(
        shell,
        "privacy",
        "Privacy",
        "security-high-symbolic",
        privacy::page,
    );
    page(
        shell,
        "extensions",
        "Addons", // Addons: the page is Settings › Addons (its id stays `extensions`)
        "application-x-addon-symbolic",
        extensions,
    );
    page(
        shell,
        "updates",
        "Updates",
        "software-update-available-symbolic",
        super::updates::page,
    );
}

fn page(
    shell: &Shell,
    id: &str,
    title: &str,
    icon: &str,
    build: fn(&Shell) -> adw::PreferencesPage,
) {
    register_settings_page(
        shell,
        SettingsPage {
            id: id.into(),
            title: title.into(),
            icon: icon.into(),
            build: Rc::new(build),
        },
    );
}

/// A group with an optional title.
pub fn group(title: Option<&str>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    if let Some(t) = title {
        g.set_title(t);
    }
    g
}

/// A plain line: what it is, a sentence, a control at the end.
pub fn line(title: &str, subtitle: &str) -> adw::ActionRow {
    let r = adw::ActionRow::new();
    r.set_title(title);
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.set_subtitle_lines(3);
    r
}

/// A switch line.
pub fn switch(
    shell: &Shell,
    title: &str,
    subtitle: &str,
    on: bool,
    f: impl Fn(&Shell, bool) + 'static,
) -> adw::SwitchRow {
    let r = adw::SwitchRow::new();
    r.set_title(title);
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.set_active(on);
    let weak = shell.downgrade();
    r.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            f(&s, r.is_active());
        }
    });
    r
}

/// A choice from a short list.
pub fn combo(
    shell: &Shell,
    title: &str,
    subtitle: &str,
    items: &[&str],
    selected: usize,
    f: impl Fn(&Shell, usize) + 'static,
) -> adw::ComboRow {
    let r = adw::ComboRow::new();
    r.set_title(title);
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.set_model(Some(&gtk::StringList::new(items)));
    r.set_selected(selected as u32);
    let weak = shell.downgrade();
    r.connect_selected_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            f(&s, r.selected() as usize);
        }
    });
    r
}

/// Two or three linked toggles at the end of a line (one is always on).
pub fn segmented(
    shell: &Shell,
    options: &[&str],
    selected: usize,
    f: impl Fn(&Shell, usize) + 'static,
) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("linked");
    b.set_valign(gtk::Align::Center);
    let f = Rc::new(f);
    let mut first: Option<gtk::ToggleButton> = None;
    for (i, o) in options.iter().enumerate() {
        let t = gtk::ToggleButton::with_label(o);
        if let Some(g) = &first {
            t.set_group(Some(g));
        } else {
            first = Some(t.clone());
        }
        t.set_active(i == selected);
        let weak = shell.downgrade();
        let f = f.clone();
        t.connect_toggled(move |t| {
            if t.is_active()
                && let Some(s) = weak.upgrade()
            {
                f(&s, i);
            }
        });
        b.append(&t);
    }
    b
}

/// A button at the end of a line.
pub fn button(label: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_valign(gtk::Align::Center);
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

/// Ask before something that cannot be undone.
pub fn confirm(shell: &Shell, heading: &str, body: &str, act: &str, f: impl Fn(&Shell) + 'static) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_responses(&[("cancel", "Cancel"), ("go", act)]);
    d.set_response_appearance("go", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    let weak = shell.downgrade();
    d.connect_response(None, move |_, r| {
        if r == "go"
            && let Some(s) = weak.upgrade()
        {
            f(&s);
        }
    });
    let parent = shell
        .window()
        .visible_dialog()
        .map(|d| d.upcast::<gtk::Widget>())
        .unwrap_or_else(|| shell.window().clone().upcast());
    d.present(Some(&parent));
}

fn tabs(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let showing = group(Some("Showing tabs"));
    let icons = crate::window::tabs::shows_icons(shell);
    showing.add(&combo(
        shell,
        "Tabs show",
        "Beside the title, and on a pinned square",
        &["Site icons", "Letters"],
        if icons { 0 } else { 1 },
        |s, i| {
            let g = if i == 1 { Glyph::Letters } else { Glyph::Icons };
            s.update_settings(|st| st.set_glyph(g));
            s.ui().refresh(s, &crate::window::Refresh::all());
        },
    ));
    page.add(&showing);

    let switching = group(Some("Switching"));
    let current = shell.settings().tab_switching();
    switching.add(&combo(
        shell,
        "Ctrl+Tab switches to",
        "Follow the row, or go back through your most recent tabs",
        &["The next tab in the row", "The tab you were on before"],
        if current == TabSwitching::Recent {
            1
        } else {
            0
        },
        |s, i| {
            let v = if i == 1 {
                TabSwitching::Recent
            } else {
                TabSwitching::Row
            };
            s.update_settings(|st| st.set_tab_switching(v));
        },
    ));
    page.add(&switching);

    let leave = group(Some("Tabs you leave"));
    leave.add(&switch(
        shell,
        "Sleep tabs you aren’t using",
        "After half an hour away they come back where you left them. Pinned tabs, sound, calls and anything typed stay awake.",
        shell.settings().sleeps_tabs(),
        |s, on| s.update_settings(|st| st.set(keys::TABS_SLEEP, on)),
    ));
    page.add(&leave);
    page.add(&calendar::group(shell));
    page
}

fn appearance(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let g = group(None);
    let look = line(
        "Appearance",
        "Light, dark, or whatever the system is doing — pages follow it too",
    );
    let at = match shell.look() {
        Look::Light => 0,
        Look::Dark => 1,
        Look::System => 2,
    };
    look.add_suffix(&segmented(
        shell,
        &["Light", "Dark", "System"],
        at,
        |s, i| {
            s.set_look(match i {
                1 => Look::Dark,
                2 => Look::System,
                _ => Look::Light,
            })
        },
    ));
    g.add(&look);
    let tabs = line(
        "Tabs",
        "Down the left in a column, or across the top in a row",
    );
    let at = match shell.tabs_position() {
        TabsPosition::Left => 0,
        TabsPosition::Top => 1,
    };
    tabs.add_suffix(&segmented(shell, &["Left", "Top"], at, |s, i| {
        s.set_tabs_position(if i == 1 {
            TabsPosition::Top
        } else {
            TabsPosition::Left
        })
    }));
    g.add(&tabs);
    page.add(&g);
    page
}

fn extensions(shell: &Shell) -> adw::PreferencesPage {
    crate::addons::settings::page(shell) // Addons: Settings › Addons (the web-extensions note is its foot)
}
