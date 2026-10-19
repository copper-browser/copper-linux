//! Addon updates in Copper's own update UI (Mirrors the Mac's
//! `Fork/Addons/AddonUpdates.swift`, which feeds `UpdatePill`).
//!
//! The core's sync downloads, verifies and stages a new addon or version
//! before anything shows. Here it becomes:
//!
//! * an **Addons** group on Settings › Updates — "Update available —
//!   <Addon name> <version> for the Agent sidebar" with **Install** — that
//!   is there in the Flatpak too (Copper's own update rows are not);
//! * a toast, once per pending version, with **Install**;
//! * **Install**: the staged bundle is unpacked and made current, and the
//!   slot's view reloads with it. No relaunch.
//!
//! Settings › Updates' own "Check now" also asks the cloud for addons.

use super::core_glue;
use crate::features::settings::{button, group, line};
use crate::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;

/// The settings key holding the last pending version announced
/// (`<id>@<version>`), so the toast is shown once per version.
pub const ANNOUNCED_KEY: &str = "addons.announcedUpdate";

struct View {
    group: glib::WeakRef<adw::PreferencesGroup>,
    rows: RefCell<Vec<gtk::Widget>>,
}

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    /// The "Update available" toast while it shows.
    static TOAST: RefCell<Option<glib::WeakRef<adw::Toast>>> = const { RefCell::new(None) };
}

/// Take the "Update available" toast down (installed, or gone).
fn dismiss_toast() {
    if let Some(t) = TOAST
        .with(|t| t.borrow_mut().take())
        .and_then(|w| w.upgrade())
    {
        t.dismiss();
    }
}

/// Settings › Updates: add the Addons group to `page`.
pub fn attach(shell: &Shell, page: &adw::PreferencesPage) {
    let g = group(Some("Addons"));
    page.add(&g);
    VIEW.with(|v| {
        *v.borrow_mut() = Some(View {
            group: g.downgrade(),
            rows: RefCell::default(),
        })
    });
    draw(shell);
}

/// Settings › Updates' "Check now" (and its own schedule) ran: ask the
/// cloud for addons too.
pub fn app_checked() {
    core_glue::check_now();
}

/// Redraw the Addons group on Settings › Updates, if it is open.
pub fn draw(shell: &Shell) {
    VIEW.with(|v| {
        let v = v.borrow();
        let Some(v) = v.as_ref() else { return };
        let Some(g) = v.group.upgrade() else { return };
        for r in v.rows.borrow_mut().drain(..) {
            if let Some(row) = r.downcast_ref::<adw::PreferencesRow>() {
                g.remove(row);
            }
        }
        let mut rows: Vec<gtk::Widget> = Vec::new();
        let status = core_glue::status();
        if let Some(p) = core_glue::pending() {
            let r = line(
                "Update available",
                &format!("{} {} for the {}", p.name, p.version, p.slot_name),
            );
            let b = button("Install", true);
            let weak = shell.downgrade();
            b.connect_clicked(move |b| {
                b.set_sensitive(false);
                if let Some(s) = weak.upgrade() {
                    install(&s);
                }
            });
            r.add_suffix(&b);
            rows.push(r.upcast());
        }
        let installed: Vec<String> = core_glue::list()
            .iter()
            .filter(|a| a.installed)
            .map(|a| format!("{} {}", a.name, a.version))
            .collect();
        let detail = if status.checking {
            "Checking…".to_string()
        } else if let Some(e) = &status.error {
            format!("Couldn’t check just now · {e}")
        } else if !status.linked {
            "Addons come from your Copper Cloud. Link one in Settings › Cloud.".to_string()
        } else if installed.is_empty() {
            "No addons from your Copper Cloud".to_string()
        } else {
            format!("{} · up to date", installed.join(", "))
        };
        let r = line("Check for addon updates", &detail);
        let b = button(
            if status.checking {
                "Checking…"
            } else {
                "Check now"
            },
            false,
        );
        b.set_sensitive(status.linked && !status.checking);
        b.connect_clicked(|_| core_glue::check_now());
        r.add_suffix(&b);
        rows.push(r.upcast());
        for r in &rows {
            if let Some(row) = r.downcast_ref::<adw::PreferencesRow>() {
                g.add(row);
            }
        }
        *v.rows.borrow_mut() = rows;
    });
}

/// The addons changed: redraw, and announce a newly staged version once.
pub fn changed(shell: &Shell) {
    draw(shell);
    let Some(p) = core_glue::pending() else {
        return;
    };
    let key = format!("{}@{}", p.id, p.version);
    if shell.settings().string(ANNOUNCED_KEY) == Some(key.as_str()) {
        return;
    }
    shell.update_settings(|s| s.set(ANNOUNCED_KEY, key.clone()));
    let t = adw::Toast::new(&format!(
        "Update available — {} {} for the {}",
        p.name, p.version, p.slot_name
    ));
    t.set_button_label(Some("Install"));
    t.set_timeout(10);
    let weak = shell.downgrade();
    t.connect_button_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            install(&s);
        }
    });
    dismiss_toast();
    TOAST.with(|w| *w.borrow_mut() = Some(t.downgrade()));
    shell.ui().toasts.add_toast(t);
}

/// Install the staged update; the slot reloads with it.
pub fn install(shell: &Shell) {
    dismiss_toast();
    let weak = shell.downgrade();
    core_glue::install_pending(shell, move |result| {
        let Some(s) = weak.upgrade() else { return };
        match result {
            Ok(done) if !done.is_empty() => {
                s.toast(&format!("{} is installed", done.join(", ")));
                super::changed(&s);
            }
            Ok(_) => super::changed(&s),
            Err(e) => {
                tracing::warn!(error = %e, "addon install");
                s.toast("Couldn’t install the addon update");
                super::changed(&s);
            }
        }
    });
}
