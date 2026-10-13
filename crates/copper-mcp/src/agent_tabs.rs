//! Which tab a call lands on, and how a tab nobody is looking at is kept
//! workable (Fork/MCP/Backstage.swift `AgentTabs` / `Backstage`).
//!
//! Each agent session (`Who::key`) can hold a tab of its own: the one it
//! opened with `browser_tabs new` / `jev_run newTab`, or chose with
//! `browser_tabs select`. Its calls land there wherever the user is; an
//! agent with no tab of its own works on the one on screen. Any call can
//! name a tab outright with `tab`.
//!
//! A tab worked on off screen is "held" (the host stages it so it paints and
//! takes input) until [`LINGER`] passes with no call, then released.

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::host::{Host, TabId, TabInfo};
use crate::{CallCtx, Failure, Mcp};

/// How long a page stays staged after the agent's last call on it.
pub const LINGER: Duration = Duration::from_secs(5 * 60);

/// A digit string at least this long that starts some tab's id names that
/// tab, not an index (no row holds a thousand tabs).
const ID_DIGITS: usize = 4;

/// `browser_tabs new` and `jev_run newTab`, unless asked to show the tab.
pub fn opens_behind(name: &str, args: &Map<String, Value>) -> bool {
    if args.get("focus").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    match name {
        "browser_tabs" => args.get("action").and_then(Value::as_str) == Some("new"),
        "jev_run" => args.get("newTab").and_then(Value::as_bool) == Some(true),
        _ => false,
    }
}

/// The short id `browser_tabs list` prints.
pub fn short(id: &TabId) -> String {
    id.0.chars().take(8).collect::<String>().to_lowercase()
}

impl<H: Host> Mcp<H> {
    /// A `tab` argument: an id (short or whole) or an index in the agent
    /// window's row. `Ok(None)` when nothing usable was named.
    ///
    /// A JSON number is always an index. A string of digits is ambiguous —
    /// about one short id in fifty is all digits (`72135451`) — so it is an
    /// id when it is long enough to be one ([`ID_DIGITS`]) and some id starts
    /// with it, or when no tab has that index; otherwise an index. The Mac
    /// (`AgentTabs.resolve`) reads every digit string as an index, which
    /// leaves those tabs unreachable.
    pub(crate) fn resolve(
        &self,
        reference: Option<&Value>,
        ctx: &CallCtx,
    ) -> Result<Option<TabInfo>, Failure> {
        let Some(reference) = reference else {
            return Ok(None);
        };
        if let Some(n) = reference.as_i64() {
            return self.tab_at(n, ctx).map(Some);
        }
        if let Some(f) = reference.as_f64() {
            return self.tab_at(f as i64, ctx).map(Some);
        }
        let Some(raw) = reference.as_str() else {
            return Ok(None);
        };
        let raw = raw.trim().to_lowercase();
        if raw.is_empty() {
            return Ok(None);
        }
        let tabs = self.host.tabs();
        let hits: Vec<TabInfo> = tabs
            .iter()
            .filter(|t| t.id.0.to_lowercase().starts_with(&raw))
            .cloned()
            .collect();
        if let Ok(i) = raw.parse::<i64>() {
            let indexed = i >= 0 && tabs.iter().any(|t| t.index == Some(i as usize));
            let id_like = raw.len() >= ID_DIGITS && !hits.is_empty();
            if !id_like && (indexed || hits.is_empty()) {
                return self.tab_at(i, ctx).map(Some);
            }
        }
        match hits.len() {
            1 => Ok(hits.into_iter().next()),
            0 => Err(Failure(format!("no tab {raw} — {}", self.tab_list(ctx)))),
            _ => Err(Failure(format!(
                "{raw} names more than one tab; give more of its id"
            ))),
        }
    }

