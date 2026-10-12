//! The browser's state, without a single widget: spaces, tabs, pins,
//! selection, the closed-tab stack. Used by the GTK shell and the MCP server
//! (Mac: `Browser.swift`, `Fork/Spaces.swift`, `Session.swift`).
//!
//! # Model
//!
//! * Every space has one **row** of loose tabs.
//! * **Pins** belong to no space: one ordered set shown at the head of every
//!   space's row. Each pin remembers the space it was made in (its *home*,
//!   which picks its cookie jar when it is built).
//! * The **visible** list of the current space is pins followed by its row
//!   ([`Browser::visible`]) — what the column / strip draws and what MCP
//!   `browser_tabs` numbers.
//! * A **sleeping** tab has no live page; it keeps its URL and title and
//!   wakes when selected. Restored tabs start asleep except the active one.
//! * A **private** tab is never written to the session, never enters the
//!   closed-tab stack, and should be given an ephemeral profile by the engine.
//! * [`TabId`]s are fresh UUIDs per run and stable for the tab's life.
//!
//! Every mutation bumps [`Browser::generation`] and queues a
//! [`BrowserEvent`] ([`Browser::take_events`]); the shell redraws and asks
//! the [`crate::saver::Saver`] to write [`Browser::to_session`].

use crate::date::UnixTime;
use crate::history::{Suggestion, SuggestionKind};
use crate::session::{SessionEntry, SessionShape, Space};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fmt;

/// How many closed tabs ⌘⇧T remembers.
pub const CLOSED_CAP: usize = 12;

/// A tab's id: an uppercase UUID, fresh each run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub String);

impl TabId {
    /// A fresh id.
    pub fn new() -> Self {
        TabId(crate::ids::new_uuid())
    }
    /// The first 8 hex digits, lowercase — what MCP prints as `{tab 1a2b3c4d}`.
    pub fn short(&self) -> String {
        self.0
            .chars()
            .filter(|c| *c != '-')
            .take(8)
            .collect::<String>()
            .to_lowercase()
    }
    /// Whether `text` names this tab: the full id (any case) or its short form.
    pub fn matches(&self, text: &str) -> bool {
        self.0.eq_ignore_ascii_case(text) || self.short() == text.to_lowercase()
    }
}

impl Default for TabId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One tab.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: TabId,
    /// The address (for a sleeping tab, the one it will load on waking).
    pub url: String,
    pub title: String,
    /// Favicon URL or `data:` URI, if the page gave one.
    pub favicon: Option<String>,
    /// Pinned: the letter (or emoji) it is reduced to. `None` for a loose tab.
    pub pin: Option<String>,
    /// The row it lives in; for a pin, its home space.
    pub space: String,
    /// Tab group UUID.
    pub group: Option<String>,
    /// Sections: Saved (true) or Today (false). Pins are always saved.
    pub saved: bool,
    /// Last looked at.
    pub seen: UnixTime,
    /// Split-view token shared by both halves.
    pub split: Option<String>,
    /// No live page.
    pub sleeping: bool,
    /// Never persisted; ephemeral profile.
    pub private: bool,
    /// Unknown session keys from the entry it was restored from.
    pub extra: Map<String, Value>,
}

impl Tab {
    fn new(url: &str, title: &str, space: &str) -> Self {
        Tab {
            id: TabId::new(),
            url: url.to_string(),
            title: title.to_string(),
            favicon: None,
            pin: None,
            space: space.to_string(),
            group: None,
            saved: false,
            seen: UnixTime::now(),
            split: None,
            sleeping: false,
            private: false,
            extra: Map::new(),
        }
    }

    /// Pinned.
    pub fn is_pinned(&self) -> bool {
        self.pin.is_some()
    }

    /// What the row shows: the title, or the pretty address.
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            crate::address::pretty_str(&self.url)
        } else {
            self.title.clone()
        }
    }

    /// Worth writing to the session: not private, on a real page.
    pub fn persistable(&self) -> bool {
        !self.private && is_restorable_url(&self.url)
    }

    /// The letter a new pin gets: the first letter of the title or host.
    pub fn monogram(&self) -> String {
        let source = if self.title.is_empty() {
            crate::address::pretty_str(&self.url)
        } else {
            self.title.clone()
        };
        source
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "•".into())
    }
}

fn is_restorable_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("copper://canvas")
}

/// A closed tab, for ⌘⇧T.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedTab {
    pub url: String,
    pub title: String,
    /// The space it was in.
    pub space: String,
    /// Its index in that row.
    pub index: usize,
    pub group: Option<String>,
}

/// A space and its row.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceRow {
    pub space: Space,
    /// Loose tabs, in order.
    pub tabs: Vec<TabId>,
    /// The tab last shown in this space (may be a pin).
    pub active: Option<TabId>,
}

