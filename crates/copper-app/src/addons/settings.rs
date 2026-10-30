//! Settings › Extensions (Mirrors the Mac's `Fork/Addons/AddonsSettings.swift`).
//!
//! The installed and pending addons — name, version, slot, the cloud they
//! came from — each with "Use in Agent sidebar" (a local opt-out, kept in
//! `state.json`'s `disabled`) and Remove (local, until the cloud changes
//! it again); "Check for addon updates"; and, without a linked cloud, where
//! addons come from. The old note on web extensions stays at the foot.
//!
//! The page keeps its groups and redraws them in place when anything
//! changes ([`redraw`]).

use super::core_glue::{self, Listed};
use crate::features::settings::{button, confirm, group, line};
use crate::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;

/// The page's id in Settings (kept from the page it replaces).
pub const PAGE_ID: &str = "extensions";

struct Drawn {
    page: glib::WeakRef<adw::PreferencesPage>,
    groups: Vec<adw::PreferencesGroup>,
    foot: glib::WeakRef<adw::PreferencesGroup>,
}

thread_local! {
    static DRAWN: RefCell<Option<Drawn>> = const { RefCell::new(None) };
}

/// Build the page (Settings calls this each time it opens).
pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let foot = group(Some("Web extensions"));
    let note = line(
        "Not available on Linux",
        "WebKitGTK, the engine Copper runs on here, has no support for web extensions. Addons are Copper’s own, and come from your Copper Cloud.",
    );
    foot.add(&note);
    DRAWN.with(|d| {
        *d.borrow_mut() = Some(Drawn {
            page: page.downgrade(),
            groups: Vec::new(),
            foot: foot.downgrade(),
        })
    });
    draw(shell);
    page
}

/// Open Settings on this page.
pub fn present(shell: &Shell) {
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name(PAGE_ID);
}

/// Redraw the page if it is open.
pub fn redraw(shell: &Shell) {
    draw(shell);
}

fn draw(shell: &Shell) {
    let Some((page, foot)) = DRAWN.with(|d| {
        let d = d.borrow();
        let d = d.as_ref()?;
        Some((d.page.upgrade()?, d.foot.upgrade()))
    }) else {
        return;
    };
    let old = DRAWN.with(|d| {
        d.borrow_mut()
            .as_mut()
            .map(|d| std::mem::take(&mut d.groups))
            .unwrap_or_default()
    });
    for g in old {
        page.remove(&g);
    }
    if let Some(f) = foot.as_ref().filter(|f| f.parent().is_some()) {
        page.remove(f);
    }

    let mut groups = Vec::new();
    let status = core_glue::status();
    let addons = core_glue::list();

    let intro = group(None);
    intro.set_description(Some(
        "Addons are small apps from your Copper Cloud that fill a part of Copper — today, the Agent sidebar (Ctrl+E). They update without a new Copper.",
    ));
    if !status.linked && addons.iter().all(|a| !a.dev) {
        let r = line(
            "No Copper Cloud linked",
            "Addons come from your Copper Cloud. Link one in Settings › Cloud.",
        );
        let b = button("Open Cloud", false);
        let weak = shell.downgrade();
        b.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                let dialog = crate::window::settings::dialog(&s);
                dialog.set_visible_page_name("cloud");
            }
        });
        r.add_suffix(&b);
        intro.add(&r);
    }
    groups.push(intro);

    if addons.is_empty() {
        if status.linked {
            let g = group(Some("Agent sidebar"));
            g.add(&line(
                "No addon",
                "Your Copper Cloud hasn’t given you one yet, so the sidebar shows Copper’s own placeholder.",
            ));
            groups.push(g);
        }
    } else {
        for a in &addons {
            groups.push(addon_group(shell, a));
        }
    }

    if status.linked || addons.iter().any(|a| !a.dev) {
        let g = group(Some("Updates"));
        let detail = if status.checking {
            "Checking…".to_string()
        } else if let Some(e) = &status.error {
            format!("Couldn’t check just now · {e}")
        } else if let Some(t) = &status.checked {
            format!("Last checked {t}")
        } else {
            "Copper looks when it starts and every six hours".to_string()
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
        g.add(&r);
        groups.push(g);
    }

    for g in &groups {
        page.add(g);
    }
    if let Some(f) = &foot {
        page.add(f);
    }
    DRAWN.with(|d| {
        if let Some(d) = d.borrow_mut().as_mut() {
            d.groups = groups;
        }
    });
}

/// One addon: what it is, where it came from, and what can be done here.
fn addon_group(shell: &Shell, a: &Listed) -> adw::PreferencesGroup {
    let g = group(Some(&a.name));
    let mut desc = vec![format!("Version {}", a.version), a.slot_name.clone()];
    if a.dev {
        desc.push("from a folder on this computer".to_string());
    } else if !a.source.is_empty() {
        desc.push(format!("from {}", a.source));
    }
    g.set_description(Some(&desc.join(" · ")));

    if let Some(v) = &a.pending {
        let r = line(
            "Update available",
            &format!("{} {v} for the {}", a.name, a.slot_name),
        );
        let b = button("Install", true);
        let weak = shell.downgrade();
        b.connect_clicked(move |b| {
            b.set_sensitive(false);
            if let Some(s) = weak.upgrade() {
                super::updates::install(&s);
            }
        });
        r.add_suffix(&b);
        g.add(&r);
    }

    if a.installed || a.dev {
        let r = adw::SwitchRow::new();
        r.set_title(&format!("Use in {}", a.slot_name));
        r.set_subtitle(if a.enabled {
            "On: the sidebar shows this addon"
        } else {
            "Off: the sidebar shows Copper’s placeholder"
        });
        r.set_active(a.enabled);
        r.set_sensitive(!a.dev);
        let id = a.id.clone();
        let weak = shell.downgrade();
        r.connect_active_notify(move |r| {
            if let Some(s) = weak.upgrade() {
                core_glue::set_enabled(&s, &id, r.is_active());
            }
        });
        g.add(&r);
    }

    if !a.dev {
        let r = line(
            "Remove",
            "From this computer, until your Copper Cloud changes it again",
        );
        // Plain, as the page's other buttons; the dialog is the red one.
        let b = button("Remove…", false);
        let (id, name) = (a.id.clone(), a.name.clone());
        let weak = shell.downgrade();
        b.connect_clicked(move |_| {
            let Some(s) = weak.upgrade() else { return };
            let id = id.clone();
            confirm(
                &s,
                &format!("Remove {name}?"),
                "It leaves this computer until your Copper Cloud changes it again. What it saved here stays.",
                "Remove",
                move |s| core_glue::remove(s, &id),
            );
        });
        r.add_suffix(&b);
        g.add(&r);
    }
    g
}

/// The page is built (tests).
#[allow(dead_code)]
pub fn is_open() -> bool {
    DRAWN.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|d| d.page.upgrade().is_some())
    })
}

/// The widget for screenshots: the page if open.
#[allow(dead_code)]
pub fn widget() -> Option<gtk::Widget> {
    DRAWN.with(|d| {
        d.borrow()
            .as_ref()
            .and_then(|d| d.page.upgrade())
            .map(|p| p.upcast())
    })
}
