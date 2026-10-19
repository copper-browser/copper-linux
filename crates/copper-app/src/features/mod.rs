//! The features lane: the ad blocker, hide anything, reading mode, hard
//! reload and the inspector's element picker, the History / Bookmarks /
//! Downloads panels, the General / Tabs / Appearance / Privacy / Extensions /
//! Updates settings pages, and the update check.
//!
//! Everything hangs off two hooks: [`register`] (once, from
//! `actions::register_lanes`) and [`page_created`] (every tab's page, before
//! its first load — pages made later included).

pub mod curtain;
pub mod panels;
pub mod reader;
pub mod settings;
pub mod shield;
pub mod updates;

pub(crate) use copper_shell_core::{format, util};
mod selftest;
mod ui;

use crate::actions::{Command, register_action, register_command, register_menu_item};
use crate::platform::Gtk;
use crate::shell::Shell;
use copper_core::TabId;
use copper_engine::{LoadState, PageEvent, PageView};
use copper_engine_webkitgtk::WebKitPage;
use glib::object::ObjectExt;

/// Called once from `actions::register_lanes`.
pub fn register(shell: &Shell) {
    ui::install_css();
    shield::register(shell);
    curtain::register(shell);
    panels::register(shell);
    settings::register(shell);
    updates::register(shell);
    page_actions(shell);
    // Every page made from now on; then the pages that exist already (none
    // in practice: the session's first page is made after the lanes
    // register).
    shell.on_page_created(page_created);
    let pages: Vec<(TabId, WebKitPage)> = shell
        .pages_map()
        .borrow()
        .iter()
        .map(|(id, slot)| (id.clone(), slot.page.clone()))
        .collect();
    for (id, page) in pages {
        page_created(shell, &id, &page);
    }
    selftest::maybe_start(shell);
}

/// The downloads door, for the column's foot and the end of the tab row
/// (Mac `DownloadsDoor`, `docs/downloads.md`): hidden until something
/// downloads, then a ring, a check, a dot. Click for the Downloads panel.
pub fn door(shell: &Shell) -> gtk4::Widget {
    panels::downloads::door(shell)
}

/// A tab's page was just made, before its first load: it gets the block
/// list (now, or when the list is ready), the hidden-element style sheets
/// and the picker, and its events are watched for the picker's messages and
/// for leaving reading mode.
pub fn page_created(shell: &Shell, id: &TabId, page: &WebKitPage) {
    shield::protect(shell, page);
    curtain::dress(shell, id, page);
    reader::forget(id);
    let mut events = page.events();
    let weak = shell.downgrade();
    let tab = id.clone();
    // Weak: holding the view would keep a closed tab's page alive.
    let mine = page.widget().downgrade();
    let page_id = page.page_id();
    glib::spawn_future_local(async move {
        while let Some(ev) = events.next().await {
            let Some(shell) = weak.upgrade() else { break };
            // The tab may have a newer page (after a crash or a sleep);
            // this one's events no longer speak for it.
            let current = shell.slot(&tab).map(|s| s.page.widget());
            let Some(view) = mine.upgrade() else { break };
            if current.as_ref() != Some(&view) {
                break;
            }
            match ev {
                PageEvent::ScriptMessage { name, body } if name == curtain::HANDLER => {
                    curtain::message(&shell, &tab, &body);
                }
                PageEvent::Load(LoadState::Started { .. }) => reader::forget(&tab),
                PageEvent::Load(LoadState::Committed { .. }) => {
                    curtain::navigated(&shell, &tab);
                    panels::page_moved(&shell, &tab);
                }
                _ => {}
            }
        }
        // The page went (closed, slept, crashed and replaced).
        curtain::page_gone::<Gtk>(page_id);
        if let Some(shell) = weak.upgrade()
            && shell.slot(&tab).is_none()
        {
            reader::forget(&tab);
        }
    });
}

/// Hard reload (Ctrl+Shift+R), reading mode (Ctrl+Alt+R), the inspector's
/// element picker (Ctrl+Shift+I). The first and last replace the shell's
/// plain versions with the Mac's (a line saying what happened; the picker
/// armed rather than only the inspector shown).
fn page_actions(shell: &Shell) {
    register_action(shell, "hard-reload", &[], hard_reload);
    register_action(shell, "inspector", &[], inspect_element);
    register_action(shell, "reader", &["<Control><Alt>r"], reader::toggle);
    let has_page = |s: &Shell| s.active_id().is_some();
    register_command(
        shell,
        Command::new(
            "hard-reload",
            "Hard Reload",
            "view-refresh-symbolic",
            hard_reload,
        )
        .shortcut("Ctrl+Shift+R")
        .when(has_page),
    );
    register_command(
        shell,
        Command::new(
            "reader",
            "Reading Mode",
            "view-reader-symbolic",
            reader::toggle,
        )
        .shortcut("Ctrl+Alt+R")
        .when(has_page),
    );
    register_command(
        shell,
        Command::new(
            "inspect",
            "Inspect Element",
            "system-search-symbolic",
            inspect_element,
        )
        .shortcut("Ctrl+Shift+I")
        .when(has_page),
    );
    register_menu_item(shell, "Reading Mode", "win.reader");
}

/// Ctrl+Shift+R: this site's caches go, then the page loads from the network.
fn hard_reload(shell: &Shell) {
    if shell.active_id().is_none() {
        return;
    }
    shell.toast("Cache cleared — reloading");
    shell.hard_reload_active();
}

/// Ctrl+Shift+I: the Web Inspector, ready to pick an element. A sleeping
/// tab is woken first (inspecting it would otherwise open the inspector on
/// an empty page).
///
/// The Mac arms the picker through WebKit's private `_inspector`; WebKitGTK
/// has no way in (its inspector front end is a bare `WebKitWebViewBase`,
/// with no script or input API), so the inspector opens and the line says
/// how to pick: its own Ctrl+Shift+C, or Inspect Element on the page's
/// right-click menu — the Mac's own fallback words.
fn inspect_element(shell: &Shell) {
    let Some(id) = shell.active_id() else { return };
    let Some(page) = shell.ensure_page(&id) else {
        return;
    };
    page.show_inspector();
    shell.toast("Pick with Ctrl+Shift+C, or Right-click › Inspect Element");
}