/// Where and how to open a tab.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    pub title: String,
    /// The space (default: current).
    pub space: Option<String>,
    /// Select it.
    pub foreground: bool,
    /// At the end of the row instead of next to the active tab.
    pub at_end: bool,
    /// A private tab.
    pub private: bool,
    /// Explicit index into the row (overrides `at_end`).
    pub index: Option<usize>,
}

/// What closing did.
#[derive(Debug, Clone, PartialEq)]
pub enum CloseOutcome {
    /// Removed; `selected` is the tab now active, if any.
    Closed {
        selected: Option<TabId>,
    },
    /// A pin is put down (asleep), not closed.
    PutDown {
        selected: Option<TabId>,
    },
    NotFound,
}

/// A change, for the shell to redraw.
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserEvent {
    TabOpened(TabId),
    TabClosed(TabId),
    /// URL, title, favicon, sleep, group, pin letter… changed.
    TabChanged(TabId),
    /// Order or membership of a row or of the pins changed.
    RowChanged {
        space: Option<String>,
    },
    Selected(Option<TabId>),
    SpaceSwitched(String),
    /// A space was added, removed, renamed, reordered or restyled.
    SpacesChanged,
}

/// The whole state.
#[derive(Debug, Clone)]
pub struct Browser {
    spaces: Vec<SpaceRow>,
    pins: Vec<TabId>,
    tabs: HashMap<TabId, Tab>,
    current: String,
    closed: Vec<ClosedTab>,
    /// Unknown top-level session keys, written back out.
    session_extra: Map<String, Value>,
    generation: u64,
    events: Vec<BrowserEvent>,
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    /// One space, "Home", no tabs.
    pub fn new() -> Self {
        let home = Space::new("Home");
        let current = home.id.clone();
        Browser {
            spaces: vec![SpaceRow {
                space: home,
                tabs: Vec::new(),
                active: None,
            }],
            pins: Vec::new(),
            tabs: HashMap::new(),
            current,
            closed: Vec::new(),
            session_extra: Map::new(),
            generation: 0,
            events: Vec::new(),
        }
    }

    fn changed(&mut self, e: BrowserEvent) {
        self.generation += 1;
        self.events.push(e);
    }

    /// Bumped on every change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The changes since the last call.
    pub fn take_events(&mut self) -> Vec<BrowserEvent> {
        std::mem::take(&mut self.events)
    }

    // MARK: session

    /// Rebuild from `session.json` (`Spaces.restore`): spaces and current
    /// space, global pins (or, from an older file, every space's pins merged
    /// into one set, first address wins), each row, and the active tab.
    /// Everything comes back asleep except the active tab.
    pub fn from_session(saved: &SessionShape) -> Self {
        let mut b = Browser::new();
        b.session_extra = saved.extra.clone();
        if let Some(spaces) = saved.spaces.as_ref().filter(|s| !s.is_empty()) {
            b.spaces = spaces
                .iter()
                .map(|s| SpaceRow {
                    space: s.clone(),
                    tabs: Vec::new(),
                    active: None,
                })
                .collect();
            b.current = saved
                .space
                .as_ref()
                .filter(|c| spaces.iter().any(|s| &s.id == *c))
                .cloned()
                .unwrap_or_else(|| spaces[0].id.clone());
        }
        let known = |b: &Browser, id: &Option<String>| -> Option<String> {
            id.as_ref()
                .filter(|i| b.spaces.iter().any(|s| &s.space.id == *i))
                .cloned()
        };
        let legacy = saved.pins.is_none();
        let order: HashMap<&str, usize> = b
            .spaces
            .iter()
            .enumerate()
            .map(|(i, s)| (s.space.id.as_str(), i))
            .collect();
        let current_rank = order.get(b.current.as_str()).copied().unwrap_or(0);
        let rank = |e: &SessionEntry| {
            e.space
                .as_deref()
                .and_then(|s| order.get(s).copied())
                .unwrap_or(current_rank)
        };
        let sources: Vec<(Option<usize>, &SessionEntry)> = if legacy {
            let mut v: Vec<(usize, &SessionEntry)> = saved
                .tabs
                .iter()
                .enumerate()
                .filter(|(_, e)| e.pin.is_some())
                .collect();
            v.sort_by_key(|(i, e)| (rank(e), *i));
            v.into_iter().map(|(i, e)| (Some(i), e)).collect()
        } else {
            saved.pins.iter().flatten().map(|e| (None, e)).collect()
        };
        let saved_active = usize::try_from(saved.active).ok();
        let mut active_pin: Option<TabId> = None;
        let mut kept: HashMap<String, TabId> = HashMap::new();
        for (i, entry) in sources {
            if entry.url.is_empty() {
                continue;
            }
            let was_active = entry.active == Some(true)
                || (legacy && entry.space.is_none() && i.is_some() && i == saved_active);
            let key = pin_key(&entry.url);
            if legacy && let Some(id) = kept.get(&key) {
                if was_active {
                    active_pin = Some(id.clone());
                }
                continue;
            }
            let home = known(&b, &entry.space).unwrap_or_else(|| b.current.clone());
            let mut tab = tab_from_entry(entry, &home);
            if tab.pin.as_deref().is_none_or(str::is_empty) {
                tab.pin = Some(tab.monogram());
            }
            tab.saved = true;
            let id = tab.id.clone();
            kept.insert(key, id.clone());
            if was_active {
                active_pin = Some(id.clone());
            }
            b.pins.push(id.clone());
            b.tabs.insert(id, tab);
        }
        let mut row_active: HashMap<String, TabId> = HashMap::new();
        for (i, entry) in saved.tabs.iter().enumerate() {
            if entry.pin.is_some() || entry.url.is_empty() {
                continue;
            }
            let space = known(&b, &entry.space).unwrap_or_else(|| b.current.clone());
            let mut tab = tab_from_entry(entry, &space);
            tab.saved = entry.saved.unwrap_or(true);
            let id = tab.id.clone();
            if entry.active == Some(true) || (entry.space.is_none() && Some(i) == saved_active) {
                row_active.insert(space.clone(), id.clone());
            }
            if let Some(row) = b.spaces.iter_mut().find(|s| s.space.id == space) {
                row.tabs.push(id.clone());
            }
            b.tabs.insert(id, tab);
        }
        for row in &mut b.spaces {
            row.active = row_active.get(&row.space.id).cloned();
        }
        let current = b.current.clone();
        let visible = b.visible_ids(&current);
        let chosen = active_pin
            .filter(|p| visible.contains(p))
            .or_else(|| row_active.get(&current).cloned())
            .or_else(|| b.row_of(&current).and_then(|r| r.tabs.first().cloned()))
            .or_else(|| visible.first().cloned());
        if let Some(row) = b.row_mut(&current) {
            row.active = chosen.clone();
        }
        if let Some(id) = &chosen
            && let Some(t) = b.tabs.get_mut(id)
        {
            t.sleeping = false;
        }
        b.generation = 0;
        b.events.clear();
        b
    }

