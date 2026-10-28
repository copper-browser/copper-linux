//! The sync engine (Mac: `CloudSync.swift`, with the document building of
//! `CloudApply.swift`). Six domains, each off until the person turns it on:
//!
//! * `spaces` — the spaces and the tabs kept in them (pins and Saved);
//! * `settings` — the look-and-behaviour settings in the allowed list;
//! * `bookmarks` — the bookmarks tree;
//! * `tabs` — this device's open tabs, for the others to look at (one way:
//!   nothing of another device's ever opens here by itself);
//! * `history` — places visited, append-only;
//! * `canvas` — only a switch: whether the Personal canvas may connect.
//!
//! Pushing: a change here ([`SyncEngine::changed`]) waits two quiet seconds,
//! then the domain's document is built and, if it differs from the last one
//! synced (the base), PUT with that one's version. A 409 hands back the
//! server's copy: merged three ways ([`super::merge`]), applied here, and PUT
//! again (three tries). Pulling: at sign-in, at "Turn on sync", on every
//! reconnect of the event stream, and on each event another device's write
//! causes. Nothing runs while signed out.
//!
//! What is kept: the switches and history cursors in `cloud.json` (`sync`),
//! the bases, the `remote` history places and the canvas switch in
//! `cloud-sync.json` (0600) — the same files, the same shapes, as the Mac.
//! Each kept tab also carries its key in the spaces document on the tab
//! itself (the app: its `session.json` entry), so it is the same entry across
//! launches wherever it was restored — a kept tab's synced address is the one
//! it had when it first synced; neither a redirect nor browsing inside it
//! ever replaces it.

use super::adapter::{BrowserState, LiveKept};
use super::delete::{self, Here, Outcome, Selector};
use super::history::{self, Upload, Visit};
use super::merge;
use crate::client::{Account, CloudClient, CloudStatus, wire_id};
use crate::error::Failure;
use copper_core::bookmarks::Bookmark;
use copper_core::cloud::{
    BookmarksDoc, CloudSyncFile, KeptTab, OpenTab, SettingsDoc, SpacesDoc, SyncBase, TabsDoc,
    decode_base64, encode_base64, encode_doc,
};
use copper_core::date::{UnixTime, now_unix};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// One sync domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Domain {
    Spaces,
    Settings,
    Bookmarks,
    Tabs,
    History,
    Canvas,
}

impl Domain {
    /// Every domain, in the order the Mac lists (and pushes) them.
    pub const ALL: [Domain; 6] = [
        Domain::Spaces,
        Domain::Settings,
        Domain::Bookmarks,
        Domain::Tabs,
        Domain::History,
        Domain::Canvas,
    ];

    /// The server's name for it (and the switch's in `cloud.json`).
    pub fn as_str(self) -> &'static str {
        match self {
            Domain::Spaces => "spaces",
            Domain::Settings => "settings",
            Domain::Bookmarks => "bookmarks",
            Domain::Tabs => "tabs",
            Domain::History => "history",
            Domain::Canvas => "canvas",
        }
    }

    pub fn parse(text: &str) -> Option<Domain> {
        Domain::ALL.into_iter().find(|d| d.as_str() == text)
    }

    /// As the settings page titles it.
    pub fn title(self) -> &'static str {
        match self {
            Domain::Spaces => "Spaces and kept tabs",
            Domain::Settings => "Settings",
            Domain::Bookmarks => "Bookmarks",
            Domain::Tabs => "Open tabs",
            Domain::History => "History",
            Domain::Canvas => "Personal canvas",
        }
    }

    /// As the settings page explains it.
    pub fn detail(self) -> &'static str {
        match self {
            Domain::Spaces => {
                "Your spaces — names, colours, themes, order — and their pinned and Saved tabs. Today's tabs stay on this device."
            }
            Domain::Settings => {
                "How Copper looks and behaves: appearance, sidebar, tab switching and sleep, swipes, blocking, autocorrect, archiving. Never passwords, paths or keys."
            }
            Domain::Bookmarks => "The whole bookmarks tree, folders and all.",
            Domain::Tabs => {
                "Let your other devices see the tabs open here, and open them from there. Nothing opens by itself."
            }
            Domain::History => {
                "Places you visit, so the address field knows them everywhere. Clearing history here, or forgetting a page, deletes it on the cloud too, on a cloud that can."
            }
            Domain::Canvas => {
                "Your Personal canvas follows you between devices. Shared canvases always live on the cloud — that is what sharing means."
            }
        }
    }
}

/// What the status line says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SyncState {
    #[default]
    Idle,
    Syncing,
    Error(String),
    Offline,
}

impl SyncState {
    pub fn text(&self) -> String {
        match self {
            SyncState::Idle => "Up to date".into(),
            SyncState::Syncing => "Syncing…".into(),
            SyncState::Error(m) => m.clone(),
            SyncState::Offline => "Can't reach the cloud — will retry".into(),
        }
    }
}

/// Another device's open tabs.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceTabs {
    /// `tabs:<device id>`.
    pub domain: String,
    pub device_id: Option<String>,
    pub name: String,
    /// Unix seconds.
    pub updated: Option<f64>,
    pub version: i64,
    pub tabs: Vec<OpenTab>,
}

/// Everything the settings page and the MCP show about sync.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncStatus {
    pub on: bool,
    pub enabled: Vec<Domain>,
    pub state: SyncState,
    /// Unix seconds of the last sync the server answered.
    pub last_sync: Option<f64>,
    pub busy: usize,
    /// Each base's version, by domain.
    pub versions: BTreeMap<String, i64>,
    pub history_pushed_through: Option<f64>,
    pub history_pulled_seq: i64,
    /// Unix seconds the next history push waits for, after a failure.
    pub history_retry_at: Option<f64>,
    pub other_devices: Vec<DeviceTabs>,
}

impl SyncStatus {
    /// As the Mac's bench prints it.
    pub fn json(&self) -> Value {
        json!({
            "on": self.on,
            "enabled": self.enabled.iter().map(|d| d.as_str()).collect::<Vec<_>>(),
            "state": self.state.text(),
            "busy": self.busy,
            "lastSync": self.last_sync.map(history::stamp),
            "versions": self.versions,
            "historyPushedThrough": self.history_pushed_through,
            "historyRetryAt": self.history_retry_at.map(history::stamp),
            "historyPulledSeq": self.history_pulled_seq,
            "devices": self.other_devices.iter().map(|d| json!({"name": d.name, "tabs": d.tabs.len(), "domain": d.domain})).collect::<Vec<_>>(),
        })
    }
}

#[derive(Default)]
struct EState {
    on: bool,
    enabled: BTreeSet<Domain>,
    saved: CloudSyncFile,
    account: Option<String>,
    tab_keys: HashMap<String, String>,
    other_devices: Vec<DeviceTabs>,
    busy: usize,
    passes: usize,
    before: (SyncState, u64),
    failure: Option<String>,
    sync_state: SyncState,
    last_sync: Option<f64>,
    log: VecDeque<(f64, String)>,
    waiting: BTreeSet<Domain>,
    debounce: Option<JoinHandle<()>>,
    history_hold: Option<(f64, f64)>,
    history_retry: Option<JoinHandle<()>>,
    forgotten: BTreeMap<String, f64>,
    applying: bool,
    tasks: Vec<JoinHandle<()>>,
}

struct EngineInner {
    client: CloudClient,
    browser: Arc<dyn BrowserState>,
    state: Mutex<EState>,
    status: watch::Sender<SyncStatus>,
    lanes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        if let Ok(s) = self.state.get_mut() {
            for t in s.tasks.drain(..) {
                t.abort();
            }
            if let Some(t) = s.debounce.take() {
                t.abort();
            }
            if let Some(t) = s.history_retry.take() {
                t.abort();
            }
        }
    }
}

/// The sync engine. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct SyncEngine {
    inner: Arc<EngineInner>,
}

impl std::fmt::Debug for SyncEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncEngine")
            .field("status", &self.status())
            .finish()
    }
}

/// Strip what the Mac's decoder would drop, so a document compares the way
/// it would read back there.
trait Doc: Serialize + DeserializeOwned + PartialEq + Clone + Send + Sync + 'static {
    fn normalized(self) -> Self;
}

impl Doc for SpacesDoc {
    fn normalized(mut self) -> Self {
        self.v = 1;
        self.extra.clear();
        for t in &mut self.tabs {
            t.extra.clear();
        }
        self
    }
}

impl Doc for SettingsDoc {
    fn normalized(mut self) -> Self {
        self.v = 1;
        self.extra.clear();
        self
    }
}

fn strip_bookmarks(nodes: &mut [Bookmark]) {
    for n in nodes {
        n.extra.clear();
        if let Some(kids) = n.children.as_mut() {
            strip_bookmarks(kids);
        }
    }
}

impl Doc for BookmarksDoc {
    fn normalized(mut self) -> Self {
        self.v = 1;
        self.extra.clear();
        strip_bookmarks(&mut self.roots);
        self
    }
}

/// How one whole-document domain is built, merged and applied.
struct Ops<T> {
    build: fn(&SyncEngine, Option<&T>) -> T,
    merge: fn(Option<&T>, &T, &T) -> T,
    apply: fn(&SyncEngine, &T, Option<&T>),
}

const SPACES: Ops<SpacesDoc> = Ops {
    build: |e, base| e.build_spaces(base),
    merge: merge::spaces,
    apply: |e, doc, base| e.apply_spaces(doc, base),
};
const SETTINGS: Ops<SettingsDoc> = Ops {
    build: |e, _| SettingsDoc {
        v: 1,
        values: copper_core::settings::filter_cloud_values(&e.inner.browser.settings()),
        extra: Default::default(),
    },
    merge: merge::settings,
    apply: |e, doc, _| {
        e.inner
            .browser
            .apply_settings(&copper_core::settings::filter_cloud_values(&doc.values))
    },
};
const BOOKMARKS: Ops<BookmarksDoc> = Ops {
    build: |e, _| BookmarksDoc {
        v: 1,
        roots: e.inner.browser.bookmarks(),
        extra: Default::default(),
    },
    merge: merge::bookmarks,
    apply: |e, doc, _| {
        if e.inner.browser.bookmarks() != doc.roots {
            e.inner.browser.replace_bookmarks(&doc.roots);
        }
    },
};

/// `GET /v1/sync/docs` row.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct Meta {
    domain: String,
    version: i64,
}

/// `GET /v1/sync/docs/{domain}` (and a 409's body).
#[derive(Debug, Clone, serde::Deserialize)]
struct WireDoc {
    version: i64,
    #[serde(default)]
    payload: Option<String>,
}

#[derive(serde::Deserialize)]
struct Put {
    version: i64,
}

fn is_web(url: &str) -> Option<url::Url> {
    url::Url::parse(url)
        .ok()
        .filter(|u| u.scheme().starts_with("http"))
}

