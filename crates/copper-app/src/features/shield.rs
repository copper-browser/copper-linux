//! The ad blocker in the app (Mac: `Shield.swift`, `Browser.swift` launch,
//! Settings › Privacy).
//!
//! The list is compiled once at launch (or loaded from the engine's filter
//! store when this exact list was compiled before) and every tab's page asks
//! for it as it is made: pages made before the list is ready get it the
//! moment it is ([`copper_shield::Shield::protect`] / `ready`).
//!
//! Settings keys are the Mac's: `shield` (on, default) and `shield.paused`
//! (the sites it is off for, as `hidden.json` keys them — host, no `www.`).
//! The Mac takes the list off a tab at each navigation to a paused site;
//! WebKitGTK gives no hook early enough for that, so the paused sites are
//! baked into the list instead ([`copper_shield::List::exempting`], an
//! `ignore-previous-rules` for those main documents). Pausing a site
//! compiles the new list, swaps it into every page and reloads the page in
//! front — the same "the page reloads" the Mac's switch promises.

use crate::actions::{Command, register_command};
use crate::shell::Shell;
use copper_engine::{ContentFilter, PageView};
use copper_engine_webkitgtk::{WebKitContentFilter, WebKitPage};
use copper_shield::{List, Shield};
use std::cell::{Cell, RefCell};
use std::time::Duration;

struct State {
    shield: Shield<WebKitPage>,
    /// The list being compiled, while it is.
    compiling: Option<String>,
    /// The settings generation the shield last matched.
    seen: u64,
    /// Bumped per compile so a stale one never wins.
    round: u64,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static WATCH: Cell<bool> = const { Cell::new(false) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// The list for these paused sites: each one with and without `www.`, so a
/// pause on `example.com` covers `www.example.com` (Mac keys drop `www.`).
pub fn list_for(paused: &[String]) -> List {
    let mut hosts: Vec<String> = Vec::new();
    for h in paused {
        let h = h.trim().to_ascii_lowercase();
        if h.is_empty() {
            continue;
        }
        hosts.push(format!("www.{h}"));
        hosts.push(h);
    }
    List::exempting(hosts.iter().map(String::as_str))
}

/// At launch: compile (or load) the list, and keep watching the settings —
/// a cloud sync or another window may switch it.
pub fn start(shell: &Shell) {
    let (enabled, paused, generation) = {
        let s = shell.settings();
        (s.shield(), s.shield_paused(), s.generation())
    };
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            shield: Shield::new(enabled, paused.clone()),
            compiling: None,
            seen: generation,
            round: 0,
        })
    });
    compile(shell, &paused);
    commands(shell);
    if !WATCH.with(|w| w.replace(true)) {
        let weak = shell.downgrade();
        glib::timeout_add_local(Duration::from_secs(2), move || {
            let Some(shell) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            follow_settings(&shell);
            glib::ControlFlow::Continue
        });
    }
}

/// The palette's per-site switch (the Mac's is in Settings › Privacy, which
/// has it too).
fn commands(shell: &Shell) {
    let on_here = |s: &Shell| {
        crate::features::curtain::here(s).is_some_and(|h| enabled(s) && !paused_on(s, &h))
    };
    let off_here = |s: &Shell| {
        crate::features::curtain::here(s).is_some_and(|h| enabled(s) && paused_on(s, &h))
    };
    register_command(
        shell,
        Command::new(
            "shield-off-here",
            "Turn Off Ad Blocker for This Site",
            "security-low-symbolic",
            |s| toggle_here(s, true),
        )
        .when(on_here),
    );
    register_command(
        shell,
        Command::new(
            "shield-on-here",
            "Turn On Ad Blocker for This Site",
            "security-high-symbolic",
            |s| toggle_here(s, false),
        )
        .when(off_here),
    );
    register_command(
        shell,
        Command::new(
            "shield-on",
            "Turn On Ad Blocker",
            "security-high-symbolic",
            |s| {
                set_enabled(s, true);
                s.toast("Blocking ads and trackers");
            },
        )
        .when(|s| !enabled(s)),
    );
}

/// Off (or on) for the site in front, saying so.
fn toggle_here(shell: &Shell, off: bool) {
    let Some(site) = crate::features::curtain::here(shell) else {
        return;
    };
    set_paused(shell, &site, off);
    shell.toast(&if off {
        format!("Ad blocker off on {site} — reloading")
    } else {
        format!("Ad blocker on for {site} — reloading")
    });
}

/// Compile the list for `paused`; when it is ready every page gets it.
fn compile(shell: &Shell, paused: &[String]) {
    let list = list_for(paused);
    let Some((round, previous)) = with(|st| {
        st.round += 1;
        st.compiling = Some(list.id.clone());
        (st.round, st.shield.list().map(|l| l.id().to_string()))
    }) else {
        return;
    };
    let engine = shell.engine().clone();
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        let started = std::time::Instant::now();
        let result = copper_shield::compiled(&engine, &list, previous.as_deref()).await;
        let Some(shell) = weak.upgrade() else { return };
        let current = with(|st| st.round == round).unwrap_or(false);
        if !current {
            return;
        }
        match &result {
            Ok(_) => {
                tracing::info!(id = %list.id, ms = started.elapsed().as_millis() as u64, "shield list ready")
            }
            Err(e) => tracing::error!(error = %e, "shield list"),
        }
        ready(
            &shell,
            previous.as_deref(),
            result.map_err(|e| e.to_string()),
        );
    });
}