    /// The session to write (`Spaces.shape`): every space's row in space
    /// order, with the older builds' copy of the pins at the head of the
    /// current space; `pins` with each pin's home; `active` indexes the
    /// active tab within `tabs`. Private and non-web tabs are left out.
    pub fn to_session(&self) -> SessionShape {
        let active = self.active_id();
        let mut entries: Vec<SessionEntry> = Vec::new();
        let mut active_index = 0usize;
        for row in &self.spaces {
            let shown = row.space.id == self.current;
            let mut put = |id: &TabId, entries: &mut Vec<SessionEntry>| {
                let Some(tab) = self.tabs.get(id).filter(|t| t.persistable()) else {
                    return;
                };
                let mut e = entry_from_tab(tab, Some(row.space.id.clone()));
                if shown && active.as_ref() == Some(id) {
                    e.active = Some(true);
                    active_index = entries.len();
                }
                entries.push(e);
            };
            if shown {
                for id in &self.pins {
                    put(id, &mut entries);
                }
            }
            for id in &row.tabs {
                put(id, &mut entries);
            }
        }
        let pins: Vec<SessionEntry> = self
            .pins
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .filter(|t| t.persistable())
            .map(|t| {
                let mut e = entry_from_tab(t, Some(t.space.clone()));
                if active.as_ref() == Some(&t.id) {
                    e.active = Some(true);
                }
                e
            })
            .collect();
        SessionShape {
            tabs: entries,
            active: active_index as i64,
            spaces: Some(self.spaces.iter().map(|r| r.space.clone()).collect()),
            space: Some(self.current.clone()),
            pins: Some(pins),
            extra: self.session_extra.clone(),
        }
    }

    // MARK: reading

    /// Every space, in order.
    pub fn spaces(&self) -> impl Iterator<Item = &Space> {
        self.spaces.iter().map(|r| &r.space)
    }

    /// Every space with its row.
    pub fn rows(&self) -> &[SpaceRow] {
        &self.spaces
    }

    /// One space.
    pub fn space(&self, id: &str) -> Option<&Space> {
        self.row_of(id).map(|r| &r.space)
    }

    /// The current space's id.
    pub fn current_space_id(&self) -> &str {
        &self.current
    }

    /// The current space.
    pub fn current_space(&self) -> &Space {
        &self
            .row_of(&self.current)
            .expect("current space exists")
            .space
    }

    /// The cookie jar a tab uses: `None` = the shared default. Private tabs
    /// should get an ephemeral profile regardless.
    pub fn profile_of(&self, id: &TabId) -> Option<String> {
        let tab = self.tabs.get(id)?;
        self.space(&tab.space)?.profile.clone()
    }

    /// Every profile name in use, sorted.
    pub fn profiles(&self) -> Vec<String> {
        let mut p: Vec<String> = self.spaces().filter_map(|s| s.profile.clone()).collect();
        p.sort();
        p.dedup();
        p
    }

    /// A tab.
    pub fn tab(&self, id: &TabId) -> Option<&Tab> {
        self.tabs.get(id)
    }