fn decode_payload<T: Doc>(payload: Option<&str>) -> Option<T> {
    let bytes = decode_base64(payload?)?;
    serde_json::from_slice::<T>(&bytes)
        .ok()
        .map(Doc::normalized)
}

fn stem(space: &str, pinned: bool, url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default();
    format!(
        "{}|{}|{host}",
        space.to_lowercase(),
        if pinned { "pin" } else { "saved" }
    )
}

impl SyncEngine {
    /// The engine over `client` and the browser's state. Reads
    /// `cloud-sync.json`; nothing is sent until [`SyncEngine::start`] or a
    /// sync is asked for.
    pub fn new(client: CloudClient, browser: Arc<dyn BrowserState>) -> Self {
        let saved = client
            .store()
            .load::<CloudSyncFile>()
            .map(|l| l.value)
            .unwrap_or_default();
        let prefs = client.sync_prefs();
        let account = client.account().map(|a| a.user_id.to_ascii_uppercase());
        let mut enabled = BTreeSet::new();
        for (on, d) in [
            (prefs.spaces, Domain::Spaces),
            (prefs.settings, Domain::Settings),
            (prefs.bookmarks, Domain::Bookmarks),
            (prefs.tabs, Domain::Tabs),
            (prefs.history, Domain::History),
            (saved.canvas.unwrap_or(false), Domain::Canvas),
        ] {
            if on {
                enabled.insert(d);
            }
        }
        let reset =
            matches!((&account, &saved.owner), (Some(a), Some(o)) if !a.eq_ignore_ascii_case(o));
        let state = EState {
            on: prefs.on,
            enabled,
            saved,
            account,
            ..Default::default()
        };
        let (status, _) = watch::channel(SyncStatus::default());
        let engine = SyncEngine {
            inner: Arc::new(EngineInner {
                client,
                browser,
                state: Mutex::new(state),
                status,
                lanes: Mutex::new(HashMap::new()),
            }),
        };
        if reset {
            engine.reset_state();
        }
        engine.publish();
        engine
    }

