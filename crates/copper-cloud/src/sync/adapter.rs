//! Reading this browser's state into Cloud documents and writing merged ones
//! back — through the objects the rest of the app edits, never behind their
//! backs into their files (Mac: `CloudApply.swift`).
//!
//! The engine calls these from its tokio tasks. Implementations must be
//! thread-safe and quick; a GTK app marshals each call onto its main loop
//! (e.g. `glib::MainContext::invoke` + a oneshot channel) and waits for the
//! answer. Everything is in `copper_core` types so the same adapter can back
//! the GTK app, the MCP server and tests ([`MemoryBrowser`]).

use copper_core::bookmarks::Bookmark;
use copper_core::cloud::OpenTab;
use copper_core::session::Space;
use std::collections::BTreeMap;
use std::sync::Mutex;

/// A pinned or Saved tab as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveKept {
    /// Stable for this run (the tab's own id), so a tab that navigates keeps
    /// its sync key.
    pub tab_id: String,
    /// The sync key the tab carries from an earlier run — kept on the tab
    /// itself, in the session ([`BrowserState::keep_keys`]) — so a kept tab
    /// that was restored somewhere else than where it was kept (a redirect
    /// at launch, browsing before the quit) is still the same entry.
    pub key: Option<String>,
    /// The space's id (uppercase UUID).
    pub space: String,
    pub pinned: bool,
    /// `http(s)` only; the pending address if the tab is asleep.
    pub url: String,
    pub title: String,
    /// The pin's letter, as the column draws it.
    pub pin: Option<String>,
}

/// A place as history keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalVisit {
    pub url: String,
    /// `copper_core::address::history_key`.
    pub key: String,
    pub title: String,
    pub count: u32,
    /// The last visit, Unix seconds.
    pub last: f64,
}

/// The browser, as the sync engine sees it.
pub trait BrowserState: Send + Sync {
    // spaces

    /// Every space, in the column's order.
    fn spaces(&self) -> Vec<Space>;
    /// The pinned and Saved `http(s)` tabs of every space, in row order (not
    /// private, not blank, not the agent's own tabs).
    fn kept_tabs(&self) -> Vec<LiveKept>;
    /// Make the spaces these: added, edited, ordered (Mac:
    /// `Spaces.cloudAdopt`). Spaces not listed are not removed here.
    fn adopt_spaces(&self, spaces: &[Space]);
    /// A space made here and never synced is the same as a server one with the
    /// same name: it takes that id, its tabs and all.
    fn rekey_space(&self, old: &str, new: &str);
    /// Close a kept tab the document no longer has.
    fn close_kept(&self, tab_id: &str);
    /// Open a kept tab, asleep, in `space`: pinned with `pin` as its letter,
    /// or Saved when `pin` is `None`. Returns the new tab's id.
    fn open_kept(&self, url: &str, title: &str, pin: Option<&str>, space: &str) -> Option<String>;
    /// Remove a space the document no longer has, moving its Today tabs to a
    /// neighbour. Never called for the last space.
    fn remove_space(&self, id: &str);
    /// Keep each tab's sync key on the tab (`(tab id, key)`), where it lasts
    /// across launches (the app: the tab's `session.json` entry), and hand it
    /// back as [`LiveKept::key`]. Not a change to push.
    fn keep_keys(&self, keys: &[(String, String)]);

    // settings

    /// The cloud settings values (`copper_core::settings::Settings::cloud_values`).
    fn settings(&self) -> BTreeMap<String, String>;
    /// Apply filtered cloud settings values.
    fn apply_settings(&self, values: &BTreeMap<String, String>);

    // bookmarks

    /// The whole bookmarks tree.
    fn bookmarks(&self) -> Vec<Bookmark>;
    /// Replace the whole tree, ids kept, in one write.
    fn replace_bookmarks(&self, roots: &[Bookmark]);

    // open tabs

    /// Every window's ordinary tabs and the parked rows' — not pins, not
    /// private, not blank — at most 500.
    fn open_tabs(&self) -> Vec<OpenTab>;

    // history

    /// Every place in history.
    fn visits(&self) -> Vec<LocalVisit>;
    /// A visit from another device (Mac: `History.take`).
    fn take_visit(&self, url: &str, title: &str, count: u32, last: f64);
    /// A batch of `take_visit` is over (save once).
    fn settle_history(&self) {}
}

/// An in-memory browser: for tests, and as the reference for what each
/// method means.
#[derive(Debug, Default)]
pub struct MemoryBrowser {
    pub state: Mutex<MemoryState>,
}