    /// Find a tab by full or short id.
    pub fn find_tab(&self, text: &str) -> Option<&Tab> {
        self.tabs.values().find(|t| t.id.matches(text))
    }

    /// Every tab (pins and every row), in no particular order.
    pub fn all_tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.values()
    }

    /// The pins, in order.
    pub fn pins(&self) -> Vec<&Tab> {
        self.pins
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .collect()
    }

    /// A space's loose tabs, in order.
    pub fn row(&self, space: &str) -> Vec<&Tab> {
        self.row_of(space)
            .map(|r| r.tabs.iter().filter_map(|id| self.tabs.get(id)).collect())
            .unwrap_or_default()
    }

    /// Pins then the current space's row: what the window shows.
    pub fn visible(&self) -> Vec<&Tab> {
        self.visible_ids(&self.current)
            .iter()
            .filter_map(|id| self.tabs.get(id))
            .collect()
    }

    /// The selected tab's id.
    pub fn active_id(&self) -> Option<TabId> {
        self.row_of(&self.current)
            .and_then(|r| r.active.clone())
            .filter(|id| self.tabs.contains_key(id))
    }

    /// The selected tab.
    pub fn active(&self) -> Option<&Tab> {
        self.active_id().and_then(|id| self.tabs.get(&id))
    }

    /// Closed tabs, newest last.
    pub fn closed_tabs(&self) -> &[ClosedTab] {
        &self.closed
    }

    /// Open (non-private) tabs whose address or title matches what was
    /// typed, as "Switch to Tab" completion rows (kind [`SuggestionKind::Open`]).
    pub fn open_tab_suggestions(&self, typed: &str, limit: usize) -> Vec<Suggestion> {
        let needle = crate::address::strip_typed(typed);
        if needle.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<Suggestion> = self
            .tabs
            .values()
            .filter(|t| !t.private)
            .filter_map(|t| {
                let key = crate::address::strip_typed(&crate::address::pretty_str(&t.url));
                let title = t.title.to_lowercase();
                let score = if key.starts_with(&needle) {
                    12.0
                } else if title.starts_with(&needle) {
                    10.0
                } else if key.contains(&needle) || title.contains(&needle) {
                    7.0
                } else {
                    return None;
                };
                Some(Suggestion {
                    key: t.title.clone(),
                    title: t.title.clone(),
                    url: t.url.clone(),
                    kind: SuggestionKind::Open,
                    tab: Some(t.id.clone()),
                    score: score + t.seen.0 / 1e12,
                })
            })
            .collect();
        crate::history::sort_suggestions(&mut out);
        out.truncate(limit);
        out
    }

    // MARK: spaces

    /// Add a space at the end; returns its id.
    pub fn add_space(&mut self, name: &str) -> String {
        let name = if name.trim().is_empty() {
            format!("Space {}", self.spaces.len() + 1)
        } else {
            name.to_string()
        };
        let space = Space::new(name);
        let id = space.id.clone();
        self.spaces.push(SpaceRow {
            space,
            tabs: Vec::new(),
            active: None,
        });
        self.changed(BrowserEvent::SpacesChanged);
        id
    }

    /// Replace a space's name, hue, icon, profile, theme (id kept).
    pub fn update_space(&mut self, id: &str, f: impl FnOnce(&mut Space)) -> bool {
        let Some(row) = self.row_mut(id) else {
            return false;
        };
        let keep = row.space.id.clone();
        f(&mut row.space);
        row.space.id = keep;
        self.changed(BrowserEvent::SpacesChanged);
        true
    }

    /// Move a space to `index`.
    pub fn move_space(&mut self, id: &str, index: usize) -> bool {
        let Some(from) = self.spaces.iter().position(|r| r.space.id == id) else {
            return false;
        };
        let row = self.spaces.remove(from);
        let to = index.min(self.spaces.len());
        self.spaces.insert(to, row);
        self.changed(BrowserEvent::SpacesChanged);
        true
    }

    /// Remove a space (never the last). Its tabs move to `keep_tabs_in` if
    /// given, else they close. Pins made there move home to the landing
    /// space. Returns the space now current if it changed.
    pub fn remove_space(&mut self, id: &str, keep_tabs_in: Option<&str>) -> bool {
        if self.spaces.len() < 2 {
            return false;
        }
        let Some(i) = self.spaces.iter().position(|r| r.space.id == id) else {
            return false;
        };
        let kept = keep_tabs_in
            .filter(|d| *d != id && self.row_of(d).is_some())
            .map(str::to_string);
        let landing = kept
            .clone()
            .unwrap_or_else(|| self.spaces[if i == 0 { 1 } else { i - 1 }].space.id.clone());
        let row = self.spaces.remove(i);
        match &kept {
            Some(dest) => {
                for tid in &row.tabs {
                    if let Some(t) = self.tabs.get_mut(tid) {
                        t.space = dest.clone();
                    }
                }
                if let Some(d) = self.row_mut(dest) {
                    d.tabs.extend(row.tabs.iter().cloned());
                }
            }
            None => {
                for tid in &row.tabs {
                    self.tabs.remove(tid);
                }
            }
        }
        for pid in &self.pins {
            if let Some(t) = self.tabs.get_mut(pid)
                && t.space == id
            {
                t.space = landing.clone();
            }
        }
        if self.current == id {
            self.current = landing.clone();
            self.ensure_active();
            self.changed(BrowserEvent::SpaceSwitched(landing));
        }
        self.changed(BrowserEvent::SpacesChanged);
        true
    }

    /// Show another space. Lands on the tab it last showed, else its most
    /// recently seen tab, else its first visible tab.
    pub fn switch_space(&mut self, id: &str) -> bool {
        if self.row_of(id).is_none() || self.current == id {
            return false;
        }
        self.current = id.to_string();
        self.ensure_active();
        self.changed(BrowserEvent::SpaceSwitched(id.to_string()));
        if let Some(a) = self.active_id() {
            self.wake(&a);
        }
        self.changed(BrowserEvent::Selected(self.active_id()));
        true
    }

    /// The next/previous space (wrapping). Returns the new current id.
    pub fn cycle_space(&mut self, forward: bool) -> Option<String> {
        let n = self.spaces.len();
        if n < 2 {
            return None;
        }
        let i = self
            .spaces
            .iter()
            .position(|r| r.space.id == self.current)?;
        let j = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        let id = self.spaces[j].space.id.clone();
        self.switch_space(&id);
        Some(id)
    }

    // MARK: tabs

    /// Open `url` (`Browser.open`): beside the active tab (at the head of the
    /// row when the active tab is a pin), or at the end with `at_end`.
    pub fn open_tab(&mut self, url: &str, opts: OpenOptions) -> TabId {
        let space = opts
            .space
            .filter(|s| self.row_of(s).is_some())
            .unwrap_or_else(|| self.current.clone());
        let mut tab = Tab::new(url, &opts.title, &space);
        tab.private = opts.private;
        let id = tab.id.clone();
        let active = self.active_id();
        let in_current = space == self.current;
        let row = self.row_mut(&space).expect("space exists");
        let index = match (opts.index, opts.at_end) {
            (Some(i), _) => i.min(row.tabs.len()),
            (None, true) => row.tabs.len(),
            (None, false) => match active
                .as_ref()
                .and_then(|a| row.tabs.iter().position(|t| t == a))
            {
                Some(p) => p + 1,
                None if active.is_some() && in_current => 0,
                None => row.tabs.len(),
            },
        };
        row.tabs.insert(index, id.clone());
        self.tabs.insert(id.clone(), tab);
        self.changed(BrowserEvent::TabOpened(id.clone()));
        self.changed(BrowserEvent::RowChanged {
            space: Some(space.clone()),
        });
        if opts.foreground {
            self.select(&id);
        }
        id
    }

    /// Select a tab, switching space if it lives in another one. Wakes it
    /// and stamps `seen`.
    pub fn select(&mut self, id: &TabId) -> bool {
        let Some(tab) = self.tabs.get(id) else {
            return false;
        };
        if !tab.is_pinned() && tab.space != self.current {
            let space = tab.space.clone();
            self.current = space.clone();
            self.changed(BrowserEvent::SpaceSwitched(space));
        }
        let current = self.current.clone();
        if let Some(row) = self.row_mut(&current) {
            row.active = Some(id.clone());
        }
        self.wake(id);
        if let Some(t) = self.tabs.get_mut(id) {
            t.seen = UnixTime::now();
        }
        self.changed(BrowserEvent::Selected(Some(id.clone())));
        true
    }

    /// Select the `index`th visible tab.
    pub fn select_index(&mut self, index: usize) -> bool {
        match self.visible_ids(&self.current).get(index).cloned() {
            Some(id) => self.select(&id),
            None => false,
        }
    }

    /// Close a tab (`Browser.close`). A pin is put down (asleep) instead,
    /// unless `force`. A closed non-private tab joins the closed stack.
    pub fn close_tab(&mut self, id: &TabId, force: bool) -> CloseOutcome {
        let Some(tab) = self.tabs.get(id).cloned() else {
            return CloseOutcome::NotFound;
        };
        let was_active = self.active_id().as_ref() == Some(id);
        if tab.is_pinned() && !force {
            if let Some(t) = self.tabs.get_mut(id) {
                t.sleeping = true;
            }
            self.changed(BrowserEvent::TabChanged(id.clone()));
            let mut selected = self.active_id();
            if was_active {
                let current = self.current.clone();
                let loose: Vec<&Tab> = self
                    .row(&current)
                    .into_iter()
                    .filter(|t| !t.sleeping)
                    .collect();
                let pool: Vec<&Tab> = if loose.is_empty() {
                    self.visible().into_iter().filter(|t| t.id != *id).collect()
                } else {
                    loose
                };
                selected = pool
                    .iter()
                    .max_by(|a, b| a.seen.0.total_cmp(&b.seen.0))
                    .map(|t| t.id.clone());
                match &selected {
                    Some(s) => {
                        let s = s.clone();
                        self.select(&s);
                    }
                    None => self.set_active(None),
                }
            }
            return CloseOutcome::PutDown { selected };
        }
        let visible_before = self.visible_ids(&self.current);
        let visible_index = visible_before.iter().position(|t| t == id);
        if tab.is_pinned() {
            self.pins.retain(|p| p != id);
            self.changed(BrowserEvent::RowChanged { space: None });
        } else if let Some(row) = self.row_mut(&tab.space) {
            if let Some(index) = row.tabs.iter().position(|t| t == id) {
                row.tabs.remove(index);
                if !tab.private && is_restorable_url(&tab.url) {
                    self.closed.push(ClosedTab {
                        url: tab.url.clone(),
                        title: tab.title.clone(),
                        space: tab.space.clone(),
                        index,
                        group: tab.group.clone(),
                    });
                    if self.closed.len() > CLOSED_CAP {
                        self.closed.remove(0);
                    }
                }
            }
            self.changed(BrowserEvent::RowChanged {
                space: Some(tab.space.clone()),
            });
        }
        self.tabs.remove(id);
        for row in &mut self.spaces {
            if row.active.as_ref() == Some(id) {
                row.active = None;
            }
        }
        self.changed(BrowserEvent::TabClosed(id.clone()));
        let mut selected = self.active_id();
        if was_active {
            let visible = self.visible_ids(&self.current);
            selected = visible_index
                .and_then(|i| visible.get(i.min(visible.len().saturating_sub(1))).cloned());
            match &selected {
                Some(s) => {
                    let s = s.clone();
                    self.select(&s);
                }
                None => self.set_active(None),
            }
        }
        CloseOutcome::Closed { selected }
    }

    /// ⌘⇧T: the newest closed tab, back where it was, selected.
    pub fn reopen_closed(&mut self) -> Option<TabId> {
        let ghost = self.closed.pop()?;
        let space = if self.row_of(&ghost.space).is_some() {
            ghost.space.clone()
        } else {
            self.current.clone()
        };
        let id = self.open_tab(
            &ghost.url,
            OpenOptions {
                title: ghost.title,
                space: Some(space),
                index: Some(ghost.index),
                foreground: true,
                ..Default::default()
            },
        );
        if let Some(t) = self.tabs.get_mut(&id) {
            t.group = ghost.group;
        }
        Some(id)
    }

    /// Change a tab's fields (URL, title, favicon, group, saved, split…).
    /// Id, pin and space are managed by their own methods and are restored
    /// if `f` changes them.
    pub fn update_tab(&mut self, id: &TabId, f: impl FnOnce(&mut Tab)) -> bool {
        let Some(t) = self.tabs.get_mut(id) else {
            return false;
        };
        let (keep_id, keep_pin, keep_space) = (t.id.clone(), t.pin.clone(), t.space.clone());
        f(t);
        t.id = keep_id;
        t.pin = keep_pin;
        t.space = keep_space;
        self.changed(BrowserEvent::TabChanged(id.clone()));
        true
    }

    /// The page navigated.
    pub fn set_url(&mut self, id: &TabId, url: &str) -> bool {
        self.update_tab(id, |t| t.url = url.to_string())
    }

    /// The page's title arrived.
    pub fn set_title(&mut self, id: &TabId, title: &str) -> bool {
        self.update_tab(id, |t| t.title = title.to_string())
    }

    /// The page's icon arrived.
    pub fn set_favicon(&mut self, id: &TabId, favicon: Option<&str>) -> bool {
        self.update_tab(id, |t| t.favicon = favicon.map(str::to_string))
    }

    /// Put a tab to sleep (drop its page) or wake it.
    pub fn set_sleeping(&mut self, id: &TabId, sleeping: bool) -> bool {
        self.update_tab(id, |t| t.sleeping = sleeping)
    }

    fn wake(&mut self, id: &TabId) {
        if let Some(t) = self.tabs.get_mut(id)
            && t.sleeping
        {
            t.sleeping = false;
            self.generation += 1;
            self.events.push(BrowserEvent::TabChanged(id.clone()));
        }
    }

    /// Tabs not looked at for `after_secs` go to sleep — not the active one,
    /// not pins, not private ones. Returns the ones put to sleep.
    pub fn sleep_idle(&mut self, now: UnixTime, after_secs: f64) -> Vec<TabId> {
        let active = self.active_id();
        let ids: Vec<TabId> = self
            .tabs
            .values()
            .filter(|t| {
                !t.sleeping && !t.is_pinned() && !t.private && Some(&t.id) != active.as_ref()
            })
            .filter(|t| now.0 - t.seen.0 >= after_secs)
            .map(|t| t.id.clone())
            .collect();
        for id in &ids {
            self.set_sleeping(id, true);
        }
        ids
    }

    /// Pin a tab (to the end of the pins); `letter` defaults to its monogram.
    pub fn pin(&mut self, id: &TabId, letter: Option<&str>) -> bool {
        let Some(tab) = self.tabs.get(id) else {
            return false;
        };
        if tab.is_pinned() || tab.private {
            return false;
        }
        let space = tab.space.clone();
        let letter = letter.map(str::to_string).unwrap_or_else(|| tab.monogram());
        if let Some(row) = self.row_mut(&space) {
            row.tabs.retain(|t| t != id);
        }
        self.pins.push(id.clone());
        if let Some(t) = self.tabs.get_mut(id) {
            t.pin = Some(letter);
            t.saved = true;
        }
        self.changed(BrowserEvent::TabChanged(id.clone()));
        self.changed(BrowserEvent::RowChanged { space: None });
        self.changed(BrowserEvent::RowChanged { space: Some(space) });
        true
    }

    /// Unpin: the tab joins the head of the current space's row.
    pub fn unpin(&mut self, id: &TabId) -> bool {
        if !self.pins.contains(id) {
            return false;
        }
        self.pins.retain(|p| p != id);
        let current = self.current.clone();
        if let Some(t) = self.tabs.get_mut(id) {
            t.pin = None;
            t.space = current.clone();
        }
        if let Some(row) = self.row_mut(&current) {
            row.tabs.insert(0, id.clone());
        }
        self.changed(BrowserEvent::TabChanged(id.clone()));
        self.changed(BrowserEvent::RowChanged { space: None });
        self.changed(BrowserEvent::RowChanged {
            space: Some(current),
        });
        true
    }

    /// Change a pin's letter.
    pub fn set_pin_letter(&mut self, id: &TabId, letter: &str) -> bool {
        let Some(t) = self.tabs.get_mut(id) else {
            return false;
        };
        if t.pin.is_none() || letter.is_empty() {
            return false;
        }
        t.pin = Some(letter.to_string());
        self.changed(BrowserEvent::TabChanged(id.clone()));
        true
    }

    /// Move within its row (or within the pins) to `index`.
    pub fn move_tab(&mut self, id: &TabId, index: usize) -> bool {
        let Some(tab) = self.tabs.get(id) else {
            return false;
        };
        let (list, space) = if tab.is_pinned() {
            (&mut self.pins, None)
        } else {
            let space = tab.space.clone();
            match self.spaces.iter_mut().find(|r| r.space.id == space) {
                Some(r) => (&mut r.tabs, Some(space)),
                None => return false,
            }
        };
        let Some(from) = list.iter().position(|t| t == id) else {
            return false;
        };
        let item = list.remove(from);
        list.insert(index.min(list.len()), item);
        self.changed(BrowserEvent::RowChanged { space });
        true
    }

    /// Move a loose tab to the end of another space's row.
    pub fn move_to_space(&mut self, id: &TabId, space: &str) -> bool {
        let Some(tab) = self.tabs.get(id) else {
            return false;
        };
        if tab.is_pinned() || tab.space == space || self.row_of(space).is_none() {
            return false;
        }
        let from = tab.space.clone();
        let was_active = self.active_id().as_ref() == Some(id);
        if let Some(row) = self.row_mut(&from) {
            row.tabs.retain(|t| t != id);
            if row.active.as_ref() == Some(id) {
                row.active = None;
            }
        }
        if let Some(row) = self.row_mut(space) {
            row.tabs.push(id.clone());
        }
        if let Some(t) = self.tabs.get_mut(id) {
            t.space = space.to_string();
        }
        self.changed(BrowserEvent::RowChanged { space: Some(from) });
        self.changed(BrowserEvent::RowChanged {
            space: Some(space.to_string()),
        });
        if was_active {
            self.ensure_active();
            self.changed(BrowserEvent::Selected(self.active_id()));
        }
        true
    }

    /// Put a tab in a group (or take it out).
    pub fn set_group(&mut self, id: &TabId, group: Option<&str>) -> bool {
        self.update_tab(id, |t| t.group = group.map(str::to_string))
    }

    // MARK: helpers

    fn row_of(&self, space: &str) -> Option<&SpaceRow> {
        self.spaces.iter().find(|r| r.space.id == space)
    }

    fn row_mut(&mut self, space: &str) -> Option<&mut SpaceRow> {
        self.spaces.iter_mut().find(|r| r.space.id == space)
    }

    fn visible_ids(&self, space: &str) -> Vec<TabId> {
        let mut v = self.pins.clone();
        if let Some(r) = self.row_of(space) {
            v.extend(r.tabs.iter().cloned());
        }
        v
    }

    fn set_active(&mut self, id: Option<TabId>) {
        let current = self.current.clone();
        if let Some(row) = self.row_mut(&current) {
            row.active = id.clone();
        }
        self.changed(BrowserEvent::Selected(id));
    }

    /// Make sure the current space has a valid active tab: the remembered
    /// one, else the most recently seen loose tab, else the first visible.
    fn ensure_active(&mut self) {
        if self.active_id().is_some() {
            return;
        }
        let current = self.current.clone();
        let pick = self
            .row(&current)
            .into_iter()
            .max_by(|a, b| a.seen.0.total_cmp(&b.seen.0))
            .map(|t| t.id.clone())
            .or_else(|| self.visible_ids(&current).first().cloned());
        if let Some(row) = self.row_mut(&current) {
            row.active = pick;
        }
    }
}

