//! [`copper_cloud::BrowserState`] over the live shell (Mac: `CloudApply.swift`).
//!
//! The sync engine calls these from its tokio workers. Each call is run on
//! the UI main loop ([`MainLoop::post`]) and waited for; called on the main
//! thread itself it runs in place. Writes go through the shell's own
//! operations, so the column, the row, the stage and the session file follow
//! at once — and are marked seen, so the local-change watch never pushes a
//! remote apply back as an edit made here.

use crate::Platform;
use crate::main_loop::MainLoop;
use crate::shell::Shell;
use copper_cloud::sync::{BrowserState, LiveKept, LocalVisit};
use copper_core::bookmarks::Bookmark;
use copper_core::browser::OpenOptions;
use copper_core::cloud::{CloudFolder, OpenTab};
use copper_core::session::Space;
use copper_core::{Browser, Tab, TabId};
use serde_json::Value;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::time::Duration;

/// Where a kept tab's sync key lives: a key of its own in the tab's
/// `session.json` entry (the entry's unknown keys travel with the tab, and
/// back to the file). So a kept tab is the same entry of the spaces document
/// across launches, whatever address it was restored at. The Mac never reads
/// this file; its `Session.Entry` decoder would skip the key anyway.
pub const SYNC_KEY: &str = "cloudKey";

/// The app's browser, for the sync engine: the shell of platform `P`
/// (`fn() -> P`: sendable whatever `P` is; no `P` is held).
pub struct AppBrowser<P: Platform>(PhantomData<fn() -> P>);

impl<P: Platform> AppBrowser<P> {
    pub fn new() -> Self {
        AppBrowser(PhantomData)
    }
}