    /// Listen to the client (account changes, events) and, when signed in
    /// with sync on, sync a moment after launch. Call once, inside a tokio
    /// runtime.
    pub fn start(&self) {
        let mut events = self.inner.client.subscribe();
        let weak = Arc::downgrade(&self.inner);
        let listen = tokio::spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return,
                };
                let Some(inner) = weak.upgrade() else { return };
                SyncEngine { inner }.received(&event.kind, &event.data);
            }
        });
        let mut status = self.inner.client.watch_status();
        let weak = Arc::downgrade(&self.inner);
        let watch = tokio::spawn(async move {
            while status.changed().await.is_ok() {
                let now = status.borrow_and_update().clone();
                let Some(inner) = weak.upgrade() else { return };
                SyncEngine { inner }.cloud_changed(&now);
            }
        });
        let me = self.clone();
        let first = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if me.inner.client.is_signed_in() && me.is_on() {
                me.sync_now().await;
            }
        });
        self.with(|s| s.tasks.extend([listen, watch, first]));
    }

    // MARK: state

    fn with<R>(&self, f: impl FnOnce(&mut EState) -> R) -> R {
        let mut g = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g)
    }

    fn publish(&self) {
        let prefs = self.inner.client.sync_prefs();
        let status = self.with(|s| SyncStatus {
            on: s.on,
            enabled: s.enabled.iter().copied().collect(),
            state: s.sync_state.clone(),
            last_sync: s.last_sync,
            busy: s.busy,
            versions: s
                .saved
                .docs
                .iter()
                .map(|(k, v)| (k.clone(), v.version))
                .collect(),
            history_pushed_through: prefs.pushed_through.map(|t| t.0),
            history_pulled_seq: prefs.pulled_seq,
            history_retry_at: s.history_hold.map(|h| h.0),
            other_devices: s.other_devices.clone(),
        });
        self.inner.status.send_if_modified(|old| {
            if *old == status {
                false
            } else {
                *old = status;
                true
            }
        });
    }

    /// Now.
    pub fn status(&self) -> SyncStatus {
        self.publish();
        self.inner.status.borrow().clone()
    }

    /// Every change to [`SyncEngine::status`].
    pub fn watch(&self) -> watch::Receiver<SyncStatus> {
        self.inner.status.subscribe()
    }

    /// The cloud's own log, a few hundred lines, no secrets.
    pub fn log(&self) -> Vec<(f64, String)> {
        self.with(|s| s.log.iter().cloned().collect())
    }

    /// The client this engine syncs through.
    pub fn client(&self) -> &CloudClient {
        &self.inner.client
    }

    fn note(&self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!(target: "copper_cloud::sync", "{text}");
        self.with(|s| {
            s.log.push_back((now_unix(), text));
            while s.log.len() > 200 {
                s.log.pop_front();
            }
        });
    }

    fn save_state(&self) {
        let file = self.with(|s| {
            if let Some(a) = &s.account {
                s.saved.owner = Some(a.clone());
            }
            s.saved.clone()
        });
        if let Err(e) = self.inner.client.store().save(&file) {
            tracing::error!(error = %e, "cloud-sync.json not written");
        }
        self.publish();
    }

    fn reset_state(&self) {
        self.with(|s| {
            s.saved.docs.clear();
            s.saved.remote.clear();
            s.saved.owner = s.account.clone();
        });
        self.inner.client.update_sync_prefs(|p| {
            p.pushed_through = None;
            p.pulled_seq = 0;
        });
        self.save_state();
    }

    fn base<T: Doc>(&self, domain: &str) -> Option<T> {
        self.with(|s| s.saved.docs.get(domain).cloned())
            .and_then(|b| b.decode::<T>().ok())
            .map(Doc::normalized)
    }

    fn base_version(&self, domain: &str) -> Option<i64> {
        self.with(|s| s.saved.docs.get(domain).map(|b| b.version))
    }

    fn set_base(&self, domain: &str, base: Option<SyncBase>) {
        self.with(|s| match base {
            Some(b) => {
                s.saved.docs.insert(domain.to_string(), b);
            }
            None => {
                s.saved.docs.remove(domain);
            }
        });
        self.save_state();
    }

    async fn lane(&self, name: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut lanes = self.inner.lanes.lock().unwrap_or_else(|e| e.into_inner());
            lanes.entry(name.to_string()).or_default().clone()
        };
        lock.lock_owned().await
    }

    fn applying(&self, f: impl FnOnce()) {
        self.with(|s| s.applying = true);
        f();
        self.with(|s| s.applying = false);
    }

    // MARK: switches

    /// Sync on, and this domain switched on — whether or not anyone is
    /// signed in right now.
    pub fn syncs(&self, domain: Domain) -> bool {
        self.with(|s| s.on && s.enabled.contains(&domain))
    }

    /// Signed in, sync on, and this domain switched on.
    pub fn active(&self, domain: Domain) -> bool {
        self.inner.client.is_signed_in() && self.syncs(domain)
    }

    /// "Turn on sync" was pressed.
    pub fn is_on(&self) -> bool {
        self.with(|s| s.on)
    }

    /// The chosen domains.
    pub fn enabled(&self) -> BTreeSet<Domain> {
        self.with(|s| s.enabled.clone())
    }

    fn store_switches(&self, domains: &BTreeSet<Domain>, on: bool) {
        self.inner.client.update_sync_prefs(|p| {
            p.spaces = domains.contains(&Domain::Spaces);
            p.settings = domains.contains(&Domain::Settings);
            p.bookmarks = domains.contains(&Domain::Bookmarks);
            p.tabs = domains.contains(&Domain::Tabs);
            p.history = domains.contains(&Domain::History);
            p.on = on;
        });
        let canvas = domains.contains(&Domain::Canvas);
        let save = self.with(|s| {
            s.on = on;
            s.enabled = domains.clone();
            if s.saved.canvas.unwrap_or(false) != canvas {
                s.saved.canvas = Some(canvas);
                true
            } else {
                false
            }
        });
        if save {
            self.save_state();
        }
        self.publish();
    }

    /// One switch. Once sync is on, turning a domain on syncs it now; turning
    /// Open tabs off clears this device's list on the server.
    pub async fn set(&self, domain: Domain, value: bool) {
        let mut next = self.enabled();
        if value {
            next.insert(domain);
        } else {
            next.remove(&domain);
        }
        if next == self.enabled() {
            return;
        }
        let was_on = self.active(domain);
        self.store_switches(&next, self.is_on());
        self.note(format!(
            "{}: {}",
            domain.title(),
            if value { "on" } else { "off" }
        ));
        if !self.inner.client.is_signed_in() || !self.is_on() {
            return;
        }
        if value {
            self.sync(domain).await;
        } else if was_on && domain == Domain::Tabs {
            self.clear_tabs().await;
        }
    }

    /// "Turn on sync": these domains, and a first sync of each.
    pub async fn turn_on(&self, domains: &[Domain]) {
        let set: BTreeSet<Domain> = domains.iter().copied().collect();
        self.store_switches(&set, true);
        self.note(format!(
            "Sync on: {}",
            Domain::ALL
                .iter()
                .filter(|d| set.contains(d))
                .map(|d| d.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        self.sync_now().await;
    }

    /// Sync off; the switches are remembered. Open tabs are cleared on the
    /// server so other devices stop showing a stale list.
    pub async fn turn_off(&self) {
        let had_tabs = self.active(Domain::Tabs);
        let enabled = self.enabled();
        self.store_switches(&enabled, false);
        self.note("Sync off");
        self.with(|s| s.sync_state = SyncState::Idle);
        self.publish();
        if had_tabs {
            self.clear_tabs_forced().await;
        }
    }

    /// Before signing out by hand: clear this device's open tabs from the
    /// server while the session still works, and turn sync off.
    pub async fn signing_out(&self) {
        if self.active(Domain::Tabs) {
            self.clear_tabs().await;
        }
        if self.is_on() {
            let enabled = self.enabled();
            self.store_switches(&enabled, false);
        }
    }

    /// [`SyncEngine::signing_out`], then the client's sign-out.
    pub async fn sign_out(&self) {
        self.signing_out().await;
        self.inner.client.sign_out().await;
    }

    fn cloud_changed(&self, status: &CloudStatus) {
        let now = status
            .account
            .as_ref()
            .map(|a: &Account| a.user_id.to_ascii_uppercase());
        let before = self.with(|s| s.account.clone());
        if now == before {
            if !status.linked {
                self.with(|s| s.sync_state = SyncState::Idle);
                self.publish();
            }
            return;
        }
        self.with(|s| {
            s.account = now.clone();
            s.other_devices.clear();
        });
        let Some(now) = now else {
            // Signed out: sync stops; the switches are remembered for next
            // time. Nothing syncs again until "Turn on sync" is pressed.
            if self.is_on() {
                let enabled = self.enabled();
                self.store_switches(&enabled, false);
                self.note("Sync off — signed out");
            }
            self.with(|s| s.sync_state = SyncState::Idle);
            self.publish();
            return;
        };
        let owner = self.with(|s| s.saved.owner.clone());
        if owner.as_deref().map(str::to_ascii_uppercase) != Some(now) {
            self.reset_state();
        }
        if self.is_on() {
            let me = self.clone();
            tokio::spawn(async move { me.sync_now().await });
        }
    }

    // MARK: local changes

    /// Something of this domain changed here (the app calls this on its
    /// model's change notifications). Two quiet seconds, then push.
    pub fn changed(&self, domain: Domain) {
        if self.with(|s| s.applying) {
            return;
        }
        if !(self.active(domain) || (domain == Domain::Spaces && self.active(Domain::Tabs))) {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let me = self.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let due = me.with(|s| std::mem::take(&mut s.waiting));
            for d in Domain::ALL {
                if due.contains(&d) {
                    me.push(d).await;
                }
            }
        });
        self.with(|s| {
            s.waiting.insert(domain);
            if let Some(old) = s.debounce.replace(task) {
                old.abort();
            }
        });
    }

    // MARK: events

    fn received(&self, kind: &str, event: &Value) {
        if !self.inner.client.is_signed_in() || !self.is_on() {
            return;
        }
        let mine = self.inner.client.account().map(|a| wire_id(&a.device_id));
        let me = self.clone();
        match kind {
            "ready" | "resync" => {
                tokio::spawn(async move { me.pull_all().await });
            }
            "doc" => {
                let Some(domain) = event.get("domain").and_then(Value::as_str) else {
                    return;
                };
                let from = event
                    .get("device_id")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase);
                if from.is_some() && from == mine {
                    return;
                }
                if domain.starts_with("tabs:") {
                    tokio::spawn(async move { me.refresh_devices().await });
                } else if let Some(d) = Domain::parse(domain)
                    && self.active(d)
                {
                    tokio::spawn(async move { me.pull(d).await });
                }
            }
            "history" if self.active(Domain::History) => {
                tokio::spawn(async move { me.pull_history().await });
            }
            _ => {}
        }
    }

    // MARK: syncing

    /// Everything that is on: pull, merge, push.
    pub async fn sync_now(&self) {
        if !self.inner.client.is_signed_in() {
            self.with(|s| s.sync_state = SyncState::Idle);
            self.publish();
            return;
        }
        if !self.is_on() {
            return;
        }
        self.with(|s| {
            if s.passes == 0 && s.busy == 0 {
                Self::begin(s, self.inner.client.answers());
            }
            s.passes += 1;
        });
        self.pull_all().await;
        for d in Domain::ALL {
            if self.active(d) && d != Domain::History {
                self.push(d).await;
            }
        }
        if self.active(Domain::History) {
            self.push_history(true).await;
        }
        let settle = self.with(|s| {
            s.passes -= 1;
            s.passes == 0 && s.busy == 0
        });
        if settle {
            self.settle();
        }
    }

    fn begin(s: &mut EState, answers: u64) {
        s.before = (
            if s.sync_state == SyncState::Syncing {
                SyncState::Idle
            } else {
                s.sync_state.clone()
            },
            answers,
        );
    }

    /// `settle`, as a rule (Mac: `CloudSync.settled`).
    pub fn settled(
        signed_in: bool,
        failure: Option<&str>,
        answered: bool,
        before: &SyncState,
    ) -> (SyncState, bool) {
        if !signed_in {
            return (SyncState::Idle, false);
        }
        if let Some(f) = failure {
            return (
                if f == "offline" {
                    SyncState::Offline
                } else {
                    SyncState::Error(f.to_string())
                },
                false,
            );
        }
        if answered {
            return (SyncState::Idle, true);
        }
        (
            if *before == SyncState::Syncing {
                SyncState::Idle
            } else {
                before.clone()
            },
            false,
        )
    }

    fn settle(&self) {
        let signed_in = self.inner.client.is_signed_in();
        let answers = self.inner.client.answers();
        self.with(|s| {
            let (state, stamp) = Self::settled(
                signed_in,
                s.failure.as_deref(),
                answers > s.before.1,
                &s.before.0,
            );
            s.sync_state = state;
            if stamp {
                s.last_sync = Some(now_unix());
            }
            s.failure = None;
        });
        self.publish();
    }

    /// One piece of network work: counted for the status line, failures
    /// turned into it.
    async fn work<F, Fut>(&self, name: &str, body: F) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), Failure>>,
    {
        let answers = self.inner.client.answers();
        self.with(|s| {
            if s.busy == 0 && s.passes == 0 {
                Self::begin(s, answers);
            }
            s.busy += 1;
            s.sync_state = SyncState::Syncing;
        });
        self.publish();
        let result = body().await;
        let ok = match result {
            Ok(()) => true,
            Err(e) => {
                self.with(|s| {
                    s.failure = Some(if e.is_network() {
                        "offline".to_string()
                    } else {
                        e.message.clone()
                    })
                });
                self.note(format!("{name}: {}", e.message));
                false
            }
        };
        let settle = self.with(|s| {
            s.busy -= 1;
            s.busy == 0 && s.passes == 0
        });
        if settle {
            self.settle();
        }
        ok
    }

    async fn pull_all(&self) {
        if !self.inner.client.is_signed_in() || !self.is_on() {
            return;
        }
        let me = self.clone();
        self.work("docs", || async move {
            let list: Vec<Meta> = me
                .inner
                .client
                .request_json("GET", "/v1/sync/docs", None, &[])
                .await?;
            for d in [Domain::Spaces, Domain::Settings, Domain::Bookmarks] {
                if !me.active(d) {
                    continue;
                }
                let server = list.iter().find(|m| m.domain == d.as_str());
                if server.is_none() || server.map(|m| m.version) != me.base_version(d.as_str()) {
                    me.pull(d).await;
                }
            }
            me.refresh_devices_known(Some(list)).await;
            if me.active(Domain::History) {
                me.pull_history().await;
            }
            Ok(())
        })
        .await;
    }

    async fn sync(&self, domain: Domain) {
        match domain {
            Domain::History => {
                self.pull_history().await;
                self.push_history(true).await;
            }
            Domain::Tabs => {
                self.push(Domain::Tabs).await;
                self.refresh_devices().await;
            }
            Domain::Canvas => {}
            _ => {
                self.pull(domain).await;
                self.push(domain).await;
            }
        }
    }

    /// Pull a shared domain: take the server's copy, merged with anything
    /// changed here since the last sync.
    pub async fn pull(&self, domain: Domain) {
        if !self.active(domain) {
            return;
        }
        match domain {
            Domain::Spaces => self.pull_doc(domain, SPACES).await,
            Domain::Settings => self.pull_doc(domain, SETTINGS).await,
            Domain::Bookmarks => self.pull_doc(domain, BOOKMARKS).await,
            Domain::History => self.pull_history().await,
            Domain::Tabs => self.refresh_devices().await,
            Domain::Canvas => {}
        }
    }

    /// Push a domain if it changed here since the last sync.
    pub async fn push(&self, domain: Domain) {
        match domain {
            Domain::Spaces => {
                if self.active(Domain::Spaces) {
                    self.push_doc(domain, SPACES).await;
                }
                if self.active(Domain::Tabs) {
                    self.push_tabs().await;
                }
            }
            Domain::Settings if self.active(domain) => self.push_doc(domain, SETTINGS).await,
            Domain::Bookmarks if self.active(domain) => self.push_doc(domain, BOOKMARKS).await,
            Domain::Tabs if self.active(domain) => self.push_tabs().await,
            Domain::History if self.active(domain) => self.push_history(false).await,
            _ => {}
        }
    }

    async fn pull_doc<T: Doc>(&self, domain: Domain, ops: Ops<T>) {
        let _lane = self.lane(domain.as_str()).await;
        let me = self.clone();
        let name = domain.as_str();
        self.work(name, || async move {
            let path = format!("/v1/sync/docs/{name}");
            let doc: WireDoc = match me.inner.client.request_json("GET", &path, None, &[]).await {
                Ok(d) => d,
                Err(f) if f.status == 404 => {
                    // Nothing there yet: this device's copy goes up as the first.
                    me.set_base(name, None);
                    return Ok(());
                }
                Err(f) => return Err(f),
            };
            if !me.active(domain) {
                return Ok(());
            }
            if Some(doc.version) == me.base_version(name) {
                return Ok(());
            }
            let Some(server) = decode_payload::<T>(doc.payload.as_deref()) else {
                return Err(Failure::new(
                    0,
                    "decode",
                    format!("The cloud's {name} didn't read — left alone"),
                ));
            };
            let base = me.base::<T>(name);
            let local = (ops.build)(&me, base.as_ref()).normalized();
            let merged = match &base {
                Some(b) if local == *b => server.clone(),
                _ => (ops.merge)(base.as_ref(), &local, &server),
            };
            if merged != local {
                me.applying(|| (ops.apply)(&me, &merged, base.as_ref()));
            }
            me.set_base(
                name,
                Some(SyncBase::from_doc(doc.version, &server).map_err(encode_failure)?),
            );
            me.note(format!(
                "Pulled {name} v{}{}",
                doc.version,
                if merged == local { "" } else { " — applied" }
            ));
            if merged != server {
                me.put(domain, merged, &ops).await?;
            }
            Ok(())
        })
        .await;
    }

    async fn push_doc<T: Doc>(&self, domain: Domain, ops: Ops<T>) {
        let _lane = self.lane(domain.as_str()).await;
        let name = domain.as_str();
        let base = self.base::<T>(name);
        let local = (ops.build)(self, base.as_ref()).normalized();
        if base.as_ref() == Some(&local) {
            return;
        }
        let me = self.clone();
        self.work(name, || async move { me.put(domain, local, &ops).await })
            .await;
    }

    /// PUT against the base's version; on 409 merge with the server's copy,
    /// apply, and try again (three times at most).
    async fn put<T: Doc>(&self, domain: Domain, first: T, ops: &Ops<T>) -> Result<(), Failure> {
        let name = domain.as_str();
        let path = format!("/v1/sync/docs/{name}");
        let mut doc = first;
        for _ in 0..3 {
            if !self.active(domain) {
                return Ok(());
            }
            let version = self.base_version(name).unwrap_or(0);
            let payload = encode_doc(&doc).map_err(encode_failure)?;
            let body = json!({"base_version": version, "payload": encode_base64(&payload)});
            match self
                .inner
                .client
                .request_json::<Put>("PUT", &path, Some(body), &[])
                .await
            {
                Ok(answer) => {
                    self.set_base(
                        name,
                        Some(SyncBase {
                            version: answer.version,
                            payload: encode_base64(&payload),
                            extra: Default::default(),
                        }),
                    );
                    self.note(format!("Pushed {name} v{}", answer.version));
                    return Ok(());
                }
                Err(f) if f.status == 409 => {
                    let mut current = f
                        .body
                        .as_deref()
                        .and_then(|b| serde_json::from_slice::<WireDoc>(b).ok());
                    if current
                        .as_ref()
                        .is_some_and(|c| c.payload.is_none() && c.version != 0)
                    {
                        current = self
                            .inner
                            .client
                            .request_json("GET", &path, None, &[])
                            .await
                            .ok();
                    }
                    let Some((current, server)) = current.and_then(|c| {
                        let s = decode_payload::<T>(c.payload.as_deref())?;
                        Some((c, s))
                    }) else {
                        // Gone since: start over from nothing.
                        self.set_base(name, None);
                        continue;
                    };
                    let base = self.base::<T>(name);
                    let local = (ops.build)(self, base.as_ref()).normalized();
                    let merged = (ops.merge)(base.as_ref(), &local, &server);
                    if merged != local {
                        self.applying(|| (ops.apply)(self, &merged, base.as_ref()));
                    }
                    self.set_base(
                        name,
                        Some(SyncBase::from_doc(current.version, &server).map_err(encode_failure)?),
                    );
                    self.note(format!(
                        "Merged {name} with v{} from another device",
                        current.version
                    ));
                    if merged == server {
                        return Ok(());
                    }
                    doc = merged;
                }
                Err(f) => return Err(f),
            }
        }
        Err(Failure::new(
            409,
            "conflict",
            format!(
                "{} kept changing elsewhere — will try again",
                domain.title()
            ),
        ))
    }

    // MARK: spaces, built and applied

    fn live_kept(&self) -> Vec<LiveKept> {
        self.inner
            .browser
            .kept_tabs()
            .into_iter()
            .filter(|t| is_web(&t.url).is_some())
            .collect()
    }

    /// At launch, before any page has loaded: give every restored kept tab
    /// its key in the last spaces document synced — matched by the address it
    /// was restored with — and keep that key on the tab. From then on a
    /// redirect when it wakes, or browsing inside it, never makes it a
    /// different entry, in this run or the next. Local only: nothing is sent,
    /// and it runs whether or not spaces sync is on (so turning it back on
    /// later finds every tab still keyed).
    pub fn remember_kept_tabs(&self) {
        if let Some(base) = self.base::<SpacesDoc>(Domain::Spaces.as_str()) {
            self.build_spaces(Some(&base));
        }
    }

    /// This device's spaces document. `base` is the last one synced: a tab it
    /// already had keeps that entry's key, address and title (so browsing
    /// inside a pinned tab doesn't send a new copy of every space on every
    /// click). Every tab's key is kept on the tab ([`BrowserState::keep_keys`]),
    /// so that holds across launches too.
    fn build_spaces(&self, base: Option<&SpacesDoc>) -> SpacesDoc {
        // Pins sync one per site (merge::one_pin_per_site): a second pin for
        // a site already pinned here — one an older build opened from another
        // device's copy — closes before this device says what it has.
        let mut seen: HashSet<String> = HashSet::new();
        let (list, extra): (Vec<LiveKept>, Vec<LiveKept>) = self
            .live_kept()
            .into_iter()
            .partition(|t| !t.pinned || seen.insert(merge::site(&t.url)));
        for t in &extra {
            self.inner.browser.close_kept(&t.tab_id);
        }
        let base_by: HashMap<&str, &KeptTab> = base
            .map(|b| b.tabs.iter().map(|t| (t.key.as_str(), t)).collect())
            .unwrap_or_default();
        let known = self.with(|s| s.tab_keys.clone());
        let mut used: HashSet<String> = HashSet::new();
        let mut key_for: HashMap<String, String> = HashMap::new();
        // 1. Keys this run already knows, then the key a tab carries from an
        // earlier run while the base still has it — each while it still
        // describes the tab: same space and section. A kept tab that wandered
        // off to another site (a redirect when it woke, browsing) is still
        // the same entry; one moved to another space or section is not.
        let carried = |item: &LiveKept| {
            item.key
                .as_ref()
                .filter(|k| base_by.contains_key(k.as_str()))
                .cloned()
        };
        for from_this_run in [true, false] {
            for item in &list {
                if key_for.contains_key(&item.tab_id) {
                    continue;
                }
                let key = if from_this_run {
                    known.get(&item.tab_id).cloned()
                } else {
                    carried(item)
                };
                if let Some(key) = key
                    && !used.contains(&key)
                    && key.starts_with(&format!(
                        "{}|{}|",
                        item.space.to_lowercase(),
                        if item.pinned { "pin" } else { "saved" }
                    ))
                {
                    key_for.insert(item.tab_id.clone(), key.clone());
                    used.insert(key);
                }
            }
        }
        // 2. The base's entries: same address first, then same site.
        for exact in [true, false] {
            for item in &list {
                if key_for.contains_key(&item.tab_id) {
                    continue;
                }
                let s = stem(&item.space, item.pinned, &item.url);
                if let Some(entry) = base.and_then(|b| {
                    b.tabs.iter().find(|e| {
                        !used.contains(&e.key)
                            && e.key.starts_with(&format!("{s}|"))
                            && (!exact || e.url == item.url)
                    })
                }) {
                    key_for.insert(item.tab_id.clone(), entry.key.clone());
                    used.insert(entry.key.clone());
                }
            }
        }
        // 3. New ones: the site and the address.
        for item in &list {
            if key_for.contains_key(&item.tab_id) {
                continue;
            }
            let s = stem(&item.space, item.pinned, &item.url);
            let mut key = format!("{s}|{}", item.url);
            let mut n = 2;
            while used.contains(&key) {
                key = format!("{s}|{}#{n}", item.url);
                n += 1;
            }
            key_for.insert(item.tab_id.clone(), key.clone());
            used.insert(key);
        }
        self.with(|s| s.tab_keys = key_for.clone());
        let changed: Vec<(String, String)> = list
            .iter()
            .filter_map(|item| {
                let key = key_for.get(&item.tab_id)?;
                (item.key.as_ref() != Some(key)).then(|| (item.tab_id.clone(), key.clone()))
            })
            .collect();
        if !changed.is_empty() {
            self.inner.browser.keep_keys(&changed);
        }
        let mut tabs: Vec<KeptTab> = list
            .iter()
            .filter_map(|item| {
                let key = key_for.get(&item.tab_id)?.clone();
                Some(match base_by.get(key.as_str()) {
                    Some(was) => KeptTab {
                        key,
                        space: item.space.clone(),
                        url: was.url.clone(),
                        title: was.title.clone(),
                        pinned: item.pinned,
                        pin: item.pin.clone().or_else(|| was.pin.clone()),
                        group: item.group.clone(),
                        extra: Default::default(),
                    },
                    None => KeptTab {
                        key,
                        space: item.space.clone(),
                        url: item.url.clone(),
                        title: item.title.clone(),
                        pinned: item.pinned,
                        pin: item.pin.clone(),
                        group: item.group.clone(),
                        extra: Default::default(),
                    },
                })
            })
            .collect();
        // Sorted, not in row order: dragging a pin along the row is this
        // device's business.
        tabs.sort_by(|a, b| a.key.cmp(&b.key));
        SpacesDoc {
            v: 1,
            spaces: self.inner.browser.spaces(),
            tabs,
            groups: self.inner.browser.groups(),
            extra: Default::default(),
        }
    }

    /// Make this device's spaces the merged document's: spaces added, edited,
    /// ordered, and those gone removed; kept tabs opened (asleep) or closed.
    fn apply_spaces(&self, doc: &SpacesDoc, base: Option<&SpacesDoc>) {
        let browser = &self.inner.browser;
        let wanted: HashSet<String> = doc
            .spaces
            .iter()
            .map(|s| s.id.to_ascii_uppercase())
            .collect();
        if base.is_none() {
            for (old, new) in merge::same_named(&browser.spaces(), &doc.spaces) {
                if !wanted.contains(&new.to_ascii_uppercase()) {
                    continue;
                }
                browser.rekey_space(&old, &new);
                let (prefix, to) = (old.to_lowercase(), new.to_lowercase());
                self.with(|s| {
                    for key in s.tab_keys.values_mut() {
                        if let Some(rest) = key.strip_prefix(&prefix) {
                            *key = format!("{to}{rest}");
                        }
                    }
                });
            }
        }
        let local = self.build_spaces(base);
        let local_keys: HashSet<String> = local.tabs.iter().map(|t| t.key.clone()).collect();
        let doc_keys: HashSet<&str> = doc.tabs.iter().map(|t| t.key.as_str()).collect();
        if !doc.spaces.is_empty() {
            browser.adopt_spaces(&doc.spaces);
        }
        // Kept tabs gone from the document — only ones the base had — close.
        let by_key: HashMap<String, String> = self.with(|s| {
            s.tab_keys
                .iter()
                .map(|(tab, key)| (key.clone(), tab.clone()))
                .collect()
        });
        let live: HashSet<String> = self.live_kept().into_iter().map(|t| t.tab_id).collect();
        let base_keys: HashSet<&str> = base
            .map(|b| b.tabs.iter().map(|t| t.key.as_str()).collect())
            .unwrap_or_default();
        // A pin whose site the document still pins under another key is not
        // gone: it takes that key below rather than closing and reopening.
        let doc_pin_sites: HashSet<String> = doc
            .tabs
            .iter()
            .filter(|t| t.pinned)
            .map(|t| merge::site(&t.url))
            .collect();
        for t in &local.tabs {
            if !doc_keys.contains(t.key.as_str())
                && base_keys.contains(t.key.as_str())
                && !(t.pinned && doc_pin_sites.contains(&merge::site(&t.url)))
                && let Some(tab) = by_key.get(&t.key)
                && live.contains(tab)
            {
                browser.close_kept(tab);
            }
        }
        // Kept tabs new in the document: open them, asleep, each with its
        // entry's key — kept on the tab at once, so even one that redirects
        // the first time it wakes (and is quit before anything else syncs)
        // comes back as the same entry.
        let mut opened: Vec<(String, String)> = vec![];
        // Pins are global, but older Macs keyed them by the space on screen,
        // so a document can hold one pin under several spaces' keys (Mac:
        // Copper#92). A pin for a site already pinned here is that pin: take
        // its key when the pin's own is gone from the document, and never
        // pin the site twice.
        // shortcut: matched by host, so two pins on one site sync as one; upgrade when pins carry an id.
        let host = |u: &str| is_web(u).and_then(|u| u.host_str().map(str::to_lowercase));
        let mut keys_now = self.with(|s| s.tab_keys.clone());
        let mut pinned_here: Vec<(String, Option<String>)> = self
            .live_kept()
            .into_iter()
            .filter(|t| t.pinned)
            .map(|t| (t.tab_id, host(&t.url)))
            .collect();
        for entry in &doc.tabs {
            if local_keys.contains(&entry.key) {
                continue;
            }
            let Some(url) = is_web(&entry.url) else {
                continue;
            };
            if entry.pinned
                && let Some(site) = host(&entry.url)
                && pinned_here
                    .iter()
                    .any(|(_, h)| h.as_deref() == Some(site.as_str()))
            {
                let free = pinned_here.iter().position(|(tab, h)| {
                    h.as_deref() == Some(site.as_str())
                        && keys_now
                            .get(tab)
                            .is_none_or(|k| !doc_keys.contains(k.as_str()))
                });
                if let Some(i) = free {
                    let tab = pinned_here[i].0.clone();
                    keys_now.insert(tab.clone(), entry.key.clone());
                    self.with(|s| s.tab_keys.insert(tab.clone(), entry.key.clone()));
                    opened.push((tab, entry.key.clone()));
                }
                continue;
            }
            let pin = entry.pinned.then(|| {
                entry.pin.clone().unwrap_or_else(|| {
                    copper_core::address::pretty(&url)
                        .chars()
                        .next()
                        .map(|c| c.to_uppercase().collect())
                        .unwrap_or_default()
                })
            });
            if let Some(tab) =
                browser.open_kept(&entry.url, &entry.title, pin.as_deref(), &entry.space)
            {
                self.with(|s| s.tab_keys.insert(tab.clone(), entry.key.clone()));
                if pin.is_some() {
                    keys_now.insert(tab.clone(), entry.key.clone());
                    pinned_here.push((tab.clone(), host(&entry.url)));
                }
                opened.push((tab, entry.key.clone()));
            }
        }
        if !opened.is_empty() {
            browser.keep_keys(&opened);
        }
        // Folders, then which folder each kept tab is filed in — only when
        // the document says: one from an older build has no folders at all,
        // and must not empty this device's.
        if let Some(folders) = &doc.groups {
            let sent: HashSet<String> = folders.iter().map(|f| f.id.to_ascii_uppercase()).collect();
            let removed: Vec<String> = base
                .and_then(|b| b.groups.as_ref())
                .map(|g| {
                    g.iter()
                        .filter(|f| !sent.contains(&f.id.to_ascii_uppercase()))
                        .map(|f| f.id.clone())
                        .collect()
                })
                .unwrap_or_default();
            browser.adopt_groups(folders, &removed);
            let filed: HashMap<&str, Option<&str>> = doc
                .tabs
                .iter()
                .map(|t| (t.key.as_str(), t.group.as_deref()))
                .collect();
            let keys = self.with(|s| s.tab_keys.clone());
            for item in self.live_kept() {
                let Some(key) = keys.get(&item.tab_id) else {
                    continue;
                };
                let Some(want) = filed.get(key.as_str()) else {
                    continue;
                };
                let same = match (*want, item.group.as_deref()) {
                    (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    browser.file_kept(&item.tab_id, *want);
                }
            }
        }
        // Spaces gone from the document, last.
        if !doc.spaces.is_empty() {
            let synced: HashSet<String> = base
                .map(|b| b.spaces.iter().map(|s| s.id.to_ascii_uppercase()).collect())
                .unwrap_or_default();
            for space in browser.spaces() {
                let id = space.id.to_ascii_uppercase();
                if wanted.contains(&id) || !synced.contains(&id) {
                    continue;
                }
                if browser.spaces().len() <= 1 {
                    break;
                }
                browser.remove_space(&space.id);
            }
        }
    }

    // MARK: open tabs

    fn tabs_domain(&self) -> Option<String> {
        self.inner
            .client
            .account()
            .map(|a| format!("tabs:{}", wire_id(&a.device_id)))
    }

    fn open_tabs_doc(&self) -> TabsDoc {
        TabsDoc {
            v: 1,
            device: self.inner.client.device_name(),
            updated: UnixTime(now_unix()),
            tabs: self
                .inner
                .browser
                .open_tabs()
                .into_iter()
                .filter(|t| is_web(&t.url).is_some())
                .take(500)
                .collect(),
            extra: Default::default(),
        }
    }

    async fn push_tabs(&self) {
        let Some(domain) = self.tabs_domain() else {
            return;
        };
        let _lane = self.lane("tabs").await;
        let doc = self.open_tabs_doc();
        let was = self
            .with(|s| s.saved.docs.get(&domain).cloned())
            .and_then(|b| b.decode::<TabsDoc>().ok());
        if let Some(was) = was
            && was.tabs == doc.tabs
            && was.device == doc.device
        {
            return;
        }
        let me = self.clone();
        self.work("tabs", || async move { me.put_own(&domain, &doc).await })
            .await;
    }

    async fn clear_tabs(&self) {
        let Some(domain) = self
            .tabs_domain()
            .filter(|_| self.inner.client.is_signed_in())
        else {
            return;
        };
        let doc = TabsDoc {
            v: 1,
            device: self.inner.client.device_name(),
            updated: UnixTime(now_unix()),
            tabs: vec![],
            extra: Default::default(),
        };
        let me = self.clone();
        self.work("tabs", || async move { me.put_own(&domain, &doc).await })
            .await;
    }

    async fn clear_tabs_forced(&self) {
        // Sync is off by now: `put_own` doesn't check `active`.
        self.clear_tabs().await;
    }

    /// This device's own document: nobody else writes it, so a 409 only
    /// means the version here is stale — take the server's and go again.
    async fn put_own(&self, domain: &str, doc: &TabsDoc) -> Result<(), Failure> {
        let payload = encode_doc(doc).map_err(encode_failure)?;
        let path = format!("/v1/sync/docs/{domain}");
        for _ in 0..2 {
            let version = self.base_version(domain).unwrap_or(0);
            let body = json!({"base_version": version, "payload": encode_base64(&payload)});
            match self
                .inner
                .client
                .request_json::<Put>("PUT", &path, Some(body), &[])
                .await
            {
                Ok(answer) => {
                    self.set_base(
                        domain,
                        Some(SyncBase {
                            version: answer.version,
                            payload: encode_base64(&payload),
                            extra: Default::default(),
                        }),
                    );
                    return Ok(());
                }
                Err(f) if f.status == 409 => {
                    let version = f
                        .body
                        .as_deref()
                        .and_then(|b| serde_json::from_slice::<WireDoc>(b).ok())
                        .map(|d| d.version)
                        .unwrap_or(0);
                    self.set_base(
                        domain,
                        Some(SyncBase {
                            version,
                            payload: String::new(),
                            extra: Default::default(),
                        }),
                    );
                }
                Err(f) => return Err(f),
            }
        }
        Ok(())
    }

    /// The other devices' open tabs, newest first.
    pub fn other_devices(&self) -> Vec<DeviceTabs> {
        self.with(|s| s.other_devices.clone())
    }

    /// Read the other devices' open tabs again.
    pub async fn refresh_devices(&self) {
        self.refresh_devices_known(None).await;
    }

    async fn refresh_devices_known(&self, known: Option<Vec<Meta>>) {
        if !self.inner.client.is_signed_in() || !self.is_on() {
            return;
        }
        let mine = self.tabs_domain();
        let me = self.clone();
        self.work("devices", || async move {
            let list: Vec<Meta> = match known {
                Some(k) => k,
                None => {
                    me.inner
                        .client
                        .request_json("GET", "/v1/sync/docs", None, &[])
                        .await?
                }
            };
            let have = me.other_devices();
            let mut next = vec![];
            for meta in list {
                if !meta.domain.starts_with("tabs:") || Some(&meta.domain) == mine.as_ref() {
                    continue;
                }
                if let Some(h) = have
                    .iter()
                    .find(|d| d.domain == meta.domain && d.version == meta.version)
                {
                    next.push(h.clone());
                    continue;
                }
                let path = format!("/v1/sync/docs/{}", meta.domain);
                let Ok(doc) = me
                    .inner
                    .client
                    .request_json::<WireDoc>("GET", &path, None, &[])
                    .await
                else {
                    continue;
                };
                let Some(tabs) = doc
                    .payload
                    .as_deref()
                    .and_then(decode_base64)
                    .and_then(|b| serde_json::from_slice::<TabsDoc>(&b).ok())
                else {
                    continue;
                };
                let id = meta.domain.trim_start_matches("tabs:").to_string();
                next.push(DeviceTabs {
                    domain: meta.domain.clone(),
                    device_id: copper_core::ids::is_uuid(&id).then_some(id),
                    name: tabs.device,
                    updated: Some(tabs.updated.0),
                    version: meta.version,
                    tabs: tabs.tabs,
                });
            }
            next.retain(|d| !d.tabs.is_empty());
            next.sort_by(|a, b| {
                b.updated
                    .unwrap_or(f64::MIN)
                    .partial_cmp(&a.updated.unwrap_or(f64::MIN))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            me.with(|s| s.other_devices = next);
            me.publish();
            Ok(())
        })
        .await;
    }

    // MARK: history

    fn local_visits(&self) -> Vec<Visit> {
        self.inner
            .browser
            .visits()
            .into_iter()
            .map(|v| Visit {
                key: v.key,
                url: v.url,
                title: v.title,
                at: v.last,
            })
            .collect()
    }

    /// New places visited here, oldest first, in requests the cloud takes.
    /// After a push fails, the next one waits 30 s, then twice as long each
    /// time up to 15 minutes, unless `force`.
    pub async fn push_history(&self, force: bool) {
        if !self.active(Domain::History) {
            return;
        }
        if !force
            && let Some((until, _)) = self.with(|s| s.history_hold)
            && until > now_unix()
        {
            return;
        }
        let _lane = self.lane("history-push").await;
        let me = self.clone();
        let ok = self
            .work("history", || async move { me.upload_history().await })
            .await;
        self.history_pushed(ok);
    }

    async fn upload_history(&self) -> Result<(), Failure> {
        let limits = self.inner.client.history_limits().await;
        let now = now_unix();
        let through = self
            .inner
            .client
            .sync_prefs()
            .pushed_through
            .map(|t| t.0)
            .unwrap_or(0.0)
            .min(now);
        let forgotten = self.with(|s| s.forgotten.clone());
        let visits = delete::unforgotten(self.local_visits(), &forgotten);
        let remote = self.with(|s| s.saved.remote.clone());
        let plan = history::plan(&visits, through, &remote, now, limits);
        if plan.is_empty() {
            return Ok(());
        }
        let mut upload = Upload::new(plan, limits, through, now);
        let mut told = 0;
        let mut refusals = 0;
        let result = loop {
            let Some(entries) = upload.next_request() else {
                break Ok(());
            };
            if !self.active(Domain::History) {
                break Ok(());
            }
            let count = entries.len();
            match self
                .inner
                .client
                .request_raw("POST", "/v1/sync/history", history::body(&entries), &[])
                .await
            {
                Ok(answer) => upload.accepted(&history::rejected(&answer.body)),
                Err(f) if history::refuses(&f) => {
                    if refusals == 0 {
                        self.note(format!(
                            "History: the cloud refused {count} visit{} ({}: {}) — finding which",
                            if count == 1 { "" } else { "s" },
                            f.status,
                            f.message
                        ));
                    }
                    refusals += 1;
                    if let Err(e) = upload.refused(&f) {
                        break Err(e);
                    }
                }
                Err(f) => break Err(f),
            }
            self.keep_upload(&upload, &mut told);
        };
        self.keep_upload(&upload, &mut told);
        if upload.skipped.len() > 5 {
            self.note(format!(
                "History: {} more visits left out",
                upload.skipped.len() - 5
            ));
        }
        if upload.shortened > 0 {
            self.note(format!(
                "History: shortened {} long title{} to fit the cloud's {} bytes per visit",
                upload.shortened,
                if upload.shortened == 1 { "" } else { "s" },
                upload.limits().entry_bytes
            ));
        }
        if upload.limits() != limits {
            self.inner.client.set_history_limits(upload.limits());
        }
        if upload.sent > 0 {
            self.note(format!(
                "Pushed {} visit{}",
                upload.sent,
                if upload.sent == 1 { "" } else { "s" }
            ));
        }
        result
    }

    /// The cursor as far as the upload says, visits stamped ahead of now
    /// remembered, and what was left out said in the log.
    fn keep_upload(&self, upload: &Upload, told: &mut usize) {
        let through = upload.through();
        self.inner
            .client
            .update_sync_prefs(|p| p.pushed_through = Some(UnixTime(through)));
        self.with(|s| {
            for (key, at) in upload.ahead() {
                s.saved.remote.insert(key, at);
            }
            s.saved.remote.retain(|_, at| *at > through);
        });
        self.save_state();
        for skip in upload
            .skipped
            .iter()
            .skip(*told)
            .take(5_usize.saturating_sub(*told))
        {
            let size = if skip.bytes > 0 {
                format!(", {} bytes", skip.bytes)
            } else {
                String::new()
            };
            self.note(format!(
                "History: left out a visit to {} ({}{size})",
                skip.host, skip.why
            ));
        }
        *told = (*told).max(upload.skipped.len().min(5));
    }

    fn history_pushed(&self, ok: bool) {
        if let Some(t) = self.with(|s| s.history_retry.take()) {
            t.abort();
        }
        if ok || !self.active(Domain::History) {
            self.with(|s| s.history_hold = None);
            self.publish();
            return;
        }
        let wait =
            self.with(|s| (s.history_hold.map(|h| h.1).unwrap_or(15.0) * 2.0).clamp(30.0, 900.0));
        self.with(|s| s.history_hold = Some((now_unix() + wait, wait)));
        self.note(format!("History: trying again in {} s", wait as i64));
        let me = self.clone();
        let retry = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
            me.push_history(true).await;
        });
        self.with(|s| s.history_retry = Some(retry));
        self.publish();
    }

    /// Places other devices visited since the last pull, merged into history
    /// — once each: a place already known within the same second is skipped.
    pub async fn pull_history(&self) {
        if !self.active(Domain::History) {
            return;
        }
        let _lane = self.lane("history-pull").await;
        let me = self.clone();
        self.work("history", || async move { me.download_history().await })
            .await;
    }

    async fn download_history(&self) -> Result<(), Failure> {
        #[derive(serde::Deserialize)]
        struct Pulled {
            entries: Vec<Value>,
            next: i64,
            #[serde(default)]
            more: Option<bool>,
        }
        let mut known: HashMap<String, (f64, u32, String)> = HashMap::new();
        for v in self.inner.browser.visits() {
            known.entry(v.key).or_insert((v.last, v.count, v.title));
        }
        let mut merged = 0;
        for _ in 0..40 {
            if !self.active(Domain::History) {
                return Ok(());
            }
            let since = self.inner.client.sync_prefs().pulled_seq;
            let since_text = since.to_string();
            let page: Pulled = self
                .inner
                .client
                .request_json(
                    "GET",
                    "/v1/sync/history",
                    None,
                    &[
                        ("since", &since_text),
                        ("limit", "500"),
                        ("exclude_device", "me"),
                    ],
                )
                .await?;
            let browser = self.inner.browser.clone();
            let mut remote_updates = vec![];
            self.applying(|| {
                for row in &page.entries {
                    let payload = row.get("payload");
                    let Some(raw) = payload.and_then(|p| p.get("url")).and_then(Value::as_str)
                    else {
                        continue;
                    };
                    let Some(url) = is_web(raw) else { continue };
                    let title = payload
                        .and_then(|p| p.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let when = payload
                        .and_then(|p| p.get("visited_at"))
                        .and_then(|v| match v {
                            Value::Number(n) => {
                                n.as_f64().map(|n| if n > 1e11 { n / 1000.0 } else { n })
                            }
                            Value::String(s) => history::parse_stamp(s),
                            _ => None,
                        })
                        .or_else(|| {
                            row.get("visited_at")
                                .and_then(Value::as_str)
                                .and_then(history::parse_stamp)
                        })
                        .unwrap_or_else(now_unix);
                    let key = copper_core::address::history_key(&url);
                    let have = known.get(&key).cloned();
                    if let Some((last, _, _)) = &have
                        && merge::same_visit(*last, when)
                    {
                        continue;
                    }
                    let count = have.as_ref().map(|h| h.1).unwrap_or(0) + 1;
                    browser.take_visit(url.as_str(), title, count, when);
                    let last = have.as_ref().map(|h| h.0.max(when)).unwrap_or(when);
                    let kept_title = if title.is_empty() {
                        have.as_ref().map(|h| h.2.clone()).unwrap_or_default()
                    } else {
                        title.to_string()
                    };
                    if when >= have.as_ref().map(|h| h.0).unwrap_or(f64::MIN) {
                        remote_updates.push((key.clone(), when));
                    }
                    known.insert(key, (last, count, kept_title));
                    merged += 1;
                }
                if !page.entries.is_empty() {
                    browser.settle_history();
                }
            });
            self.with(|s| {
                for (k, v) in remote_updates {
                    s.saved.remote.insert(k, v);
                }
            });
            self.inner
                .client
                .update_sync_prefs(|p| p.pulled_seq = since.max(page.next));
            self.save_state();
            if page.more != Some(true) || page.entries.is_empty() {
                break;
            }
        }
        if merged > 0 {
            self.note(format!(
                "Pulled {merged} visit{} from other devices",
                if merged == 1 { "" } else { "s" }
            ));
        }
        Ok(())
    }

    // MARK: deleting history on the cloud

    /// Deletes visits on the cloud — every device's rows, as `selector` says —
    /// and leaves history here as it is. For settings, the CLI and agents.
    pub async fn delete_history(&self, selector: &Selector) -> Result<Outcome, Failure> {
        self.delete_history_until(selector, &delete::Stop::new())
            .await
    }

    /// [`SyncEngine::delete_history`] that `stop` can stop (Settings › Cloud's
    /// Stop): while looking, nothing goes ([`delete::Stop::failure`]); while
    /// deleting, what already went is the outcome ([`Outcome::stopped`]).
    pub async fn delete_history_until(
        &self,
        selector: &Selector,
        stop: &delete::Stop,
    ) -> Result<Outcome, Failure> {
        self.erase(selector, &delete::words(selector), Here::Kept, stop)
            .await
    }

    /// History cleared here: everything on the cloud too while history syncs.
    /// Fire and forget; the log says how it went.
    pub fn history_cleared(&self) {
        if !self.active(Domain::History) {
            return;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let _ = me
                .erase(
                    &Selector::cleared(),
                    "cleared here",
                    Here::Cleared,
                    &delete::Stop::new(),
                )
                .await;
        });
    }

    /// A page forgotten here: its rows on the cloud too, whichever device sent
    /// them.
    pub fn history_forgot(&self, key: &str) {
        if !self.active(Domain::History) || key.is_empty() {
            return;
        }
        self.with(|s| s.forgotten.insert(key.to_string(), now_unix()));
        let site = key.split('/').next().unwrap_or("").to_string();
        let me = self.clone();
        let key = key.to_string();
        tokio::spawn(async move {
            let _ = me
                .erase(
                    &Selector::forgot(&key),
                    &format!("a page on {site}, forgotten here"),
                    Here::Forgot,
                    &delete::Stop::new(),
                )
                .await;
        });
    }

    async fn erase(
        &self,
        selector: &Selector,
        why: &str,
        here: Here,
        stop: &delete::Stop,
    ) -> Result<Outcome, Failure> {
        let failed = |e: Failure| {
            // Stopped by the person: nothing to tell the log (the Mac's
            // CancellationError).
            if e.code != "stopped" {
                self.note(format!(
                    "History: nothing deleted from the cloud ({why}) — {}",
                    e.message
                ));
            }
            e
        };
        if !self.inner.client.is_signed_in() {
            return Err(failed(Failure::not_signed_in()));
        }
        if self.inner.client.deletes_history().is_none() {
            self.inner.client.load_info(false).await;
        }
        let Some(can) = self.inner.client.deletes_history() else {
            return Err(failed(Failure::new(
                0,
                "network",
                "Couldn't ask the cloud whether it can delete history — try again when it's reachable",
            )));
        };
        if !can {
            return Err(failed(Failure::new(0, "unsupported", delete::UNSUPPORTED)));
        }
        let _push = self.lane("history-push").await;
        let _pull = self.lane("history-pull").await;
        if here == Here::Cleared {
            self.settle_push(selector, here);
        }
        let client = self.inner.client.clone();
        // A task of its own, so dropping this future never cuts a delete off
        // halfway: only `stop` ends it early, at a point where what went is
        // known.
        let selector_owned = selector.clone();
        let stop = stop.clone();
        let result = tokio::spawn(async move {
            delete::run_until(&selector_owned, delete::PAGE_LIMIT, &stop, |ask| {
                let client = client.clone();
                async move {
                    let answer = match ask.body {
                        Some(body) => {
                            client
                                .request_raw(ask.method, "/v1/sync/history", body, &[])
                                .await?
                        }
                        None => {
                            client
                                .request(ask.method, "/v1/sync/history", None, &ask.query_pairs())
                                .await?
                        }
                    };
                    Ok(answer.body)
                }
            })
            .await
        })
        .await
        .unwrap_or_else(|e| Err(Failure::new(0, "cancelled", e.to_string())));
        match result {
            Ok(outcome) => {
                if here == Here::Kept {
                    self.settle_push(selector, here);
                }
                let visits = format!(
                    "{} visit{}",
                    outcome.deleted,
                    if outcome.deleted == 1 { "" } else { "s" }
                );
                let cut = if outcome.stopped {
                    " (stopped)".to_string()
                } else if outcome.cut {
                    format!(
                        " (looked through the first {} only)",
                        delete::PAGE_LIMIT * delete::PAGE_SIZE
                    )
                } else {
                    String::new()
                };
                self.note(format!(
                    "History: deleted {visits} from the cloud — {why}{cut}"
                ));
                Ok(outcome)
            }
            Err(e) => {
                if here == Here::Kept && e.code == "partial" {
                    self.settle_push(selector, here);
                }
                Err(failed(e))
            }
        }
    }

    fn settle_push(&self, selector: &Selector, here: Here) {
        let now = now_unix();
        let visits = if here == Here::Kept {
            self.local_visits()
        } else {
            vec![]
        };
        let prefs = self.inner.client.sync_prefs();
        let through = prefs.pushed_through.map(|t| t.0);
        let device = self.inner.client.account().map(|a| wire_id(&a.device_id));
        let remote = self.with(|s| s.saved.remote.clone());
        let (t, r) = delete::push_after(
            selector,
            here,
            &visits,
            through,
            &remote,
            device.as_deref(),
            now,
        );
        if t == through && r == remote {
            return;
        }
        self.inner
            .client
            .update_sync_prefs(|p| p.pushed_through = t.map(UnixTime));
        self.with(|s| s.saved.remote = r);
        self.save_state();
    }
}