/// The key two pins are "the same address" by (`Spaces.pinKey`): scheme and
/// host lowercased, fragment dropped.
fn pin_key(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.to_string(),
    }
}

fn tab_from_entry(e: &SessionEntry, space: &str) -> Tab {
    Tab {
        id: TabId::new(),
        url: e.url.clone(),
        title: e.title.clone(),
        favicon: None,
        pin: e.pin.clone(),
        space: space.to_string(),
        group: e.group.clone(),
        saved: e.saved.unwrap_or(true),
        seen: e.seen.unwrap_or_default(),
        split: e.split.clone(),
        sleeping: true,
        private: false,
        extra: e.extra.clone(),
    }
}

fn entry_from_tab(t: &Tab, space: Option<String>) -> SessionEntry {
    SessionEntry {
        url: t.url.clone(),
        title: t.title.clone(),
        pin: t.pin.clone(),
        space,
        active: None,
        group: t.group.clone(),
        saved: if t.pin.is_none() { Some(t.saved) } else { None },
        seen: Some(t.seen),
        split: t.split.clone(),
        extra: t.extra.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(foreground: bool) -> OpenOptions {
        OpenOptions {
            foreground,
            ..Default::default()
        }
    }

    #[test]
    fn open_select_close_reopen() {
        let mut b = Browser::new();
        let a = b.open_tab("https://a.test/", opts(true));
        let c = b.open_tab(
            "https://c.test/",
            OpenOptions {
                at_end: true,
                ..Default::default()
            },
        );
        let m = b.open_tab("https://b.test/", opts(false));
        let urls: Vec<_> = b.visible().iter().map(|t| t.url.clone()).collect();
        assert_eq!(
            urls,
            ["https://a.test/", "https://b.test/", "https://c.test/"]
        );
        assert_eq!(b.active_id(), Some(a.clone()));
        assert_eq!(
            b.close_tab(&a, false),
            CloseOutcome::Closed {
                selected: Some(m.clone())
            }
        );
        assert_eq!(b.closed_tabs().len(), 1);
        let back = b.reopen_closed().unwrap();
        assert_eq!(b.visible()[0].id, back);
        assert_eq!(b.active_id(), Some(back));
        let p = b.open_tab(
            "https://p.test/",
            OpenOptions {
                private: true,
                ..Default::default()
            },
        );
        b.close_tab(&p, false);
        assert!(
            b.closed_tabs().is_empty(),
            "private tabs are not remembered"
        );
        assert!(b.find_tab(&c.short()).is_some());
    }

    #[test]
    fn pins_are_global_and_put_down() {
        let mut b = Browser::new();
        let a = b.open_tab("https://a.test/", opts(true));
        let x = b.open_tab("https://x.test/", opts(false));
        assert!(b.pin(&a, None));
        assert_eq!(b.tab(&a).unwrap().pin.as_deref(), Some("A"));
        let s2 = b.add_space("Work");
        b.switch_space(&s2);
        assert_eq!(b.visible().len(), 1, "the pin shows in every space");
        b.select(&a);
        assert!(matches!(
            b.close_tab(&a, false),
            CloseOutcome::PutDown { .. }
        ));
        assert!(b.tab(&a).unwrap().sleeping);
        b.select(&x);
        assert_ne!(
            b.current_space_id(),
            s2,
            "selecting a tab of another space switches to it"
        );
    }

    #[test]
    fn session_round_trip_keeps_rows_pins_and_active() {
        let mut b = Browser::new();
        let a = b.open_tab("https://a.test/", opts(true));
        b.pin(&a, Some("Z"));
        let w = b.add_space("Work");
        let t = b.open_tab(
            "https://w.test/",
            OpenOptions {
                space: Some(w.clone()),
                foreground: true,
                ..Default::default()
            },
        );
        b.open_tab(
            "https://private.test/",
            OpenOptions {
                private: true,
                ..Default::default()
            },
        );
        let shape = b.to_session();
        assert_eq!(shape.tab_count(), 2);
        let r = Browser::from_session(&shape);
        assert_eq!(r.current_space_id(), w);
        assert_eq!(r.active().unwrap().url, "https://w.test/");
        assert!(!r.active().unwrap().sleeping);
        assert_eq!(r.pins()[0].pin.as_deref(), Some("Z"));
        assert!(r.pins()[0].sleeping);
        assert!(r.tab(&t).is_none(), "ids are fresh per run");
        assert_eq!(r.to_session().tabs.len(), shape.tabs.len());
    }
}
