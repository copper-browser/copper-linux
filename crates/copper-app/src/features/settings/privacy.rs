//! Settings › Privacy (Mac: `Settings.swift` `privacy`,
//! `Fork/PrivacySettings.swift`): the ad blocker (everywhere, and for the
//! site in front), and clearing browsing data — history and sign-ins ask
//! first; the cache is harmless and goes at once. Cookies and caches go
//! from every jar: the shared one and each space profile's.

use super::{button, confirm, group, line, switch};
use crate::features::{curtain, shield};
use crate::platform::Gtk;
use crate::shell::Shell;
use crate::shell::pages::profile_id;
use copper_engine::{Engine, Profile, WebsiteDataKinds};
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();

    let blocker = group(Some("Ad blocker"));
    let trouble = shield::trouble::<Gtk>();
    blocker.add(&switch(
        shell,
        "Block ads and trackers",
        trouble
            .as_deref()
            .unwrap_or("Third parties whose only job is to watch"),
        shield::enabled(shell),
        shield::set_enabled,
    ));
    if let Some(t) = &trouble {
        let r = line(
            t,
            "Nothing is being blocked until this clears — try again, or restart Copper",
        );
        let again = button("Try again", false);
        let weak = shell.downgrade();
        again.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                shield::retry(&s);
            }
        });
        r.add_suffix(&again);
        blocker.add(&r);
    }
    if let Some(site) = curtain::here(shell)
        && shield::enabled(shell)
        && trouble.is_none()
    {
        let s2 = site.clone();
        blocker.add(&switch(
            shell,
            &format!("Block on {site}"),
            "Turn off here if the site breaks — the page reloads",
            !shield::paused_on(shell, &site),
            move |s, on| shield::set_paused(s, &s2, !on),
        ));
    }
    let paused = shell.settings().shield_paused();
    if !paused.is_empty() {
        let r = line("Off for", &paused.join(", "));
        blocker.add(&r);
    }
    page.add(&blocker);

    let clear = group(Some("Clear browsing data"));
    let history = line("History", "Every address you have been to");
    let b = button("Clear…", false);
    b.update_property(&[gtk4::accessible::Property::Label("Clear history")]);
    let weak = shell.downgrade();
    b.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            confirm(
                &s,
                "Clear all history?",
                if history_syncs() {
                    "Every address you have been to goes from this computer and from Copper Cloud, for every computer signed in as you. This can’t be undone."
                } else {
                    "Every address you have been to goes from this computer. Tabs, bookmarks and passwords stay. This can’t be undone."
                },
                "Clear History",
                clear_history,
            );
        }
    });
    history.add_suffix(&b);
    clear.add(&history);

    let cookies = line(
        "Cookies and sign-ins",
        "Signs you out of every site, in every space",
    );
    let b = button("Sign out of everything…", false);
    let weak = shell.downgrade();
    b.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            let spaces = if s.browser().profiles().is_empty() {
                ""
            } else {
                " — in every space and profile"
            };
            confirm(
                &s,
                "Sign out of every site?",
                &format!(
                    "Cookies and site data are deleted{spaces}, so every site asks you to sign in again. Saved passwords stay."
                ),
                "Sign Out of Everything",
                clear_sites,
            );
        }
    });
    cookies.add_suffix(&b);
    clear.add(&cookies);

    let cache = line("Cache", "Only what was fetched to draw pages");
    let b = button("Clear", false);
    b.update_property(&[gtk4::accessible::Property::Label("Clear cache")]);
    let weak = shell.downgrade();
    b.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            clear_cache(&s);
        }
    });
    cache.add_suffix(&b);
    clear.add(&cache);
    page.add(&clear);
    page
}

/// History syncs to Copper Cloud now: clearing it here clears it there.
fn history_syncs() -> bool {
    crate::cloud::get().is_some_and(|c| c.sync.active(copper_cloud::Domain::History))
}

/// Every address, here and — while history syncs — on Copper Cloud.
pub fn clear_history(shell: &Shell) {
    shell.history_mut().clear();
    shell.history_changed();
    crate::cloud::history_cleared();
    shell.toast("History cleared");
    crate::features::panels::refresh(shell, "history");
}

/// Cookies and site data, in every jar.
pub fn clear_sites(shell: &Shell) {
    remove(shell, WebsiteDataKinds::ALL, "Signed out of everything");
}

/// Only what was fetched to draw pages.
pub fn clear_cache(shell: &Shell) {
    let kinds = WebsiteDataKinds::CACHES | WebsiteDataKinds::OFFLINE_APP_CACHE;
    remove(shell, kinds, "Cache cleared");
}

/// The shared jar and each space profile's.
fn remove(shell: &Shell, kinds: WebsiteDataKinds, said: &'static str) {
    let mut names: Vec<Option<String>> = vec![None];
    names.extend(shell.browser().profiles().into_iter().map(Some));
    let engine = shell.engine().clone();
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        let mut failed = false;
        for n in names {
            let Ok(p) = engine.profile(&profile_id(n.as_deref()), false) else {
                continue;
            };
            if let Err(e) = p.clear_website_data(None, kinds).await {
                tracing::warn!(error = %e, "clear website data");
                failed = true;
            }
        }
        if let Some(s) = weak.upgrade() {
            s.toast(if failed {
                "Some of it couldn’t be cleared"
            } else {
                said
            });
        }
    });
}