/// A list arrived: the old one comes off every page and this one goes on.
fn ready(shell: &Shell, previous: Option<&str>, result: Result<WebKitContentFilter, String>) {
    let pages = live_pages(shell);
    with(|st| {
        st.compiling = None;
        // A page that closed while it waited has nothing to protect.
        st.shield.forget(|p| !pages.iter().any(|q| same(p, q)));
        let had_list = st.shield.list().is_some();
        if let (Some(old), Ok(_)) = (previous, &result) {
            for p in &pages {
                p.remove_content_filter(old);
            }
        }
        st.shield.ready(result);
        // Waiting pages were handed the list by `ready`; every other page
        // gets it here (a swapped list), or keeps none when it is off.
        if had_list {
            st.shield.apply(pages.iter());
        }
    });
}

/// Every tab's live page.
fn live_pages(shell: &Shell) -> Vec<WebKitPage> {
    shell
        .pages_map()
        .borrow()
        .values()
        .map(|s| s.page.clone())
        .collect()
}

fn same(a: &WebKitPage, b: &WebKitPage) -> bool {
    a.widget() == b.widget()
}

/// A new page asks for the list (now, or when it is ready).
pub fn protect(_shell: &Shell, page: &WebKitPage) {
    with(|st| st.shield.protect(page));
}

/// Settings changed elsewhere (Settings › Privacy, Copper Cloud): follow.
fn follow_settings(shell: &Shell) {
    let (generation, enabled, paused) = {
        let s = shell.settings();
        (s.generation(), s.shield(), s.shield_paused())
    };
    let Some((was_enabled, was_paused, seen)) =
        with(|st| (st.shield.enabled, st.shield.paused(), st.seen))
    else {
        return;
    };
    if generation == seen {
        return;
    }
    with(|st| st.seen = generation);
    if enabled != was_enabled {
        let pages = live_pages(shell);
        with(|st| {
            st.shield.enabled = enabled;
            st.shield.apply(pages.iter());
        });
    }
    let mut sorted = paused.clone();
    sorted.sort();
    if sorted != was_paused {
        // Edited in place: pages still waiting for the first list stay
        // waiting.
        with(|st| {
            for h in &was_paused {
                if !sorted.contains(h) {
                    st.shield.pause(h, false);
                }
            }
            for h in &sorted {
                st.shield.pause(h, true);
            }
        });
        compile(shell, &sorted);
    }
}

/// Whether the blocker is on (the setting).
pub fn enabled(shell: &Shell) -> bool {
    shell.settings().shield()
}

/// Switch the blocker on or off everywhere; it takes effect with the next
/// request (Mac: every tab's controller is told).
pub fn set_enabled(shell: &Shell, on: bool) {
    shell.update_settings(|s| s.set_shield(on));
    follow_settings(shell);
}

/// Whether the blocker is off on `site` (a `hidden.json`-style key).
pub fn paused_on(shell: &Shell, site: &str) -> bool {
    shell.settings().shield_paused_on(site)
}

/// Off (or back on) for one site; the page in front reloads once the new
/// list is on it.
pub fn set_paused(shell: &Shell, site: &str, off: bool) {
    shell.update_settings(|s| s.set_shield_paused(site, off));
    follow_settings(shell);
    reload_when_ready(shell);
}

/// Reload the page in front once the list being compiled has arrived.
fn reload_when_ready(shell: &Shell) {
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        for _ in 0..100 {
            let busy = with(|st| st.compiling.is_some()).unwrap_or(false);
            if !busy {
                break;
            }
            glib::timeout_future(Duration::from_millis(50)).await;
        }
        if let Some(s) = weak.upgrade() {
            s.reload_active();
        }
    });
}

/// Why nothing is blocked, when compiling failed.
pub fn trouble() -> Option<String> {
    with(|st| st.shield.trouble().map(str::to_string)).flatten()
}

/// Try compiling again (Settings › Privacy › Try again).
pub fn retry(shell: &Shell) {
    let paused = shell.settings().shield_paused();
    compile(shell, &paused);
}

/// The id of the list on pages right now (tests).
pub fn current_list() -> Option<String> {
    with(|st| st.shield.list().map(|l| l.id().to_string())).flatten()
}

/// Whether a list is being compiled (tests).
pub fn busy() -> bool {
    with(|st| st.compiling.is_some()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pauses_cover_www() {
        let none = list_for(&[]);
        assert_eq!(none, List::standard());
        let l = list_for(&["example.com".into()]);
        let v: serde_json::Value = serde_json::from_str(&l.json).unwrap();
        let last = v.as_array().unwrap().last().unwrap();
        assert_eq!(
            last["trigger"]["if-domain"],
            serde_json::json!(["example.com", "www.example.com"])
        );
        assert_eq!(last["action"]["type"], "ignore-previous-rules");
    }
}
