//! The side panels: History (Ctrl+Y), Bookmarks (Ctrl+Shift+O; Ctrl+D keeps
//! the page in front), Downloads (Ctrl+Shift+Y). Each is searchable where
//! the Mac's is, built once, and redrawn whenever it comes on screen.

pub mod bookmarks;
pub mod downloads;
pub mod history;

use crate::actions::{Command, Panel, register_action, register_command, register_panel};
use crate::shell::Shell;
use copper_core::TabId;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

type Refresh = Rc<dyn Fn(&Shell)>;

thread_local! {
    /// Each built panel's root and redraw.
    static BUILT: RefCell<HashMap<&'static str, (glib::WeakRef<gtk::Widget>, Refresh)>> =
        RefCell::default();
}

pub fn register(shell: &Shell) {
    panel(
        shell,
        "history",
        "History",
        "document-open-recent-symbolic",
        history::build,
    );
    panel(
        shell,
        "bookmarks",
        "Bookmarks",
        "user-bookmarks-symbolic",
        bookmarks::build,
    );
    panel(
        shell,
        "downloads",
        "Downloads",
        "folder-download-symbolic",
        downloads::build,
    );

    register_action(shell, "show-history", &["<Control>y"], |s| {
        toggle(s, "history")
    });
    register_action(shell, "show-bookmarks", &["<Control><Shift>o"], |s| {
        toggle(s, "bookmarks")
    });
    register_action(shell, "show-downloads", &["<Control><Shift>y"], |s| {
        toggle(s, "downloads")
    });
    register_action(
        shell,
        "bookmark-page",
        &["<Control>d"],
        bookmarks::bookmark_current,
    );
    register_command(
        shell,
        Command::new(
            "bookmark",
            "Bookmark This Page",
            "bookmark-new-symbolic",
            bookmarks::bookmark_current,
        )
        .shortcut("Ctrl+D")
        .when(|s| s.active_id().is_some()),
    );
    // The palette's rows for the panels carry their shortcuts.
    for (id, name, icon, keys) in [
        (
            "history",
            "History",
            "document-open-recent-symbolic",
            "Ctrl+Y",
        ),
        (
            "bookmarks",
            "Bookmarks",
            "user-bookmarks-symbolic",
            "Ctrl+Shift+O",
        ),
        (
            "downloads",
            "Downloads",
            "folder-download-symbolic",
            "Ctrl+Shift+Y",
        ),
    ] {
        register_command(
            shell,
            Command::new(&format!("panel-{id}"), name, icon, move |s| toggle(s, id)).shortcut(keys),
        );
    }
    downloads::watch(shell);
}

fn panel(
    shell: &Shell,
    id: &'static str,
    title: &str,
    icon: &str,
    build: fn(&Shell) -> (gtk::Widget, Refresh),
) {
    register_panel(
        shell,
        Panel {
            id: id.into(),
            title: title.into(),
            icon: icon.into(),
            build: Rc::new(move |s| {
                let (w, refresh) = build(s);
                let weak = s.downgrade();
                let r = refresh.clone();
                w.connect_map(move |_| {
                    if let Some(s) = weak.upgrade() {
                        r(&s);
                    }
                });
                BUILT.with(|b| b.borrow_mut().insert(id, (w.downgrade(), refresh)));
                w
            }),
        },
    );
}

/// Show or hide a panel.
pub fn toggle(shell: &Shell, id: &str) {
    shell.ui().stage.toggle_panel(shell, id);
}

/// Show a panel (and redraw it).
pub fn show(shell: &Shell, id: &str) {
    shell.ui().stage.show_panel(shell, id);
    refresh(shell, id);
}

/// Put a panel away.
pub fn close(shell: &Shell) {
    shell.ui().stage.hide_panel();
}

/// Whether panel `id` is on screen.
pub fn showing(id: &str) -> bool {
    BUILT.with(|b| {
        b.borrow()
            .get(id)
            .and_then(|(w, _)| w.upgrade())
            .is_some_and(|w| w.is_mapped() && w.is_child_visible() && revealed(&w))
    })
}

/// The panel's revealer is open (a hidden panel stays mapped while it slides).
fn revealed(w: &gtk::Widget) -> bool {
    let mut p = w.parent();
    while let Some(x) = p {
        if let Some(r) = x.downcast_ref::<gtk::Revealer>() {
            return r.reveals_child();
        }
        p = x.parent();
    }
    true
}

/// Each built panel's minimum and natural width (tests: none may push the
/// side panel wider than its 360 px).
pub fn widths() -> Vec<(&'static str, i32, i32)> {
    BUILT.with(|b| {
        b.borrow()
            .iter()
            .filter_map(|(id, (w, _))| {
                let w = w.upgrade()?;
                let (min, nat, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
                Some((*id, min, nat))
            })
            .collect()
    })
}

/// The Downloads panel is on screen (the toasts stay quiet then).
pub fn downloads_showing() -> bool {
    showing("downloads")
}

/// Redraw panel `id` if it is on screen.
pub fn refresh(shell: &Shell, id: &str) {
    let r = BUILT.with(|b| b.borrow().get(id).map(|(_, r)| r.clone()));
    if let Some(r) = r
        && showing(id)
    {
        r(shell);
    }
}

/// The page in front moved on: History has a new line.
pub fn page_moved(shell: &Shell, tab: &TabId) {
    if shell.active_id().as_ref() == Some(tab) {
        // History records the visit in the same turn; redraw after it.
        let weak = shell.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(s) = weak.upgrade() {
                refresh(&s, "history");
            }
        });
    }
}

/// Escape inside a panel puts it away.
pub fn escape_closes(shell: &Shell, w: &impl IsA<gtk::Widget>) {
    let keys = gtk::EventControllerKey::new();
    let weak = shell.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape
            && let Some(s) = weak.upgrade()
        {
            close(&s);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    w.add_controller(keys);
}

/// Open `url` in the tab in front (a new tab when there is none).
pub fn go(shell: &Shell, url: &str) {
    match shell.active_id() {
        Some(id) => shell.navigate(&id, url),
        None => {
            shell.open(
                url,
                crate::shell::OpenTab {
                    foreground: true,
                    ..Default::default()
                },
            );
        }
    }
}

/// A site's mark: its kept icon, or its letter.
pub fn site_mark(shell: &Shell, url: &str, letter_from: &str) -> gtk::Widget {
    if let Some(tex) = shell.cached_icon(url).or_else(|| shell.open_icon(url)) {
        return crate::window::tabs::icon_image(&tex, 16).upcast();
    }
    crate::window::letter_chip(letter_from, 16).upcast()
}