fn encode_failure(e: serde_json::Error) -> Failure {
    Failure::new(0, "encode", e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settle_rules() {
        assert_eq!(
            SyncEngine::settled(false, Some("x"), true, &SyncState::Idle),
            (SyncState::Idle, false)
        );
        assert_eq!(
            SyncEngine::settled(true, Some("offline"), true, &SyncState::Idle),
            (SyncState::Offline, false)
        );
        assert_eq!(
            SyncEngine::settled(true, Some("boom"), true, &SyncState::Idle),
            (SyncState::Error("boom".into()), false)
        );
        assert_eq!(
            SyncEngine::settled(true, None, true, &SyncState::Offline),
            (SyncState::Idle, true)
        );
        assert_eq!(
            SyncEngine::settled(true, None, false, &SyncState::Offline),
            (SyncState::Offline, false)
        );
        assert_eq!(
            SyncEngine::settled(true, None, false, &SyncState::Syncing),
            (SyncState::Idle, false)
        );
    }

    #[test]
    fn stems() {
        assert_eq!(
            stem("ABC", true, "https://Mail.Test/x"),
            "abc|pin|mail.test"
        );
        assert_eq!(stem("ABC", false, "nope"), "abc|saved|");
    }

    #[test]
    fn domains() {
        for d in Domain::ALL {
            assert_eq!(Domain::parse(d.as_str()), Some(d));
        }
        assert_eq!(Domain::parse("tabs:abc"), None);
    }

    // MARK: kept-tab keys across launches

    use crate::sync::adapter::{MemoryBrowser, MemoryState};
    use copper_core::session::Space;
    use copper_core::{Paths, Store};

    const HOME: &str = "A0000000-0000-4000-8000-00000000000A";
    const WORK: &str = "A0000000-0000-4000-8000-0000000000AB";
    const KEPT_AT: &str = "http://127.0.0.1:8080/r";
    const LANDED: &str = "http://localhost:8080/landed?from=r";

    fn store(dir: &std::path::Path) -> Store {
        Store::new(Paths {
            data: dir.join("data"),
            config: dir.join("config"),
        })
    }

    fn space(id: &str, name: &str) -> Space {
        Space {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn tab(id: &str, space: &str, url: &str, pin: Option<&str>, key: Option<&str>) -> LiveKept {
        LiveKept {
            group: None,
            tab_id: id.into(),
            key: key.map(str::to_string),
            space: space.into(),
            pinned: pin.is_some(),
            url: url.into(),
            title: format!("Title of {url}"),
            pin: pin.map(str::to_string),
        }
    }

    fn browser(kept: Vec<LiveKept>) -> Arc<MemoryBrowser> {
        Arc::new(MemoryBrowser::new(MemoryState {
            spaces: vec![space(HOME, "Home"), space(WORK, "Work")],
            kept,
            ..Default::default()
        }))
    }

    /// A launch: a fresh engine over the same files (the bases in
    /// `cloud-sync.json`), nothing remembered in memory.
    fn launch(dir: &std::path::Path, b: &Arc<MemoryBrowser>) -> SyncEngine {
        let client = CloudClient::open(store(dir)).unwrap();
        SyncEngine::new(client, b.clone())
    }

    /// What a PUT that was answered with `version` leaves behind.
    fn synced(e: &SyncEngine, version: i64, doc: &SpacesDoc) {
        e.set_base("spaces", Some(SyncBase::from_doc(version, doc).unwrap()));
    }

    fn base(e: &SyncEngine) -> SpacesDoc {
        e.base::<SpacesDoc>("spaces").expect("a spaces base")
    }

    fn build(e: &SyncEngine) -> SpacesDoc {
        let b = e.base::<SpacesDoc>("spaces");
        e.build_spaces(b.as_ref()).normalized()
    }

    fn keys(b: &MemoryBrowser) -> Vec<(String, Option<String>)> {
        b.snapshot()
            .kept
            .iter()
            .map(|t| (t.tab_id.clone(), t.key.clone()))
            .collect()
    }

    /// The tabs as the next launch restores them: new ids, the address each
    /// had at the quit, and whatever key each carried.
    fn relaunched(b: &MemoryBrowser, run: u32) -> Arc<MemoryBrowser> {
        let mut kept = b.snapshot().kept;
        for t in &mut kept {
            t.tab_id = format!("{}@run{run}", t.tab_id);
        }
        browser(kept)
    }

    #[test]
    fn a_kept_tab_redirected_after_a_relaunch_stays_the_same_entry() {
        let dir = tempfile::tempdir().unwrap();
        // Run 1: a Saved tab is kept at /r and synced.
        let b1 = browser(vec![tab("t1", HOME, KEPT_AT, None, None)]);
        let e1 = launch(dir.path(), &b1);
        let doc = e1.build_spaces(None).normalized();
        assert_eq!(doc.tabs.len(), 1);
        let key = doc.tabs[0].key.clone();
        assert_eq!(doc.tabs[0].url, KEPT_AT);
        synced(&e1, 1, &doc);
        // The key is kept on the tab (its session entry).
        assert_eq!(keys(&b1), vec![("t1".into(), Some(key.clone()))]);
        // It wakes and redirects to another host: this run already knew.
        b1.edit(|s| s.kept[0].url = LANDED.into());
        assert_eq!(build(&e1), doc);
        drop(e1);

        // Run 2: restored at the address it was quit at (the redirect's), and
        // it redirects again. The document doesn't change: nothing to push.
        let b2 = relaunched(&b1, 2);
        assert_eq!(b2.snapshot().kept[0].url, LANDED);
        let e2 = launch(dir.path(), &b2);
        assert_eq!(base(&e2), doc);
        assert_eq!(build(&e2), doc, "the redirect replaced the kept tab");
        // Run 3: the same again.
        let b3 = relaunched(&b2, 3);
        let e3 = launch(dir.path(), &b3);
        assert_eq!(build(&e3), doc);

        // Without the key the tab carries (a build from before this fix), the
        // same relaunch made it a new entry and dropped the one at /r — what
        // every other device then closed.
        let bare = relaunched(&b1, 4);
        bare.edit(|s| s.kept[0].key = None);
        let e4 = launch(dir.path(), &bare);
        let lost = build(&e4);
        assert!(!lost.tabs.iter().any(|t| t.key == key), "{lost:?}");
        assert!(lost.tabs.iter().any(|t| t.url == LANDED));
    }

    #[test]
    fn remember_kept_tabs_keys_restored_tabs_before_they_load() {
        let dir = tempfile::tempdir().unwrap();
        // The last document synced has a Saved tab another device made (any
        // key shape) and a pin; this device restores both, never keyed (a
        // session written before keys were kept, or with spaces sync off).
        let theirs = "a0000000-0000-4000-8000-00000000000a|saved|127.0.0.1|from-the-mac";
        let doc = SpacesDoc {
            groups: None,
            v: 1,
            spaces: vec![space(HOME, "Home"), space(WORK, "Work")],
            tabs: vec![
                KeptTab {
                    group: None,
                    key: theirs.into(),
                    space: HOME.into(),
                    url: KEPT_AT.into(),
                    title: "Hyper-V".into(),
                    pinned: false,
                    pin: None,
                    extra: Default::default(),
                },
                KeptTab {
                    group: None,
                    key: "a0000000-0000-4000-8000-00000000000a|pin|mail.test|https://mail.test/"
                        .into(),
                    space: HOME.into(),
                    url: "https://mail.test/".into(),
                    title: "Mail".into(),
                    pinned: true,
                    pin: Some("M".into()),
                    extra: Default::default(),
                },
            ],
            extra: Default::default(),
        };
        let mut doc = doc;
        doc.tabs.sort_by(|a, b| a.key.cmp(&b.key));
        {
            let e = launch(dir.path(), &browser(vec![]));
            synced(&e, 17, &doc);
        }
        let b1 = browser(vec![
            tab("pin", HOME, "https://mail.test/", Some("M"), None),
            tab("hv", HOME, KEPT_AT, None, None),
        ]);
        let e1 = launch(dir.path(), &b1);
        // Spaces sync is off and nobody is signed in: still keyed, locally.
        assert!(!e1.syncs(Domain::Spaces));
        e1.remember_kept_tabs();
        let carried: BTreeMap<String, Option<String>> = keys(&b1).into_iter().collect();
        assert_eq!(carried["hv"].as_deref(), Some(theirs));
        assert_eq!(
            carried["pin"].as_deref(),
            Some("a0000000-0000-4000-8000-00000000000a|pin|mail.test|https://mail.test/")
        );
        // Then it wakes and redirects, and the next launch restores it there.
        b1.edit(|s| s.kept[1].url = LANDED.into());
        assert_eq!(build(&e1), doc);
        let b2 = relaunched(&b1, 2);
        let e2 = launch(dir.path(), &b2);
        e2.remember_kept_tabs();
        assert_eq!(build(&e2), doc);

        // No base: nothing to match, nothing written.
        let empty = tempfile::tempdir().unwrap();
        let b3 = browser(vec![tab("x", HOME, KEPT_AT, None, None)]);
        launch(empty.path(), &b3).remember_kept_tabs();
        assert_eq!(keys(&b3), vec![("x".into(), None)]);
    }

    #[test]
    fn a_user_navigation_keeps_the_entry_and_later_edits_still_sync() {
        let dir = tempfile::tempdir().unwrap();
        let b1 = browser(vec![
            tab(
                "mail",
                HOME,
                "https://mail.example.com/inbox",
                Some("M"),
                None,
            ),
            tab("docs", WORK, "https://docs.example.com/", None, None),
        ]);
        let e1 = launch(dir.path(), &b1);
        let doc = e1.build_spaces(None).normalized();
        synced(&e1, 1, &doc);
        let mail = doc.tabs.iter().find(|t| t.pinned).unwrap().clone();
        // The person browses inside the pin, off to another site, and quits.
        b1.edit(|s| s.kept[0].url = "https://news.example.org/today".into());
        assert_eq!(build(&e1), doc, "browsing inside a pin resends nothing");
        // Next launch: restored there; still the same pin, at the address it
        // was kept with (the Mac's rule: the one it had when it first synced).
        let b2 = relaunched(&b1, 2);
        let e2 = launch(dir.path(), &b2);
        let now = build(&e2);
        assert_eq!(now, doc);
        assert_eq!(
            now.tabs.iter().find(|t| t.pinned).unwrap().url,
            "https://mail.example.com/inbox"
        );

        // What the person does to that tab still syncs, as itself.
        // Closing it removes its entry (and only it).
        b2.edit(|s| s.kept.retain(|t| !t.tab_id.starts_with("mail")));
        let closed = build(&e2);
        assert!(!closed.tabs.iter().any(|t| t.key == mail.key));
        assert_eq!(closed.tabs.len(), 1);
        let after = merge::spaces(Some(&doc), &closed, &doc);
        assert!(!after.tabs.iter().any(|t| t.key == mail.key));
        synced(&e2, 2, &closed);

        // Moving the Saved tab to another space moves its entry there.
        let b3 = relaunched(&b2, 3);
        b3.edit(|s| {
            s.kept[0].space = HOME.into();
            s.kept[0].url = "https://elsewhere.example.net/".into();
        });
        let e3 = launch(dir.path(), &b3);
        let moved = build(&e3);
        assert_eq!(moved.tabs.len(), 1);
        assert_eq!(moved.tabs[0].space, HOME);
        assert!(moved.tabs[0].key.starts_with(&HOME.to_lowercase()));
        assert_ne!(moved.tabs[0].key, closed.tabs[0].key);
        // …and the new key is the one the tab carries from now on.
        assert_eq!(b3.snapshot().kept[0].key.as_ref(), Some(&moved.tabs[0].key));

        // A tab kept anew is a new entry, at the address it has now.
        b3.edit(|s| {
            s.kept
                .push(tab("new", WORK, "https://new.example.com/a", None, None))
        });
        let added = build(&e3);
        assert!(
            added
                .tabs
                .iter()
                .any(|t| t.url == "https://new.example.com/a" && t.space == WORK)
        );
    }

    #[test]
    fn a_carried_key_is_only_used_while_the_base_has_it_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let b1 = browser(vec![tab("a", HOME, KEPT_AT, None, None)]);
        let e1 = launch(dir.path(), &b1);
        let doc = e1.build_spaces(None).normalized();
        synced(&e1, 1, &doc);
        let key = doc.tabs[0].key.clone();
        // A key the base no longer has (stale) is not used: the address is.
        let b2 = browser(vec![tab(
            "a2",
            HOME,
            KEPT_AT,
            None,
            Some("a0000000-0000-4000-8000-00000000000a|saved|gone|x"),
        )]);
        let e2 = launch(dir.path(), &b2);
        assert_eq!(build(&e2), doc);
        assert_eq!(b2.snapshot().kept[0].key.as_ref(), Some(&key));
        // Two tabs carrying one key (a copy of the tab): one keeps it, the
        // other is a new entry with a key of its own.
        let b3 = browser(vec![
            tab("x", HOME, LANDED, None, Some(&key)),
            tab("y", HOME, LANDED, None, Some(&key)),
        ]);
        let e3 = launch(dir.path(), &b3);
        let two = build(&e3);
        assert_eq!(two.tabs.len(), 2);
        assert!(two.tabs.iter().any(|t| t.key == key && t.url == KEPT_AT));
        let carried: BTreeSet<Option<String>> =
            b3.snapshot().kept.into_iter().map(|t| t.key).collect();
        assert_eq!(carried.len(), 2, "{carried:?}");
    }

    #[test]
    fn tabs_opened_for_another_devices_entries_carry_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let b = browser(vec![]);
        let e = launch(dir.path(), &b);
        let before = SpacesDoc {
            groups: None,
            v: 1,
            spaces: vec![space(HOME, "Home"), space(WORK, "Work")],
            tabs: vec![],
            extra: Default::default(),
        };
        synced(&e, 1, &before);
        let key = "a0000000-0000-4000-8000-00000000000a|saved|127.0.0.1|made-elsewhere";
        let mut doc = before.clone();
        doc.tabs.push(KeptTab {
            group: None,
            key: key.into(),
            space: HOME.into(),
            url: KEPT_AT.into(),
            title: "Kept elsewhere".into(),
            pinned: false,
            pin: None,
            extra: Default::default(),
        });
        e.apply_spaces(&doc, Some(&before));
        let opened = b.snapshot().kept;
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].url, KEPT_AT);
        assert_eq!(opened[0].key.as_deref(), Some(key));
        synced(&e, 2, &doc);
        // Woken, redirected, quit before anything else synced: the same entry.
        b.edit(|s| s.kept[0].url = LANDED.into());
        let next = relaunched(&b, 2);
        let e2 = launch(dir.path(), &next);
        assert_eq!(build(&e2), doc);
    }

    #[test]
    fn a_pin_keyed_under_two_spaces_is_pinned_once() {
        // An older Mac keyed its pins by the space on screen: the document
        // holds each pin under both spaces' keys. One pin each, not two.
        let dir = tempfile::tempdir().unwrap();
        let home = |host: &str, url: &str| format!("{}|pin|{host}|{url}", HOME.to_lowercase());
        let work = |host: &str, url: &str| format!("{}|pin|{host}|{url}", WORK.to_lowercase());
        let pin = |key: String, space: &str, url: &str| KeptTab {
            group: None,
            key,
            space: space.into(),
            url: url.into(),
            title: url.into(),
            pinned: true,
            pin: Some("P".into()),
            extra: Default::default(),
        };
        let mail_key = home("mail.test", "https://mail.test/");
        let b = browser(vec![tab(
            "mail",
            HOME,
            "https://mail.test/",
            Some("M"),
            Some(&mail_key),
        )]);
        let e = launch(dir.path(), &b);
        let mut before = SpacesDoc {
            groups: None,
            v: 1,
            spaces: vec![space(HOME, "Home"), space(WORK, "Work")],
            tabs: vec![pin(mail_key.clone(), HOME, "https://mail.test/")],
            extra: Default::default(),
        };
        before.tabs.sort_by(|a, b| a.key.cmp(&b.key));
        synced(&e, 1, &before);
        let mut doc = before.clone();
        doc.tabs.extend([
            // The same pin, browsed elsewhere, under the other space.
            pin(
                work("mail.test", "https://mail.test/u/1"),
                WORK,
                "https://mail.test/u/1",
            ),
            // A new pin, twice.
            pin(
                home("news.test", "https://news.test/"),
                HOME,
                "https://news.test/",
            ),
            pin(
                work("news.test", "https://news.test/"),
                WORK,
                "https://news.test/",
            ),
        ]);
        doc.tabs.sort_by(|a, b| a.key.cmp(&b.key));
        e.apply_spaces(&doc, Some(&before));
        let urls: Vec<String> = b.snapshot().kept.into_iter().map(|t| t.url).collect();
        assert_eq!(urls, vec!["https://mail.test/", "https://news.test/"]);
        // The pin already here keeps its own key.
        assert_eq!(keys(&b)[0], ("mail".into(), Some(mail_key)));
    }

    #[test]
    fn a_second_pin_for_a_site_closes_and_a_rekeyed_pin_stays_open() {
        // What an older build left here: the same site pinned twice.
        let dir = tempfile::tempdir().unwrap();
        let key = |n: &str| {
            format!(
                "{}|pin|mail.test|https://mail.test/{n}",
                HOME.to_lowercase()
            )
        };
        let b = browser(vec![
            tab(
                "first",
                HOME,
                "https://mail.test/",
                Some("M"),
                Some(&key("")),
            ),
            tab(
                "copy",
                HOME,
                "https://mail.test/u/1",
                Some("M"),
                Some(&key("u/1")),
            ),
        ]);
        let e = launch(dir.path(), &b);
        let built = build(&e);
        let pins: Vec<&str> = built
            .tabs
            .iter()
            .filter(|t| t.pinned)
            .map(|t| t.key.as_str())
            .collect();
        assert_eq!(pins, vec![key("").as_str()]);
        assert_eq!(keys(&b), vec![("first".into(), Some(key("")))]);

        // Another device's merge kept the site's pin under another key: the
        // pin here takes that key and stays open, never closed and reopened.
        synced(&e, 1, &built);
        let mut doc = built.clone();
        doc.tabs[0].key =
            "00000000-0000-4000-8000-000000000000|pin|mail.test|https://mail.test/".into();
        e.apply_spaces(&doc, Some(&built));
        assert_eq!(
            keys(&b),
            vec![("first".into(), Some(doc.tabs[0].key.clone()))]
        );
    }

    #[test]
    fn folders_come_with_the_spaces_and_an_old_document_keeps_them() {
        use copper_core::cloud::CloudFolder;
        let dir = tempfile::tempdir().unwrap();
        let b = browser(vec![]);
        let e = launch(dir.path(), &b);
        let before = SpacesDoc {
            groups: None,
            v: 1,
            spaces: vec![space(HOME, "Home"), space(WORK, "Work")],
            tabs: vec![],
            extra: Default::default(),
        };
        synced(&e, 1, &before);
        let folder = |id: &str, name: &str| CloudFolder {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        };
        let apps = "F0000000-0000-4000-8000-0000000000A2";
        let mut doc = before.clone();
        doc.groups = Some(vec![
            folder("F0000000-0000-4000-8000-0000000000A1", "Exowatt"),
            folder(apps, "Exowatt › Apps"),
        ]);
        doc.tabs.push(KeptTab {
            group: Some(apps.into()),
            key: "a0000000-0000-4000-8000-0000000000ab|saved|x.test|https://x.test/".into(),
            space: WORK.into(),
            url: "https://x.test/".into(),
            title: "X".into(),
            pinned: false,
            pin: None,
            extra: Default::default(),
        });
        e.apply_spaces(&doc, Some(&before));
        let got = b.snapshot();
        assert_eq!(got.groups.as_ref().map(Vec::len), Some(2));
        assert_eq!(got.kept[0].group.as_deref(), Some(apps));
        // What it sends back carries the folders and the filing.
        synced(&e, 2, &doc);
        let built = build(&e);
        assert_eq!(built.groups, doc.groups);
        assert_eq!(built.tabs[0].group.as_deref(), Some(apps));
        // A document from an older build, without folders, takes nothing away.
        let merged = merge::spaces(
            Some(&doc),
            &built,
            &SpacesDoc {
                groups: None,
                ..doc.clone()
            },
        );
        assert_eq!(merged.groups, doc.groups);
    }
}