impl<P: Platform> Default for AppBrowser<P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Run `f` with the shell on the main thread and wait for its answer
/// (`None`: the window has gone, or the main loop did not answer in time).
pub fn on_main<P, R, F>(f: F) -> Option<R>
where
    P: Platform,
    R: Send + 'static,
    F: FnOnce(&Shell<P>) -> R + Send + 'static,
{
    if P::Loop::is_main_thread() {
        return crate::shell::current::<P>().map(|s| f(&s));
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    P::Loop::post(move || {
        let out = crate::shell::current::<P>().map(|s| f(&s));
        let _ = tx.send(out);
    });
    rx.recv_timeout(Duration::from_secs(30)).ok().flatten()
}

/// A write from the cloud: run it, then count everything as seen.
fn apply<P, R, F>(f: F) -> Option<R>
where
    P: Platform,
    R: Send + 'static,
    F: FnOnce(&Shell<P>) -> R + Send + 'static,
{
    on_main::<P, _, _>(move |s| {
        let out = f(s);
        super::mark_seen(s);
        out
    })
}

fn is_web(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The sync key a tab carries, if any.
pub fn carried_key(t: &Tab) -> Option<String> {
    t.extra
        .get(SYNC_KEY)
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
}

/// Keep each tab's sync key on it (`(tab id, key)`); tabs gone since are
/// skipped. How many changed.
pub fn keep_keys_in(b: &mut Browser, keys: &[(String, String)]) -> usize {
    let mut n = 0;
    for (tab, key) in keys {
        let id = TabId(tab.clone());
        let Some(t) = b.tab(&id) else { continue };
        if carried_key(t).as_deref() == Some(key.as_str()) {
            continue;
        }
        b.update_tab(&id, |t| {
            t.extra.insert(SYNC_KEY.into(), Value::String(key.clone()));
        });
        n += 1;
    }
    n
}

/// The pinned and Saved web tabs, pins first, then each space's row.
pub fn kept<P: Platform>(shell: &Shell<P>) -> Vec<LiveKept> {
    kept_in(&shell.browser())
}

/// [`kept`], of a browser.
pub fn kept_in(b: &Browser) -> Vec<LiveKept> {
    let mut out = Vec::new();
    for t in b.pins() {
        if t.private || !is_web(&t.url) {
            continue;
        }
        out.push(LiveKept {
            tab_id: t.id.0.clone(),
            key: carried_key(t),
            space: t.space.clone(),
            pinned: true,
            url: t.url.clone(),
            title: t.title.clone(),
            pin: t.pin.clone(),
            group: t.group.clone(),
        });
    }
    for space in b.spaces() {
        for t in b.row(&space.id) {
            if !t.saved || t.is_pinned() || t.private || !is_web(&t.url) {
                continue;
            }
            out.push(LiveKept {
                tab_id: t.id.0.clone(),
                key: carried_key(t),
                space: space.id.clone(),
                pinned: false,
                url: t.url.clone(),
                title: t.title.clone(),
                pin: None,
                group: t.group.clone(),
            });
        }
    }
    out
}

/// Every space's loose web tabs (not pins, not private), at most 500.
pub fn open_tabs<P: Platform>(shell: &Shell<P>) -> Vec<OpenTab> {
    let b = shell.browser();
    let active = b.active_id();
    let mut out = Vec::new();
    for space in b.spaces() {
        for t in b.row(&space.id) {
            if t.private || t.is_pinned() || !is_web(&t.url) {
                continue;
            }
            out.push(OpenTab {
                url: t.url.clone(),
                title: t.title.clone(),
                space: Some(space.name.clone()),
                active: (active.as_ref() == Some(&t.id)).then_some(true),
                extra: Default::default(),
            });
            if out.len() >= 500 {
                return out;
            }
        }
    }
    out
}

/// The cloud `settings` values as this device wears them: `sidebar` is the
/// shell's effective tab position (a fresh profile shows tabs on the left
/// before the setting is ever written).
pub fn cloud_settings<P: Platform>(shell: &Shell<P>) -> BTreeMap<String, String> {
    let mut v = shell.settings().cloud_values();
    v.insert(
        copper_core::settings::keys::SIDEBAR.into(),
        (shell.tabs_position() == copper_core::settings::TabsPosition::Left).to_string(),
    );
    v
}

impl<P: Platform> BrowserState for AppBrowser<P> {
    fn spaces(&self) -> Vec<Space> {
        on_main::<P, _, _>(|s| s.browser().spaces().cloned().collect()).unwrap_or_default()
    }

    fn kept_tabs(&self) -> Vec<LiveKept> {
        on_main::<P, _, _>(kept::<P>).unwrap_or_default()
    }

    fn adopt_spaces(&self, spaces: &[Space]) {
        let spaces = spaces.to_vec();
        apply::<P, _, _>(move |s| {
            s.with_browser(|b| b.adopt_spaces(&spaces));
            s.show_active();
        });
    }

    fn rekey_space(&self, old: &str, new: &str) {
        let (old, new) = (old.to_string(), new.to_string());
        apply::<P, _, _>(move |s| {
            s.with_browser(|b| b.rekey_space(&old, &new));
        });
    }

    fn close_kept(&self, tab_id: &str) {
        // Gone from the spaces document: removed, not merely unloaded — an
        // unloaded Saved row would still be kept here and come straight
        // back on the next push.
        let id = TabId(tab_id.to_string());
        apply::<P, _, _>(move |s| {
            s.close_force(&id);
        });
    }

    fn open_kept(&self, url: &str, title: &str, pin: Option<&str>, space: &str) -> Option<String> {
        let (url, title, space) = (url.to_string(), title.to_string(), space.to_string());
        let pin = pin.map(str::to_string);
        apply::<P, _, _>(move |s| {
            s.with_browser(|b| {
                let space = b
                    .spaces()
                    .find(|x| x.id.eq_ignore_ascii_case(&space))
                    .map(|x| x.id.clone())?;
                let id = b.open_tab(
                    &url,
                    OpenOptions {
                        title,
                        space: Some(space),
                        foreground: false,
                        at_end: true,
                        ..Default::default()
                    },
                );
                b.set_sleeping(&id, true);
                b.update_tab(&id, |t| t.saved = true);
                if let Some(letter) = pin.as_deref() {
                    b.pin(&id, Some(letter));
                }
                Some(id.0)
            })
        })
        .flatten()
    }

    fn keep_keys(&self, keys: &[(String, String)]) {
        let keys = keys.to_vec();
        apply::<P, _, _>(move |s| {
            s.with_browser(|b| keep_keys_in(b, &keys));
        });
    }

    fn groups(&self) -> Option<Vec<CloudFolder>> {
        on_main::<P, _, _>(|s| crate::agents::groups::folders(s))
    }

    fn adopt_groups(&self, folders: &[CloudFolder], removed: &[String]) {
        let (folders, removed) = (folders.to_vec(), removed.to_vec());
        apply::<P, _, _>(move |s| crate::agents::groups::adopt(s, &folders, &removed));
    }

    fn file_kept(&self, tab_id: &str, group: Option<&str>) {
        let (id, group) = (TabId(tab_id.to_string()), group.map(str::to_string));
        apply::<P, _, _>(move |s| {
            s.with_browser(|b| b.set_group(&id, group.as_deref()));
        });
    }

    fn remove_space(&self, id: &str) {
        let id = id.to_string();
        apply::<P, _, _>(move |s| {
            let found = s
                .browser()
                .spaces()
                .find(|x| x.id.eq_ignore_ascii_case(&id))
                .map(|x| x.id.clone());
            if let Some(id) = found {
                s.remove_space(&id, true);
            }
        });
    }

    fn settings(&self) -> BTreeMap<String, String> {
        on_main::<P, _, _>(cloud_settings::<P>).unwrap_or_default()
    }

    fn apply_settings(&self, values: &BTreeMap<String, String>) {
        let values = values.clone();
        apply::<P, _, _>(move |s| {
            let (look, position) = (s.look(), s.tabs_position());
            s.update_settings(|st| st.apply_cloud_values(&values));
            let now = s.settings().look();
            if now != look {
                s.set_look(now);
            }
            let now = s.tabs_position();
            if now != position {
                s.set_tabs_position(now);
            }
        });
    }

    fn bookmarks(&self) -> Vec<Bookmark> {
        on_main::<P, _, _>(|s| s.bookmarks().roots().to_vec()).unwrap_or_default()
    }

    fn replace_bookmarks(&self, roots: &[Bookmark]) {
        let roots = roots.to_vec();
        apply::<P, _, _>(move |s| {
            s.bookmarks_mut().replace(roots);
            let list = s.bookmarks().roots().to_vec();
            s.saver().schedule(list);
        });
    }

    fn open_tabs(&self) -> Vec<OpenTab> {
        on_main::<P, _, _>(open_tabs::<P>).unwrap_or_default()
    }

    fn visits(&self) -> Vec<LocalVisit> {
        on_main::<P, _, _>(|s| {
            s.history()
                .visits()
                .map(|v| LocalVisit {
                    url: v.url.clone(),
                    key: v.key.clone(),
                    title: v.title.clone(),
                    count: v.count.clamp(0, u32::MAX as i64) as u32,
                    last: v.last.unix(),
                })
                .collect()
        })
        .unwrap_or_default()
    }

    fn take_visit(&self, url: &str, title: &str, count: u32, last: f64) {
        let (url, title) = (url.to_string(), title.to_string());
        apply::<P, _, _>(move |s| {
            s.history_mut().take(
                &url,
                &title,
                count as i64,
                copper_core::SwiftDate::from_unix(last),
            );
        });
    }

    fn settle_history(&self) {
        apply::<P, _, _>(|s| s.history_changed());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::SessionShape;

    fn keep_one() -> (Browser, TabId, TabId) {
        let mut b = Browser::new();
        let saved = b.open_tab(
            "http://127.0.0.1:8080/r",
            OpenOptions {
                foreground: false,
                at_end: true,
                ..Default::default()
            },
        );
        b.update_tab(&saved, |t| t.saved = true);
        let pin = b.open_tab("https://mail.test/", OpenOptions::default());
        b.pin(&pin, Some("M"));
        b.take_events();
        (b, saved, pin)
    }

    #[test]
    fn a_kept_tabs_key_lives_on_the_tab() {
        let (mut b, saved, pin) = keep_one();
        assert!(kept_in(&b).iter().all(|k| k.key.is_none()));
        let keys = vec![
            (
                saved.0.clone(),
                "home|saved|127.0.0.1|http://127.0.0.1:8080/r".into(),
            ),
            (
                pin.0.clone(),
                "home|pin|mail.test|https://mail.test/".into(),
            ),
            ("gone".to_string(), "x".to_string()),
        ];
        assert_eq!(keep_keys_in(&mut b, &keys), 2);
        assert!(!b.take_events().is_empty(), "a change the session saves");
        // Again: nothing changes, nothing to save.
        assert_eq!(keep_keys_in(&mut b, &keys), 0);
        assert!(b.take_events().is_empty());
        let live = kept_in(&b);
        let of = |id: &TabId| live.iter().find(|k| k.tab_id == id.0).unwrap().key.clone();
        assert_eq!(
            of(&saved).as_deref(),
            Some("home|saved|127.0.0.1|http://127.0.0.1:8080/r")
        );
        assert_eq!(
            of(&pin).as_deref(),
            Some("home|pin|mail.test|https://mail.test/")
        );
        // Navigating doesn't touch it.
        b.set_url(&saved, "http://localhost:8080/landed");
        assert_eq!(of(&saved), carried_key(b.tab(&saved).unwrap()));
    }

    #[test]
    fn the_key_round_trips_through_session_json() {
        let (mut b, saved, pin) = keep_one();
        keep_keys_in(
            &mut b,
            &[
                (saved.0.clone(), "k-saved".into()),
                (pin.0.clone(), "k-pin".into()),
            ],
        );
        // The redirect it took before the quit is what the session keeps.
        b.set_url(&saved, "http://localhost:8080/landed");
        let text = serde_json::to_string(&b.to_session()).unwrap();
        assert!(text.contains(r#""cloudKey":"k-saved""#), "{text}");
        assert!(text.contains(r#""cloudKey":"k-pin""#), "{text}");
        // Next launch: new tab ids, the same keys.
        let shape: SessionShape = serde_json::from_str(&text).unwrap();
        let again = Browser::from_session(&shape);
        let live = kept_in(&again);
        assert_eq!(live.len(), 2);
        assert!(
            live.iter()
                .all(|k| k.tab_id != saved.0 && k.tab_id != pin.0)
        );
        let by_url: BTreeMap<String, Option<String>> =
            live.into_iter().map(|k| (k.url, k.key)).collect();
        assert_eq!(
            by_url["http://localhost:8080/landed"].as_deref(),
            Some("k-saved")
        );
        assert_eq!(by_url["https://mail.test/"].as_deref(), Some("k-pin"));
        // …and written back out unchanged.
        let twice = serde_json::to_string(&again.to_session()).unwrap();
        assert!(twice.contains(r#""cloudKey":"k-saved""#));
        assert!(twice.contains(r#""cloudKey":"k-pin""#));
    }

    #[test]
    fn unloading_a_saved_tab_keeps_it_kept_and_removing_it_does_not() {
        let (mut b, saved, pin) = keep_one();
        keep_keys_in(&mut b, &[(saved.0.clone(), "k-saved".into())]);
        let before = kept_in(&b);
        b.select(&saved);
        // ⌘W, an agent's close, Close Other Tabs: only unloaded. The kept
        // list the sync engine diffs is exactly what it was, so nothing is
        // deleted remotely.
        assert!(matches!(
            b.close_with(&saved, copper_core::browser::Closing::CLOSE),
            copper_core::CloseOutcome::Unloaded { .. }
        ));
        assert!(b.tab(&saved).unwrap().sleeping);
        assert_eq!(kept_in(&b), before);
        // The pin put down: still kept too.
        b.close_tab(&pin, false);
        assert_eq!(kept_in(&b), before);
        // The command line's close: still kept.
        b.close_with(&saved, copper_core::browser::Closing::KEEP_SAVED);
        assert_eq!(kept_in(&b), before);
        // The cross on the row: removed, so the next push deletes it.
        b.close_with(&saved, copper_core::browser::Closing::REMOVE);
        let after = kept_in(&b);
        assert_eq!(after.len(), 1);
        assert!(after.iter().all(|k| k.tab_id != saved.0));
        // Reopen Closed Tab: kept again (a new entry until the engine keys it).
        let back = b.reopen_closed().unwrap();
        let again = kept_in(&b);
        assert_eq!(again.len(), 2);
        assert!(
            again
                .iter()
                .any(|k| k.tab_id == back.0 && !k.pinned && k.url == "http://127.0.0.1:8080/r")
        );
    }

    #[test]
    fn the_mac_shape_reads_a_session_with_keys() {
        // `Session.Entry` on the Mac is a plain Codable struct: unknown keys
        // are skipped. Model it: an entry decoder that knows only its fields.
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        struct MacEntry {
            url: String,
            title: String,
            pin: Option<String>,
            saved: Option<bool>,
        }
        let (mut b, saved, _) = keep_one();
        keep_keys_in(&mut b, &[(saved.0.clone(), "k".into())]);
        let shape = serde_json::to_value(b.to_session()).unwrap();
        for e in shape["tabs"].as_array().unwrap() {
            serde_json::from_value::<MacEntry>(e.clone()).unwrap();
        }
    }
}
