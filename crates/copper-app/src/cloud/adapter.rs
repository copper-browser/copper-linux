//! [`copper_cloud::BrowserState`] over the live shell (Mac: `CloudApply.swift`).
//!
//! The sync engine calls these from its tokio workers. Each call is run on
//! the GTK main loop (`glib::MainContext::invoke`) and waited for; called on
//! the main thread itself it runs in place. Writes go through the shell's own
//! operations, so the column, the row, the stage and the session file follow
//! at once — and are marked seen, so the local-change watch never pushes a
//! remote apply back as an edit made here.

use crate::shell::Shell;
use copper_cloud::sync::{BrowserState, LiveKept, LocalVisit};
use copper_core::TabId;
use copper_core::bookmarks::Bookmark;
use copper_core::browser::OpenOptions;
use copper_core::cloud::OpenTab;
use copper_core::session::Space;
use std::collections::BTreeMap;
use std::time::Duration;

/// The app's browser, for the sync engine.
pub struct AppBrowser;

/// Run `f` with the shell on the main thread and wait for its answer
/// (`None`: the window has gone, or the main loop did not answer in time).
pub fn on_main<R, F>(f: F) -> Option<R>
where
    R: Send + 'static,
    F: FnOnce(&Shell) -> R + Send + 'static,
{
    let ctx = glib::MainContext::default();
    if ctx.is_owner() {
        return super::shell().map(|s| f(&s));
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    ctx.invoke(move || {
        let out = super::shell().map(|s| f(&s));
        let _ = tx.send(out);
    });
    rx.recv_timeout(Duration::from_secs(30)).ok().flatten()
}

/// A write from the cloud: run it, then count everything as seen.
fn apply<R, F>(f: F) -> Option<R>
where
    R: Send + 'static,
    F: FnOnce(&Shell) -> R + Send + 'static,
{
    on_main(move |s| {
        let out = f(s);
        super::mark_seen(s);
        out
    })
}

fn is_web(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The pinned and Saved web tabs, pins first, then each space's row.
pub fn kept(shell: &Shell) -> Vec<LiveKept> {
    let b = shell.browser();
    let mut out = Vec::new();
    for t in b.pins() {
        if t.private || !is_web(&t.url) {
            continue;
        }
        out.push(LiveKept {
            tab_id: t.id.0.clone(),
            space: t.space.clone(),
            pinned: true,
            url: t.url.clone(),
            title: t.title.clone(),
            pin: t.pin.clone(),
        });
    }
    for space in b.spaces() {
        for t in b.row(&space.id) {
            if !t.saved || t.is_pinned() || t.private || !is_web(&t.url) {
                continue;
            }
            out.push(LiveKept {
                tab_id: t.id.0.clone(),
                space: space.id.clone(),
                pinned: false,
                url: t.url.clone(),
                title: t.title.clone(),
                pin: None,
            });
        }
    }
    out
}

/// Every space's loose web tabs (not pins, not private), at most 500.
pub fn open_tabs(shell: &Shell) -> Vec<OpenTab> {
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
pub fn cloud_settings(shell: &Shell) -> BTreeMap<String, String> {
    let mut v = shell.settings().cloud_values();
    v.insert(
        copper_core::settings::keys::SIDEBAR.into(),
        (shell.tabs_position() == copper_core::settings::TabsPosition::Left).to_string(),
    );
    v
}

impl BrowserState for AppBrowser {
    fn spaces(&self) -> Vec<Space> {
        on_main(|s| s.browser().spaces().cloned().collect()).unwrap_or_default()
    }

    fn kept_tabs(&self) -> Vec<LiveKept> {
        on_main(kept).unwrap_or_default()
    }

    fn adopt_spaces(&self, spaces: &[Space]) {
        let spaces = spaces.to_vec();
        apply(move |s| {
            s.with_browser(|b| b.adopt_spaces(&spaces));
            s.show_active();
        });
    }

    fn rekey_space(&self, old: &str, new: &str) {
        let (old, new) = (old.to_string(), new.to_string());
        apply(move |s| {
            s.with_browser(|b| b.rekey_space(&old, &new));
        });
    }

    fn close_kept(&self, tab_id: &str) {
        let id = TabId(tab_id.to_string());
        apply(move |s| {
            s.close_force(&id);
        });
    }

    fn open_kept(&self, url: &str, title: &str, pin: Option<&str>, space: &str) -> Option<String> {
        let (url, title, space) = (url.to_string(), title.to_string(), space.to_string());
        let pin = pin.map(str::to_string);
        apply(move |s| {
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

    fn remove_space(&self, id: &str) {
        let id = id.to_string();
        apply(move |s| {
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
        on_main(cloud_settings).unwrap_or_default()
    }

    fn apply_settings(&self, values: &BTreeMap<String, String>) {
        let values = values.clone();
        apply(move |s| {
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
        on_main(|s| s.bookmarks().roots().to_vec()).unwrap_or_default()
    }

    fn replace_bookmarks(&self, roots: &[Bookmark]) {
        let roots = roots.to_vec();
        apply(move |s| {
            s.bookmarks_mut().replace(roots);
            let list = s.bookmarks().roots().to_vec();
            s.saver().schedule(list);
        });
    }

    fn open_tabs(&self) -> Vec<OpenTab> {
        on_main(open_tabs).unwrap_or_default()
    }

    fn visits(&self) -> Vec<LocalVisit> {
        on_main(|s| {
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
        apply(move |s| {
            s.history_mut().take(
                &url,
                &title,
                count as i64,
                copper_core::SwiftDate::from_unix(last),
            );
        });
    }

    fn settle_history(&self) {
        apply(|s| s.history_changed());
    }
}