/// What a [`MemoryBrowser`] holds.
#[derive(Debug, Default, Clone)]
pub struct MemoryState {
    pub spaces: Vec<Space>,
    pub kept: Vec<LiveKept>,
    pub settings: BTreeMap<String, String>,
    pub bookmarks: Vec<Bookmark>,
    pub open: Vec<OpenTab>,
    pub visits: Vec<LocalVisit>,
    pub next_id: u64,
}

impl MemoryBrowser {
    pub fn new(state: MemoryState) -> Self {
        MemoryBrowser {
            state: Mutex::new(state),
        }
    }

    /// A copy of what it holds now.
    pub fn snapshot(&self) -> MemoryState {
        self.lock().clone()
    }

    /// Change what it holds.
    pub fn edit(&self, f: impl FnOnce(&mut MemoryState)) {
        f(&mut self.lock());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl BrowserState for MemoryBrowser {
    fn spaces(&self) -> Vec<Space> {
        self.lock().spaces.clone()
    }

    fn kept_tabs(&self) -> Vec<LiveKept> {
        self.lock().kept.clone()
    }

    fn adopt_spaces(&self, spaces: &[Space]) {
        let mut s = self.lock();
        let mut out: Vec<Space> = spaces.to_vec();
        // Spaces not in the document stay, after the others.
        for old in &s.spaces {
            if !out.iter().any(|n| n.id.eq_ignore_ascii_case(&old.id)) {
                out.push(old.clone());
            }
        }
        s.spaces = out;
    }

    fn rekey_space(&self, old: &str, new: &str) {
        let mut s = self.lock();
        for space in &mut s.spaces {
            if space.id.eq_ignore_ascii_case(old) {
                space.id = new.to_string();
            }
        }
        for tab in &mut s.kept {
            if tab.space.eq_ignore_ascii_case(old) {
                tab.space = new.to_string();
            }
        }
    }

    fn close_kept(&self, tab_id: &str) {
        self.lock().kept.retain(|t| t.tab_id != tab_id);
    }

    fn open_kept(&self, url: &str, title: &str, pin: Option<&str>, space: &str) -> Option<String> {
        let mut s = self.lock();
        s.next_id += 1;
        let id = format!("cloud-{}", s.next_id);
        s.kept.push(LiveKept {
            tab_id: id.clone(),
            key: None,
            space: space.to_string(),
            pinned: pin.is_some(),
            url: url.to_string(),
            title: title.to_string(),
            pin: pin.map(str::to_string),
        });
        Some(id)
    }

    fn remove_space(&self, id: &str) {
        let mut s = self.lock();
        s.spaces.retain(|x| !x.id.eq_ignore_ascii_case(id));
        s.kept.retain(|t| !t.space.eq_ignore_ascii_case(id));
    }

    fn keep_keys(&self, keys: &[(String, String)]) {
        let mut s = self.lock();
        for (tab, key) in keys {
            if let Some(t) = s.kept.iter_mut().find(|t| &t.tab_id == tab) {
                t.key = Some(key.clone());
            }
        }
    }

    fn settings(&self) -> BTreeMap<String, String> {
        copper_core::settings::filter_cloud_values(&self.lock().settings)
    }

    fn apply_settings(&self, values: &BTreeMap<String, String>) {
        let mut s = self.lock();
        for (k, v) in copper_core::settings::filter_cloud_values(values) {
            s.settings.insert(k, v);
        }
    }

    fn bookmarks(&self) -> Vec<Bookmark> {
        self.lock().bookmarks.clone()
    }

    fn replace_bookmarks(&self, roots: &[Bookmark]) {
        self.lock().bookmarks = roots.to_vec();
    }

    fn open_tabs(&self) -> Vec<OpenTab> {
        self.lock().open.iter().take(500).cloned().collect()
    }

    fn visits(&self) -> Vec<LocalVisit> {
        self.lock().visits.clone()
    }

    fn take_visit(&self, url: &str, title: &str, count: u32, last: f64) {
        let Ok(parsed) = url::Url::parse(url) else {
            return;
        };
        let key = copper_core::address::history_key(&parsed);
        let mut s = self.lock();
        if let Some(v) = s.visits.iter_mut().find(|v| v.key == key) {
            v.count = count.max(v.count);
            if last >= v.last {
                v.last = last;
                v.url = url.to_string();
                if !title.is_empty() {
                    v.title = title.to_string();
                }
            }
        } else {
            s.visits.push(LocalVisit {
                url: url.to_string(),
                key,
                title: title.to_string(),
                count,
                last,
            });
        }
    }
}