    fn tab_at(&self, i: i64, ctx: &CallCtx) -> Result<TabInfo, Failure> {
        if i >= 0
            && let Some(tab) = self
                .host
                .tabs()
                .into_iter()
                .find(|t| t.index == Some(i as usize))
        {
            return Ok(tab);
        }
        Err(Failure(format!("no tab {i} — {}", self.tab_list(ctx))))
    }

    /// The caller's own tab, while it still exists.
    pub(crate) fn own(&self, key: &str) -> Option<TabInfo> {
        let id = self.state.borrow().bound.get(key).cloned()?;
        match self.host.tab(&id) {
            Some(tab) => Some(tab),
            None => {
                self.state.borrow_mut().bound.remove(key);
                None
            }
        }
    }

    /// The tab a call works on: the one it named, else the caller's own,
    /// else the one on screen.
    pub(crate) fn target(&self, ctx: &CallCtx) -> Option<TabInfo> {
        if let Some(id) = &ctx.pinned
            && let Some(tab) = self.host.tab(id)
        {
            return Some(tab);
        }
        if let Some(tab) = self.own(&ctx.who.key) {
            return Some(tab);
        }
        self.host.active_tab().and_then(|id| self.host.tab(&id))
    }

    /// Without failing, for the bookkeeping around a call.
    pub(crate) fn peek(&self, args: &Map<String, Value>, ctx: &CallCtx) -> Option<TabInfo> {
        if let Ok(Some(named)) = self.resolve(args.get("tab"), ctx) {
            return Some(named);
        }
        if let Some(tab) = self.own(&ctx.who.key) {
            return Some(tab);
        }
        self.host.active_tab().and_then(|id| self.host.tab(&id))
    }

    /// From now on this caller's calls land on `id`.
    pub(crate) fn bind(&self, id: &TabId, ctx: &CallCtx) {
        if ctx.who.binds() {
            self.state
                .borrow_mut()
                .bound
                .insert(ctx.who.key.clone(), id.clone());
        }
    }

    /// Ready to be worked on: awake, built, staged if nobody is looking.
    /// True when the page's visibility just changed.
    pub(crate) fn prepare(&self, id: &TabId) -> bool {
        let changed = self.host.prepare_tab(id);
        let on_screen = self.host.tab(id).is_some_and(|t| t.selected);
        let mut state = self.state.borrow_mut();
        if on_screen {
            state.held.remove(id);
        } else {
            state.held.insert(id.clone(), Instant::now());
        }
        changed
    }

    /// Before a call reads or drives its tab: stage it, and when that made
    /// the page visible, give the engine a beat.
    pub(crate) async fn ready(&self, ctx: &CallCtx) {
        let Some(tab) = self.target(ctx) else { return };
        if self.prepare(&tab.id) {
            self.host.sleep(Duration::from_millis(120)).await;
        }
    }

    /// The tab this call works on, made ready (`Tools.current`).
    pub(crate) fn current(&self, ctx: &CallCtx) -> Result<TabInfo, Failure> {
        let tab = self.target(ctx).ok_or("no active tab")?;
        self.prepare(&tab.id);
        Ok(self.host.tab(&tab.id).unwrap_or(tab))
    }

    /// Close a tab wherever it is, and let go of it.
    pub(crate) fn close_tab(&self, id: &TabId) {
        if self.state.borrow_mut().held.remove(id).is_some() {
            self.host.release_tab(id);
        }
        self.host.close_tab(id);
    }

