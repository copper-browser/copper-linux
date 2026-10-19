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
//!
//! The palette's per-site switches are the app's (they are commands); this
//! is the state, the compiling and the settings they act on.

use crate::main_loop::MainLoop;
use crate::shell::Shell;
use crate::{PageOf, Platform};
use copper_engine::{ContentFilter, Engine, PageView};
use copper_shield::{List, Shield};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::time::Duration;

struct State<P: Platform> {
    shield: Shield<PageOf<P>>,
    /// The list being compiled, while it is.
    compiling: Option<String>,
    /// The settings generation the shield last matched.
    seen: u64,
    /// Bumped per compile so a stale one never wins.
    round: u64,
}

thread_local! {
    /// The `State<P>` (one platform per process).
    static STATE: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
    static WATCH: Cell<bool> = const { Cell::new(false) };
}

fn with<P: Platform, R>(f: impl FnOnce(&mut State<P>) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut()?.downcast_mut::<State<P>>().map(f))
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
pub fn start<P: Platform>(shell: &Shell<P>) {
    let (enabled, paused, generation) = {
        let s = shell.settings();
        (s.shield(), s.shield_paused(), s.generation())
    };
    STATE.with(|s| {
        *s.borrow_mut() = Some(Box::new(State::<P> {
            shield: Shield::new(enabled, paused.clone()),
            compiling: None,
            seen: generation,
            round: 0,
        }))
    });
    compile(shell, &paused);
    if !WATCH.with(|w| w.replace(true)) {
        let weak = shell.downgrade();
        P::Loop::spawn_local(async move {
            loop {
                P::Loop::sleep(Duration::from_secs(2)).await;
                let Some(shell) = weak.upgrade() else { break };
                follow_settings(&shell);
            }
        });
    }
}

/// Off (or on) for the site in front, saying so (the palette's per-site
/// switch; the Mac's is in Settings › Privacy, which has it too).
pub fn toggle_here<P: Platform>(shell: &Shell<P>, site: &str, off: bool) {
    set_paused(shell, site, off);
    shell.toast(&if off {
        format!("Ad blocker off on {site} — reloading")
    } else {
        format!("Ad blocker on for {site} — reloading")
    });
}

/// Compile the list for `paused`; when it is ready every page gets it.
fn compile<P: Platform>(shell: &Shell<P>, paused: &[String]) {
    let list = list_for(paused);
    let Some((round, previous)) = with::<P, _>(|st| {
        st.round += 1;
        st.compiling = Some(list.id.clone());
        (st.round, st.shield.list().map(|l| l.id().to_string()))
    }) else {
        return;
    };
    let engine = shell.engine().clone();
    let weak = shell.downgrade();
    P::Loop::spawn_local(async move {
        let started = std::time::Instant::now();
        let result = copper_shield::compiled(&engine, &list, previous.as_deref()).await;
        let Some(shell) = weak.upgrade() else { return };
        let current = with::<P, _>(|st| st.round == round).unwrap_or(false);
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
fn ready<P: Platform>(
    shell: &Shell<P>,
    previous: Option<&str>,
    result: Result<<P::Engine as Engine>::Filter, String>,
) {
    let pages = live_pages(shell);
    with::<P, _>(|st| {
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
fn live_pages<P: Platform>(shell: &Shell<P>) -> Vec<PageOf<P>> {
    shell
        .pages_map()
        .borrow()
        .values()
        .map(|s| s.page.clone())
        .collect()
}

fn same<V: PageView>(a: &V, b: &V) -> bool {
    a.page_id() == b.page_id()
}

/// A new page asks for the list (now, or when it is ready).
pub fn protect<P: Platform>(_shell: &Shell<P>, page: &PageOf<P>) {
    with::<P, _>(|st| st.shield.protect(page));
}

/// Settings changed elsewhere (Settings › Privacy, Copper Cloud): follow.
fn follow_settings<P: Platform>(shell: &Shell<P>) {
    let (generation, enabled, paused) = {
        let s = shell.settings();
        (s.generation(), s.shield(), s.shield_paused())
    };
    let Some((was_enabled, was_paused, seen)) =
        with::<P, _>(|st| (st.shield.enabled, st.shield.paused(), st.seen))
    else {
        return;
    };
    if generation == seen {
        return;
    }
    with::<P, _>(|st| st.seen = generation);
    if enabled != was_enabled {
        let pages = live_pages(shell);
        with::<P, _>(|st| {
            st.shield.enabled = enabled;
            st.shield.apply(pages.iter());
        });
    }
    let mut sorted = paused.clone();
    sorted.sort();
    if sorted != was_paused {
        // Edited in place: pages still waiting for the first list stay
        // waiting.
        with::<P, _>(|st| {
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
pub fn enabled<P: Platform>(shell: &Shell<P>) -> bool {
    shell.settings().shield()
}

/// Switch the blocker on or off everywhere; it takes effect with the next
/// request (Mac: every tab's controller is told).
pub fn set_enabled<P: Platform>(shell: &Shell<P>, on: bool) {
    shell.update_settings(|s| s.set_shield(on));
    follow_settings(shell);
}

/// Whether the blocker is off on `site` (a `hidden.json`-style key).
pub fn paused_on<P: Platform>(shell: &Shell<P>, site: &str) -> bool {
    shell.settings().shield_paused_on(site)
}

/// Off (or back on) for one site; the page in front reloads once the new
/// list is on it.
pub fn set_paused<P: Platform>(shell: &Shell<P>, site: &str, off: bool) {
    shell.update_settings(|s| s.set_shield_paused(site, off));
    follow_settings(shell);
    reload_when_ready(shell);
}

/// Reload the page in front once the list being compiled has arrived.
fn reload_when_ready<P: Platform>(shell: &Shell<P>) {
    let weak = shell.downgrade();
    P::Loop::spawn_local(async move {
        for _ in 0..100 {
            if !busy::<P>() {
                break;
            }
            P::Loop::sleep(Duration::from_millis(50)).await;
        }
        if let Some(s) = weak.upgrade() {
            s.reload_active();
        }
    });
}

/// Why nothing is blocked, when compiling failed.
pub fn trouble<P: Platform>() -> Option<String> {
    with::<P, _>(|st| st.shield.trouble().map(str::to_string)).flatten()
}

/// Try compiling again (Settings › Privacy › Try again).
pub fn retry<P: Platform>(shell: &Shell<P>) {
    let paused = shell.settings().shield_paused();
    compile(shell, &paused);
}

/// The id of the list on pages right now (tests).
pub fn current_list<P: Platform>() -> Option<String> {
    with::<P, _>(|st| st.shield.list().map(|l| l.id().to_string())).flatten()
}

/// Whether a list is being compiled (tests).
pub fn busy<P: Platform>() -> bool {
    with::<P, _>(|st| st.compiling.is_some()).unwrap_or(false)
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
