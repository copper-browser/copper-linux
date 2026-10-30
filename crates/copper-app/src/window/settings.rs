//! Settings: the shell's own page (appearance, tabs, search), then every
//! page a lane registered ([`crate::actions::register_settings_page`]), in
//! the Mac's rail order ([`ORDER`]). It opens at the page it was last on
//! (`settings.page`, the Mac's page ids); adw's search finds rows by title.

use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::address::ENGINES;
use copper_core::settings::{Glyph, Look, TabsPosition, keys};
use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

/// Open Settings over the window.
pub fn present(shell: &Shell) {
    dialog(shell);
}

/// [`present`], returning the dialog (tests close it).
pub fn dialog(shell: &Shell) -> adw::PreferencesDialog {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    dialog.set_search_enabled(true);
    if !shell
        .registry()
        .borrow()
        .settings_pages
        .iter()
        .any(|p| p.id == "general")
    {
        dialog.add(&general(shell));
    }
    // The page Settings was last on; one Linux lacks opens General (Mac:
    // `SettingsPanel.Page(rawValue:) ?? .general`).
    let last = shell
        .settings()
        .string(keys::SETTINGS_PAGE)
        .map(str::to_string);
    let mut pages = shell.registry().borrow().settings_pages.clone();
    pages.sort_by_key(|p| rank(&p.id));
    let names: Vec<&str> = pages.iter().map(|p| p.id.as_str()).collect();
    let open_at = opening(last.as_deref(), &names).to_string();
    for p in pages {
        let page = (p.build)(shell);
        page.set_title(&p.title);
        page.set_icon_name(Some(&p.icon));
        page.set_name(Some(&p.id));
        dialog.add(&page);
    }
    dialog.set_visible_page_name(&open_at);
    let weak = shell.downgrade();
    dialog.connect_visible_page_name_notify(move |d| {
        if let (Some(s), Some(name)) = (weak.upgrade(), d.visible_page_name()) {
            let name = name.to_string();
            if s.settings().string(keys::SETTINGS_PAGE) != Some(name.as_str()) {
                s.update_settings(|st| st.set(keys::SETTINGS_PAGE, name));
            }
        }
    });
    dialog.present(Some(shell.window()));
    dialog
}

/// The page to open at: the stored one if Settings has it (`names`, the
/// registered pages), else General.
fn opening<'a>(stored: Option<&'a str>, names: &[&str]) -> &'a str {
    match stored {
        Some(n) if names.contains(&n) => n,
        _ => "general",
    }
}

/// The Mac's rail (`SettingsPanel.Page`, Copper#115): Browsing, AI & agents,
/// Data & privacy, Copper. Pages Linux lacks are simply not registered.
pub const ORDER: [&str; 10] = [
    "general",
    "tabs",
    "spaces",
    "extensions",
    "intelligence",
    "agents",
    "passwords",
    "privacy",
    "cloud",
    "about",
];

/// Where page `id` sits in [`ORDER`]; an unknown page goes last.
fn rank(id: &str) -> usize {
    ORDER.iter().position(|o| *o == id).unwrap_or(ORDER.len())
}

fn combo(title: &str, items: &[&str], selected: usize) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    row.set_title(title);
    row.set_model(Some(&gtk::StringList::new(items)));
    row.set_selected(selected as u32);
    row
}

fn general(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_name(Some("general"));
    page.set_title("General");
    page.set_icon_name(Some("preferences-system-symbolic"));

    let look = adw::PreferencesGroup::new();
    let appearance = combo(
        "Appearance",
        &["Light", "Dark", "Match System"],
        match shell.look() {
            Look::Light => 0,
            Look::Dark => 1,
            Look::System => 2,
        },
    );
    let weak = shell.downgrade();
    appearance.connect_selected_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            s.set_look(match r.selected() {
                1 => Look::Dark,
                2 => Look::System,
                _ => Look::Light,
            });
        }
    });
    look.add(&appearance);
    page.add(&look);

    let tabs = adw::PreferencesGroup::new();
    tabs.set_title("Tabs");
    let position = combo(
        "Position",
        &["On the left", "On top"],
        match shell.tabs_position() {
            TabsPosition::Left => 0,
            TabsPosition::Top => 1,
        },
    );
    let weak = shell.downgrade();
    position.connect_selected_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            s.set_tabs_position(if r.selected() == 1 {
                TabsPosition::Top
            } else {
                TabsPosition::Left
            });
        }
    });
    tabs.add(&position);
    let icons = crate::window::tabs::shows_icons(shell);
    let glyph = combo(
        "Beside titles",
        &["Site icons", "Letters"],
        if icons { 0 } else { 1 },
    );
    let weak = shell.downgrade();
    glyph.connect_selected_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            let g = if r.selected() == 1 {
                Glyph::Letters
            } else {
                Glyph::Icons
            };
            s.update_settings(|st| st.set_glyph(g));
            s.ui().refresh(&s, &crate::window::Refresh::all());
        }
    });
    tabs.add(&glyph);
    let sleep = adw::SwitchRow::new();
    sleep.set_title("Put idle tabs to sleep");
    sleep.set_subtitle("A sleeping tab frees its memory and loads again when you return to it.");
    sleep.set_active(shell.settings().sleeps_tabs());
    let weak = shell.downgrade();
    sleep.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            let on = r.is_active();
            s.update_settings(|st| st.set(keys::TABS_SLEEP, on));
        }
    });
    tabs.add(&sleep);
    page.add(&tabs);

    let search = adw::PreferencesGroup::new();
    search.set_title("Search");
    let names: Vec<&str> = ENGINES.iter().map(|e| e.name).collect();
    let current = shell.settings().search_engine().id;
    let at = ENGINES.iter().position(|e| e.id == current).unwrap_or(0);
    let engine = combo("Search with", &names, at);
    let weak = shell.downgrade();
    engine.connect_selected_notify(move |r| {
        if let Some(s) = weak.upgrade()
            && let Some(e) = ENGINES.get(r.selected() as usize)
        {
            let id = e.id;
            s.update_settings(|st| st.set(keys::SEARCH_ENGINE, id));
        }
    });
    search.add(&engine);
    page.add(&search);
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_at_the_last_page() {
        let names = ["general", "tabs", "cloud"];
        assert_eq!(opening(Some("cloud"), &names), "cloud");
        assert_eq!(opening(Some("spaces"), &names), "general");
        assert_eq!(opening(None, &names), "general");
    }

    #[test]
    fn pages_follow_the_mac_rail() {
        let mut ids = [
            "about",
            "cloud",
            "x",
            "privacy",
            "agents",
            "general",
            "extensions",
        ];
        ids.sort_by_key(|id| rank(id));
        assert_eq!(
            ids,
            [
                "general",
                "extensions",
                "agents",
                "privacy",
                "cloud",
                "about",
                "x"
            ]
        );
    }
}