    /// Let go of every tab nobody has asked for in [`LINGER`], and of tabs
    /// that are gone. Called by [`Mcp::serve`] every few seconds.
    pub fn sweep(&self) {
        let now = Instant::now();
        let expired: Vec<TabId> = {
            let state = self.state.borrow();
            state
                .held
                .iter()
                .filter(|(_, at)| now.duration_since(**at) >= LINGER)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in expired {
            self.state.borrow_mut().held.remove(&id);
            if self.host.tab(&id).is_some() {
                self.host.release_tab(&id);
            }
        }
        let gone: Vec<TabId> = {
            let state = self.state.borrow();
            state
                .held
                .keys()
                .filter(|id| self.host.tab(id).is_none())
                .cloned()
                .collect()
        };
        let mut state = self.state.borrow_mut();
        for id in gone {
            state.held.remove(&id);
        }
    }

    /// Tabs staged for agents right now.
    pub fn held(&self) -> Vec<TabId> {
        self.state.borrow().held.keys().cloned().collect()
    }

    /// The tab list every tab tool prints (`Tools.tabList`): the agent
    /// window's row by index, then every other tab — other spaces, other
    /// windows, parked — as `- -:`, the line the Mac prints for a caller's
    /// own tab outside the row. (The Mac lists only the row and that one
    /// tab, so an agent in an empty space saw no tabs at all; each listed
    /// tab can be named by its `{tab …}` id.)
    pub fn tab_list(&self, ctx: &CallCtx) -> String {
        let mine = self.target(ctx).map(|t| t.id);
        let (mut listed, elsewhere): (Vec<TabInfo>, Vec<TabInfo>) = self
            .host
            .tabs()
            .into_iter()
            .partition(|t| t.index.is_some());
        listed.sort_by_key(|t| t.index);
        for tab in elsewhere {
            if !listed.iter().any(|t| t.id == tab.id) {
                listed.push(tab);
            }
        }
        if let Some(own) = self.own(&ctx.who.key)
            && !listed.iter().any(|t| t.id == own.id)
        {
            listed.push(own);
        }
        let mut lines = Vec::new();
        for tab in &listed {
            let mut marks = Vec::new();
            if tab.selected {
                marks.push("on screen");
            }
            if Some(&tab.id) == mine.as_ref() {
                marks.push("yours");
            }
            let marker = if marks.is_empty() {
                String::new()
            } else {
                format!("({}) ", marks.join(", "))
            };
            let index = tab
                .index
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".into());
            let group = tab
                .group
                .as_ref()
                .map(|g| format!(" [{g}]"))
                .unwrap_or_default();
            let pin = if tab.pinned { " (pinned)" } else { "" };
            let heat = tab
                .cpu
                .filter(|c| *c >= 10.0)
                .map(|c| format!(" {{cpu {}%}}", c as i64))
                .unwrap_or_default();
            lines.push(format!(
                "- {index}: {marker}[{}] ({}) {{tab {}}}{pin}{group}{heat}",
                tab.title,
                tab.url_or_blank(),
                short(&tab.id)
            ));
        }
        let gpu = self
            .host
            .gpu_percent()
            .filter(|g| *g >= 10.0)
            .map(|g| {
                format!(
                    "\nGPU process (shared by all tabs): {}% of a core",
                    g as i64
                )
            })
            .unwrap_or_default();
        format!("### Open tabs\n{}{gpu}", lines.join("\n"))
    }

    /// `### Page` with the address, title and short id (`Tools.pageLine`).
    pub fn page_line(&self, id: &TabId) -> String {
        let tab = self.host.tab(id).unwrap_or_else(|| TabInfo {
            id: id.clone(),
            ..TabInfo::default()
        });
        let place = if tab.selected {
            ""
        } else {
            " (background — not on the user's screen)"
        };
        format!(
            "### Page\n- URL: {}\n- Title: {}\n- Tab: {}{place}",
            tab.url_or_blank(),
            tab.title,
            short(&tab.id)
        )
    }

    /// Until the page stops loading, plus a beat, or the budget is spent.
    pub(crate) async fn settle(&self, id: &TabId, budget: Duration) {
        let deadline = Instant::now() + budget;
        self.host.sleep(Duration::from_millis(120)).await;
        while Instant::now() < deadline {
            if !self.host.is_loading(id) {
                break;
            }
            self.host.sleep(Duration::from_millis(100)).await;
        }
        self.host.sleep(Duration::from_millis(200)).await;
    }
}
