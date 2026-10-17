//! The canvas page in a tab, and the bridge between it and Copper (Mac:
//! `CanvasHost.swift`).
//!
//! A canvas tab is an ordinary tab whose address is `copper://canvas/<id>`,
//! served by [`super::scheme`]. When WebKit asks for that address the tab's
//! page is given its bridge before the bytes go back:
//!
//! * the `canvas` message handler, in an isolated world of its own, and a
//!   main-frame-only relay: the page world's `window.webkit.messageHandlers
//!   .canvas.postMessage(json)` dispatches a DOM event the isolated world
//!   forwards. A frame inside the board (a live web frame) has neither, so
//!   only the bundled page itself can speak — WebKitGTK's script messages
//!   carry no frame, which is what the Mac checks instead;
//! * the live-frame reporter (`copperFrame`, its own isolated world).
//!
//! The page is spoken to through `window.copperCanvas.*` (the curated calls
//! in `copper_pagejs::canvas`). It never touches the network: when the
//! canvas has a room on the cloud, Copper holds the socket
//! ([`copper_cloud::canvas::room::CanvasRoom`]) and relays frames both ways.
//! Every Yjs update the page reports is kept on disk ([`super::store`])
//! before anything else happens to it.
//!
//! Every call to the page is issued at once, in the order made (the
//! compaction relies on it: an export asked for after a batch of posts sees
//! all of them).

use super::registry::{self, is_personal, is_shared};
use super::store::{CanvasStore, fingerprint};
use crate::shell::Shell;
use copper_cloud::canvas::room::{CanvasRoom, RoomEvent};
use copper_core::TabId;
use copper_core::canvas::CanvasEntry;
use copper_engine::{JsonMap, LoadState, PageEvent, PageView, SnapshotRegion, UserScript, World};

use copper_engine_webkitgtk::WebKitPage;
use gtk4::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// The isolated world the `canvas` handler lives in.
pub const HOST_WORLD: &str = "CopperCanvasHost";
pub const HANDLER: &str = "canvas";
const FRAME_HANDLER: &str = "copperFrame";

/// The page world: `window.webkit.messageHandlers.canvas` posts a DOM event
/// the isolated world forwards. `window.webkit` is WebKit's own and it hands
/// out a new namespace object when handlers come and go, so the shadowing
/// `messageHandlers` accessor is put back whenever it is lost; meanwhile the
/// page's own fallback (`window.__copperHost.onMessage`, used when it finds
/// no handler) carries the message the same way.
const PAGE_RELAY: &str = r#"(() => {
  if (location.protocol !== 'copper:' || location.host !== 'canvas') return;
  const send = (text) => document.dispatchEvent(new CustomEvent('copper-canvas-post', { detail: text }));
  const relay = Object.freeze({ postMessage: (m) => send(typeof m === 'string' ? m : JSON.stringify(m)) });
  const install = () => {
    try {
      const wk = window.webkit;
      if (!wk || typeof wk !== 'object' || Object.prototype.hasOwnProperty.call(wk, 'messageHandlers')) return;
      let d = null;
      for (let o = Object.getPrototypeOf(wk); o && !d; o = Object.getPrototypeOf(o)) d = Object.getOwnPropertyDescriptor(o, 'messageHandlers');
      const native = () => { try { return d && d.get ? d.get.call(wk) : undefined; } catch (_) { return undefined; } };
      const handlers = new Proxy({}, {
        get: (_, k) => {
          if (k === 'canvas') return relay;
          const n = native();
          return n ? n[k] : undefined;
        },
      });
      Object.defineProperty(wk, 'messageHandlers', { get: () => handlers, configurable: false });
    } catch (_) {}
  };
  const host = {
    onMessage: (e, text) => {
      send(typeof text === 'string' ? text : JSON.stringify(e));
      install();
    },
  };
  Object.defineProperty(host, 'messages', { get: () => [], set: () => {}, configurable: false });
  Object.defineProperty(window, '__copperHost', { value: host, configurable: false, writable: false });
  install();
})();"#;

/// The isolated world: forward the page's events to the real handler — only
/// on a canvas page.
const HOST_RELAY: &str = r#"(() => {
  if (location.protocol !== 'copper:' || location.host !== 'canvas') return;
  document.addEventListener('copper-canvas-post', (e) => {
    if (location.protocol !== 'copper:' || location.host !== 'canvas') return;
    const d = e.detail;
    if (typeof d === 'string' && d.length < 64 * 1024 * 1024) window.webkit.messageHandlers.canvas.postMessage(d);
  }, true);
})();"#;

thread_local! {
    static HOSTS: RefCell<HashMap<TabId, Rc<CanvasHost>>> = RefCell::new(HashMap::new());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageSocket {
    None,
    Waiting,
    Open,
}

/// What failed.
#[derive(Debug, Clone)]
pub struct Failure(pub String);

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure(s)
    }
}

impl From<&str> for Failure {
    fn from(s: &str) -> Self {
        Failure(s.into())
    }
}

/// How long a canvas page may take to come up, every stage together: a
/// huge board's whole state goes to the page in one call.
pub const READY_WITHIN: Duration = Duration::from_secs(60);
/// How long one of the stages before the hand-over may go without moving
/// on before the wait gives up on it.
pub const STALL: Duration = Duration::from_secs(30);
/// How often a wait marks its tab as used, so the idle sleeper leaves it.
pub const STAMP_EVERY: Duration = Duration::from_secs(5);

/// Where a canvas tab is on its way to being ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The tab has no page (asleep, or just made).
    Tab,
    /// The page is loading the canvas address.
    Page,
    /// The page has loaded but its board has not said `ready`.
    Script,
    /// The page said `ready`; the canvas is being handed to it.
    State,
    Ready,
}

impl Stage {
    pub fn describe(self) -> &'static str {
        match self {
            Stage::Tab => "building the tab's page",
            Stage::Page => "loading the canvas page",
            Stage::Script => "starting the board (the page hasn't said it is ready)",
            Stage::State => "handing the canvas to the page",
            Stage::Ready => "ready",
        }
    }
}

/// Mark a tab as just used (its `seen`), so the idle sleeper leaves it be
/// while a call waits on it.
pub fn keep_awake(shell: &crate::shell::Shell, tab: &TabId) {
    shell
        .browser_mut()
        .update_tab(tab, |t| t.seen = copper_core::date::UnixTime::now());
}

/// One canvas tab's bridge.
pub struct CanvasHost {
    pub tab: TabId,
    canvas_id: RefCell<String>,
    store_key: RefCell<String>,
    /// The page widget the bridge is attached to (pointer, only compared).
    bound: Cell<usize>,
    generation: Cell<u64>,
    initialized: Cell<u64>,
    ready_on: Cell<usize>,
    selection: RefCell<Vec<String>>,
    room_id: RefCell<Option<String>>,
    room: RefCell<Option<CanvasRoom>>,
    room_gen: Cell<u64>,
    synced: Cell<bool>,
    restarting: Cell<bool>,
    told_status: RefCell<Option<(String, usize)>>,
    status_soon: Cell<bool>,
    page_socket: Cell<PageSocket>,
    page_ever_asked: Cell<bool>,
    room_fresh: Cell<bool>,
    held: RefCell<Vec<Vec<u8>>>,
    recent: RefCell<Vec<u64>>,
    peers: RefCell<HashMap<u64, (String, Instant)>>,
    compacting: Cell<Option<Instant>>,
    compaction_unsupported: Cell<bool>,
    last_error: RefCell<Option<String>>,
    /// Why the last hand-over of the canvas to the page failed (cleared by
    /// the next `ready`): a wait stops at once instead of running out.
    init_error: RefCell<Option<String>>,
    members: RefCell<Option<Vec<Value>>>,
    members_read: Cell<Option<Instant>>,
    chat_open: Cell<bool>,
    frame_events: RefCell<Vec<Value>>,
    /// Who the page last said is in the room (`presence`), for `describe`.
    presence: RefCell<Vec<Value>>,
    posters: super::poster::Posters,
    /// Frames page → room (sent, dropped) and room → page.
    counts: Cell<(u64, u64, u64)>,
}

/// Every host.
pub fn all() -> Vec<Rc<CanvasHost>> {
    HOSTS.with(|h| h.borrow().values().cloned().collect())
}

pub fn for_tab(tab: &TabId) -> Option<Rc<CanvasHost>> {
    HOSTS.with(|h| h.borrow().get(tab).cloned())
}

pub fn hosts_of(id: &str) -> Vec<Rc<CanvasHost>> {
    all()
        .into_iter()
        .filter(|h| *h.canvas_id.borrow() == id)
        .collect()
}

/// The tab went (closed, or showing something else): its bridge goes.
pub fn drop_tab(tab: &TabId) {
    if let Some(h) = HOSTS.with(|m| m.borrow_mut().remove(tab)) {
        h.stop_room();
        super::recount();
    }
}

/// Run a JSON-returning call, issued now (see the module docs).
fn issue(
    page: &WebKitPage,
    body: &str,
    args: &JsonMap,
    world: &World,
) -> Pin<Box<dyn Future<Output = copper_engine::Result<Value>>>> {
    let mut fut = page.call_async(body, args, world);
    // Poll once so WebKit gets the call now, in order; the answer is
    // awaited (or not) later with a real waker.
    let mut cx = Context::from_waker(Waker::noop());
    if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
        return Box::pin(std::future::ready(v));
    }
    fut
}

fn engine_world(w: copper_pagejs::World) -> World {
    match w {
        copper_pagejs::World::Page => World::Page,
        copper_pagejs::World::Isolated(n) => World::Isolated(n.to_string()),
    }
}

fn user_script(s: &copper_pagejs::UserScript) -> UserScript {
    let mut u = UserScript::new(s.source.clone());
    u.injection_time = match s.injection {
        copper_pagejs::Injection::DocumentStart => copper_engine::InjectionTime::DocumentStart,
        copper_pagejs::Injection::DocumentEnd => copper_engine::InjectionTime::DocumentEnd,
    };
    u.frames = match s.frames {
        copper_pagejs::Frames::AllFrames => copper_engine::Frames::All,
        copper_pagejs::Frames::MainFrame => copper_engine::Frames::TopOnly,
    };
    u.world = engine_world(s.world);
    u
}

fn b64(data: &[u8]) -> String {
    glib::base64_encode(data).to_string()
}

fn unb64(text: &str) -> Vec<u8> {
    glib::base64_decode(text)
}

impl CanvasHost {
    fn new(tab: &TabId, id: &str) -> Rc<CanvasHost> {
        let key = registry::entry(id)
            .map(|e| registry::store_key(&e))
            .unwrap_or_else(|| id.to_string());
        Rc::new(CanvasHost {
            tab: tab.clone(),
            canvas_id: RefCell::new(id.to_string()),
            store_key: RefCell::new(key),
            bound: Cell::new(0),
            generation: Cell::new(0),
            initialized: Cell::new(0),
            ready_on: Cell::new(0),
            selection: RefCell::default(),
            room_id: RefCell::default(),
            room: RefCell::default(),
            room_gen: Cell::new(0),
            synced: Cell::new(false),
            restarting: Cell::new(false),
            told_status: RefCell::default(),
            status_soon: Cell::new(false),
            page_socket: Cell::new(PageSocket::None),
            page_ever_asked: Cell::new(false),
            room_fresh: Cell::new(false),
            held: RefCell::default(),
            recent: RefCell::default(),
            peers: RefCell::default(),
            compacting: Cell::new(None),
            compaction_unsupported: Cell::new(false),
            last_error: RefCell::default(),
            init_error: RefCell::default(),
            members: RefCell::default(),
            members_read: Cell::new(None),
            chat_open: Cell::new(false),
            frame_events: RefCell::default(),
            presence: RefCell::default(),
            posters: super::poster::Posters::default(),
            counts: Cell::new((0, 0, 0)),
        })
    }

    pub fn canvas_id(&self) -> String {
        self.canvas_id.borrow().clone()
    }

    pub fn entry(&self) -> Option<CanvasEntry> {
        registry::entry(&self.canvas_id.borrow())
    }

    fn store(&self) -> Rc<CanvasStore> {
        let root = super::shell()
            .map(|s| s.store().paths().data.join("canvas"))
            .unwrap_or_else(|| std::env::temp_dir().join("copper-canvas"));
        CanvasStore::open(&root, &self.store_key.borrow())
    }

    fn page(&self) -> Option<WebKitPage> {
        let shell = super::shell()?;
        let slot = shell.slot(&self.tab)?;
        (slot.page.widget().as_ptr() as usize == self.bound.get()).then(|| slot.page.clone())
    }

    fn page_ptr(&self) -> usize {
        super::shell()
            .and_then(|s| s.slot(&self.tab))
            .map(|s| s.page.widget().as_ptr() as usize)
            .unwrap_or(0)
    }

    /// The page has said `ready` and been given its state, and is still the
    /// tab's page.
    pub fn is_ready(&self) -> bool {
        self.initialized.get() > 0
            && self.initialized.get() == self.generation.get()
            && self.ready_on.get() != 0
            && self.ready_on.get() == self.page_ptr()
    }

    pub fn room_open(&self) -> bool {
        self.room.borrow().as_ref().is_some_and(|r| r.is_open())
    }

    pub fn room_synced(&self) -> bool {
        self.room_open() && self.synced.get()
    }

    /// Other people in the room right now (by person; 45 s without a word
    /// and they have gone).
    pub fn collaborators(&self) -> usize {
        let me = registry::identity().0;
        let now = Instant::now();
        let mut people: Vec<String> = self
            .peers
            .borrow()
            .values()
            .filter(|(u, seen)| *u != me && now.duration_since(*seen) < Duration::from_secs(45))
            .map(|(u, _)| u.clone())
            .collect();
        people.sort();
        people.dedup();
        people.len()
    }

    // MARK: attaching

    /// WebKit is loading `copper://canvas/<id>` into the tab's page: make
    /// sure the bridge is there before the page runs. Main thread, inside
    /// the scheme request.
    pub fn attach(shell: &Shell, tab: &TabId, id: &str) -> Rc<CanvasHost> {
        let host = match for_tab(tab) {
            Some(h) => {
                if *h.canvas_id.borrow() != id {
                    h.switch_to(id);
                }
                h
            }
            None => {
                let h = CanvasHost::new(tab, id);
                HOSTS.with(|m| m.borrow_mut().insert(tab.clone(), h.clone()));
                h
            }
        };
        if let Some(slot) = shell.slot(tab) {
            let ptr = slot.page.widget().as_ptr() as usize;
            if host.bound.get() != ptr {
                host.bound.set(ptr);
                host.bind(&slot.page);
            }
        }
        super::recount();
        host
    }

    fn bind(self: &Rc<Self>, page: &WebKitPage) {
        let world = World::isolated(HOST_WORLD);
        page.add_message_handler(HANDLER, &world);
        let mut relay = UserScript::new(PAGE_RELAY);
        relay.injection_time = copper_engine::InjectionTime::DocumentStart;
        relay.frames = copper_engine::Frames::TopOnly;
        page.add_user_script(&relay);
        let mut host = UserScript::new(HOST_RELAY);
        host.injection_time = copper_engine::InjectionTime::DocumentStart;
        host.frames = copper_engine::Frames::TopOnly;
        host.world = world;
        page.add_user_script(&host);
        let frames = copper_pagejs::canvas::frame_reporter();
        let frames_world = engine_world(frames.world);
        page.add_message_handler(FRAME_HANDLER, &frames_world);
        page.add_user_script(&user_script(&frames));
        page.add_user_script(&user_script(&copper_pagejs::canvas::typing()));

        let mut events = page.events();
        let weak = Rc::downgrade(self);
        let mine = page.widget().as_ptr() as usize;
        glib::spawn_future_local(async move {
            while let Some(ev) = events.next().await {
                let Some(host) = weak.upgrade() else { break };
                if host.bound.get() != mine {
                    break;
                }
                match ev {
                    PageEvent::ScriptMessage { name, body } if name == HANDLER => {
                        let body = match body {
                            Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
                            other => other,
                        };
                        if let Value::Object(map) = body {
                            host.received(map);
                        }
                    }
                    PageEvent::ScriptMessage { name, body } if name == FRAME_HANDLER => {
                        host.frame_report(body);
                    }
                    PageEvent::Url(Some(url)) => {
                        if super::id_from_url(&url).is_none() {
                            // Left the canvas for something else.
                            drop_tab(&host.tab);
                            break;
                        }
                    }
                    PageEvent::Load(LoadState::Finished { .. }) => {
                        let weak = Rc::downgrade(&host);
                        glib::timeout_add_local_once(Duration::from_secs(2), move || {
                            if let Some(h) = weak.upgrade() {
                                h.adopt_if_live();
                            }
                        });
                    }
                    PageEvent::Crashed(_) => {
                        host.stop_room();
                        host.ready_on.set(0);
                    }
                    _ => {}
                }
            }
        });
    }

    fn switch_to(&self, id: &str) {
        self.stop_room();
        *self.room_id.borrow_mut() = None;
        self.page_socket.set(PageSocket::None);
        self.page_ever_asked.set(false);
        *self.canvas_id.borrow_mut() = id.to_string();
        *self.store_key.borrow_mut() = registry::entry(id)
            .map(|e| registry::store_key(&e))
            .unwrap_or_else(|| id.to_string());
        self.generation.set(0);
        self.initialized.set(0);
        self.selection.borrow_mut().clear();
        self.peers.borrow_mut().clear();
        *self.members.borrow_mut() = None;
    }

    // MARK: page → host

    fn received(self: &Rc<Self>, body: serde_json::Map<String, Value>) {
        let kind = body.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "ready" => {
                self.ready_on.set(self.page_ptr());
                self.stop_room();
                *self.told_status.borrow_mut() = None;
                self.page_socket.set(PageSocket::None);
                self.page_ever_asked.set(false);
                self.room_fresh.set(false);
                self.held.borrow_mut().clear();
                let me = self.clone();
                glib::spawn_future_local(async move { me.ready().await });
            }
            "update" => {
                if let Some(b) = body.get("b64").and_then(Value::as_str) {
                    let update = unb64(b);
                    if !update.is_empty() {
                        self.keep(&update);
                    }
                }
            }
            "selection" => {
                *self.selection.borrow_mut() = body
                    .get("ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "share" => {
                if let Some(s) = super::shell() {
                    s.toast("Share this canvas with canvas_invite / canvas_share_link, or from another Copper");
                }
            }
            "presence" => {
                *self.presence.borrow_mut() = body
                    .get("people")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
            }
            "framePoster" => self.frame_poster(&body),
            "ws" => {
                if let Some(b) = body.get("b64").and_then(Value::as_str) {
                    let frame = unb64(b);
                    let sent = self.room.borrow().as_ref().is_some_and(|r| r.send(frame));
                    let (ok, dropped, inn) = self.counts.get();
                    if sent {
                        self.counts.set((ok + 1, dropped, inn));
                    } else {
                        self.counts.set((ok, dropped + 1, inn));
                        tracing::debug!(target: "copper_app::canvas", "page frame dropped: no open room");
                    }
                }
            }
            "wsOpen" => {
                self.page_socket.set(PageSocket::Waiting);
                self.page_ever_asked.set(true);
                if self.room_id.borrow().is_none() {
                    return;
                }
                if self.room_open() {
                    if self.room_fresh.get() {
                        self.tell_open();
                    } else {
                        // A new BridgeSocket needs the server's SyncStep1.
                        self.stop_room();
                        self.start_room();
                    }
                } else {
                    self.start_room();
                }
            }
            "wsClose" => {
                self.page_socket.set(PageSocket::None);
                self.stop_room();
                super::recount();
            }
            "openUrl" => {
                let Some(text) = body.get("url").and_then(Value::as_str) else {
                    return;
                };
                let Some(shell) = super::shell() else { return };
                if let Some(id) = super::id_from_url(text) {
                    super::show(&shell, &id, true);
                } else if let Ok(u) = url::Url::parse(text)
                    && matches!(u.scheme(), "http" | "https")
                {
                    shell.open(
                        u.as_str(),
                        crate::shell::OpenTab {
                            foreground: true,
                            after: Some(self.tab.clone()),
                            ..Default::default()
                        },
                    );
                }
            }
            "log" => {
                let level = body.get("level").and_then(Value::as_str).unwrap_or("debug");
                let msg = body.get("msg").map(|m| match m {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
                let msg = msg.unwrap_or_default();
                tracing::debug!(target: "copper_app::canvas", %level, "page: {msg}");
                if level == "error" {
                    *self.last_error.borrow_mut() = Some(msg);
                }
            }
            "chat" => {
                if let Some(open) = body.get("open").and_then(Value::as_bool) {
                    self.chat_open.set(open);
                    let canvas = self.canvas_id();
                    super::chatprefs::update(&registry::identity().0, &canvas, |p| {
                        p.open = Some(open);
                    });
                    // Every other tab on this canvas follows.
                    for other in hosts_of(&canvas) {
                        if other.tab != self.tab && other.is_ready() {
                            other.chat_open.set(open);
                            other.post(copper_pagejs::canvas::show_chat(open));
                        }
                    }
                }
            }
            "chatRead" => {
                // The read mark, kept for the next load. The mentions it
                // read (`mentionIds`) would be marked read on the cloud's
                // mentions inbox, which Linux does not keep.
                let id = body.get("id").and_then(Value::as_str).map(String::from);
                let at = body.get("at").and_then(Value::as_f64);
                super::chatprefs::update(&registry::identity().0, &self.canvas_id(), |p| {
                    if id.is_some() {
                        p.read_id = id;
                    }
                    if at.is_some() {
                        p.read_at = at;
                    }
                });
            }
            "chatMembers" => {
                *self.members.borrow_mut() = None;
                self.tell_chat();
            }
            "mention" => self.mention(&body),
            _ => {}
        }
    }

    /// One update from this page (or another tab on the same canvas): on
    /// disk first, then to every other tab showing it.
    fn keep(self: &Rc<Self>, update: &[u8]) {
        let hash = fingerprint(update);
        {
            let mut recent = self.recent.borrow_mut();
            if let Some(at) = recent.iter().position(|h| *h == hash) {
                recent.remove(at);
                return;
            }
        }
        let store = self.store();
        store.append(update);
        registry::touched(&self.canvas_id.borrow());
        self.hand_to_siblings(update, hash);
        let online = self.room_id.borrow().is_some();
        if (online || self.entry().is_some_and(|e| is_shared(&e))) && !self.room_synced() {
            store.set_pending(store.pending() + 1);
            self.status_later();
        }
        self.compact_if_needed(false);
    }

    fn hand_to_siblings(&self, update: &[u8], hash: u64) {
        let id = self.canvas_id.borrow().clone();
        let key = self.store_key.borrow().clone();
        for other in hosts_of(&id) {
            if other.tab == self.tab || !other.is_ready() || *other.store_key.borrow() != key {
                continue;
            }
            {
                let mut r = other.recent.borrow_mut();
                r.push(hash);
                let n = r.len();
                if n > 64 {
                    r.drain(..n - 64);
                }
            }
            other.post(copper_pagejs::canvas::apply_update(&b64(update)));
        }
    }

    // MARK: ready → init

    async fn ready(self: Rc<Self>) {
        self.generation.set(self.generation.get() + 1);
        let ticket = self.generation.get();
        *self.init_error.borrow_mut() = None;
        // Signed in but the list not read yet this run: give it a moment —
        // so the room is the right one from the start — and, when the
        // canvas is not in the list kept on disk (new on the server), long
        // enough for the list to come.
        if registry::cloud_ready() && !registry::refreshed() {
            super::refresh_soon();
            let known = self.entry().is_some();
            let until = Instant::now() + Duration::from_millis(if known { 1500 } else { 15_000 });
            while !registry::refreshed() && Instant::now() < until {
                glib::timeout_future(Duration::from_millis(50)).await;
            }
        }
        if ticket != self.generation.get() {
            return;
        }
        let Some(entry) = self.entry() else {
            let gone = "That canvas isn't here any more.".to_string();
            *self.last_error.borrow_mut() = Some(gone.clone());
            *self.init_error.borrow_mut() = Some(gone);
            return;
        };
        *self.store_key.borrow_mut() = registry::store_key(&entry);
        let (snapshot, updates) = self.store().load();
        let remote = registry::room_for(&entry);
        *self.room_id.borrow_mut() = remote.clone();
        self.synced.set(false);
        let (me_id, me_name, me_color) = registry::identity();
        let options = json!({
            "docId": entry.remote_id.clone().unwrap_or_else(|| entry.id.clone()),
            "name": entry.name,
            "kind": entry.kind,
            "me": {"id": me_id, "name": me_name, "color": me_color},
            "state": snapshot.as_deref().map(b64),
            "online": remote.is_some(),
            "readOnly": false,
        });
        let ups: Vec<String> = updates.iter().map(|u| b64(u)).collect();
        let call = copper_pagejs::canvas::ready(
            &options,
            &ups,
            &super::theme(),
            &entry.name,
            &ticket.to_string(),
        );
        let t0 = Instant::now();
        if let Err(e) = self.run(&call).await {
            tracing::error!(target: "copper_app::canvas", error = %e, "canvas init failed");
            if ticket == self.generation.get() {
                *self.init_error.borrow_mut() = Some(e.0.clone());
            }
            *self.last_error.borrow_mut() = Some(e.0);
            return;
        }
        tracing::debug!(
            target: "copper_app::canvas",
            ms = t0.elapsed().as_millis() as u64,
            updates = updates.len(),
            "canvas handed to the page"
        );
        if ticket != self.generation.get() {
            return;
        }
        self.initialized.set(ticket);
        if registry::room_for(&entry) != *self.room_id.borrow() {
            self.restart_page();
            return;
        }
        if remote.is_some() {
            if is_personal(&entry) {
                registry::bind_personal();
            }
            if self.page_ever_asked.get() || self.page_socket.get() == PageSocket::Waiting {
                if self.room.borrow().is_none() {
                    self.start_room();
                }
                self.tell_open();
            } else {
                self.start_room();
            }
        } else {
            self.stop_room();
        }
        super::recount();
        self.status_now();
        self.share_now();
        // The frame reporter is on this page (`bind`): a live frame that
        // never reports didn't load (Mac `tellFrameHost`).
        self.post(copper_pagejs::canvas::frame_host(&true));
        self.compact_if_needed(!updates.is_empty());
    }

    /// A page restored from cache, or loaded before the host knew it: given
    /// its state as if it had said `ready`.
    fn adopt_if_live(self: &Rc<Self>) {
        if self.is_ready() {
            return;
        }
        let Some(page) = self.page() else { return };
        if page.url().as_deref().and_then(super::id_from_url).is_none() {
            return;
        }
        let me = self.clone();
        glib::spawn_future_local(async move {
            let live = me
                .run(&copper_pagejs::canvas::is_live())
                .await
                .unwrap_or(false);
            if live && !me.is_ready() {
                let mut m = serde_json::Map::new();
                m.insert("type".into(), json!("ready"));
                me.received(m);
            }
        });
    }

    /// Where this tab is on its way to ready.
    pub fn stage(&self) -> Stage {
        let Some(slot) = super::shell().and_then(|s| s.slot(&self.tab)) else {
            return Stage::Tab;
        };
        let ptr = slot.page.widget().as_ptr() as usize;
        if self.bound.get() != ptr {
            // A page WebKit hasn't asked for the canvas address yet.
            return Stage::Page;
        }
        if self.ready_on.get() != ptr {
            return if slot.page.is_loading() {
                Stage::Page
            } else {
                Stage::Script
            };
        }
        if self.is_ready() {
            Stage::Ready
        } else {
            Stage::State
        }
    }

    /// Until the page has said `ready` and been given its state; a sleeping
    /// tab is woken for it, and kept awake meanwhile. The wait follows the
    /// page through its stages (page built, loaded, `ready`, canvas handed
    /// over): it gives up when one of the first three makes no progress for
    /// [`STALL`], when the hand-over fails, or after `within` in all, and
    /// says which stage it was at.
    pub async fn wait_ready(self: &Rc<Self>, within: Duration) -> Result<(), Failure> {
        let started = Instant::now();
        let mut stage = self.stage();
        let mut since = started;
        let mut adopted: Option<Instant> = None;
        let mut stamped: Option<Instant> = None;
        let mut reloaded = false;
        loop {
            let now = self.stage();
            if now == Stage::Ready {
                if started.elapsed() > Duration::from_secs(2) {
                    tracing::debug!(target: "copper_app::canvas", ms = started.elapsed().as_millis() as u64, "canvas ready after a wait");
                }
                return Ok(());
            }
            if now != stage {
                tracing::debug!(target: "copper_app::canvas", from = ?stage, to = ?now, ms = since.elapsed().as_millis() as u64, "canvas stage");
                stage = now;
                since = Instant::now();
            }
            let shell = super::shell().ok_or("the window has gone")?;
            if stamped.is_none_or(|t| t.elapsed() >= STAMP_EVERY) {
                stamped = Some(Instant::now());
                keep_awake(&shell, &self.tab);
            }
            match shell.slot(&self.tab) {
                None => {
                    if shell.browser().tab(&self.tab).is_none() {
                        return Err(Failure("the canvas tab was closed".into()));
                    }
                    shell.ensure_page(&self.tab);
                }
                Some(slot) if slot.crashed.get() => {
                    if reloaded {
                        return Err(Failure(format!(
                            "the canvas page stopped while {} (its web process quit twice)",
                            stage.describe()
                        )));
                    }
                    reloaded = true;
                    tracing::warn!(target: "copper_app::canvas", "canvas page crashed; reloading it for a waiting call");
                    shell.reload(&self.tab);
                    since = Instant::now();
                }
                Some(_) => {}
            }
            if stage == Stage::Script
                && since.elapsed() > Duration::from_secs(2)
                && adopted.is_none_or(|t| t.elapsed() > Duration::from_secs(3))
            {
                adopted = Some(Instant::now());
                self.adopt_if_live();
            }
            if stage == Stage::State
                && let Some(e) = self.init_error.borrow().clone()
            {
                return Err(Failure(format!(
                    "couldn't hand the canvas to its page: {e}"
                )));
            }
            let stalled = stage != Stage::State && since.elapsed() >= STALL;
            if stalled || started.elapsed() >= within {
                let why = self.last_error.borrow().clone();
                return Err(Failure(format!(
                    "the canvas page didn't come up: still {} after {} s ({} s in all){}",
                    stage.describe(),
                    since.elapsed().as_secs(),
                    started.elapsed().as_secs(),
                    why.map(|w| format!("; the page said: {w}"))
                        .unwrap_or_default()
                )));
            }
            glib::timeout_future(Duration::from_millis(60)).await;
        }
    }

    // MARK: host → page

    /// Run a curated call and decode the answer.
    pub async fn run<T: DeserializeOwned>(
        &self,
        call: &copper_pagejs::Call<T>,
    ) -> Result<T, Failure> {
        let page = self.page().ok_or("the canvas page isn't loaded")?;
        let world = engine_world(call.world());
        let fut = issue(&page, &call.function_body(), &JsonMap::new(), &world);
        let v = fut
            .await
            .map_err(|e| Failure(format!("canvas page: {e}")))?;
        call.decode_value(v).map_err(|e| Failure(e.to_string()))
    }

    /// Run `body` (an async function body) in the page world — the bench only.
    pub async fn eval_js(&self, body: &str) -> Result<Value, Failure> {
        let page = self.page().ok_or("the canvas page isn't loaded")?;
        issue(&page, body, &JsonMap::new(), &World::Page)
            .await
            .map_err(|e| Failure(e.to_string()))
    }

    /// Issued now, answered never.
    fn post<T: 'static>(&self, call: copper_pagejs::Call<T>) {
        let Some(page) = self.page() else { return };
        let world = engine_world(call.world());
        let fut = issue(&page, &call.function_body(), &JsonMap::new(), &world);
        glib::spawn_future_local(async move {
            if let Err(e) = fut.await {
                tracing::debug!(target: "copper_app::canvas", error = %e, "canvas post");
            }
        });
    }

    fn object(v: Value) -> Option<serde_json::Map<String, Value>> {
        match v {
            Value::Object(m) => Some(m),
            Value::String(s) => serde_json::from_str::<Value>(&s)
                .ok()
                .and_then(|v| match v {
                    Value::Object(m) => Some(m),
                    _ => None,
                }),
            _ => None,
        }
    }

    pub async fn apply(self: &Rc<Self>, ops: &[Value], agent: &Value) -> Result<Value, Failure> {
        self.wait_ready(READY_WITHIN).await?;
        // Checklist assignees are named against the canvas's people: the
        // page has them first (Mac `CanvasTools`).
        if self.chat_offered() && mentions_key(ops, "assignees") {
            self.prepare_chat().await;
        }
        let input = json!({"ops": ops, "as": agent}).to_string();
        let raw = self.run(&copper_pagejs::canvas::apply(&input)).await?;
        let mut out =
            Self::object(Value::String(raw)).ok_or("the canvas page gave back no result")?;
        out.entry("applied").or_insert(json!(0));
        out.entry("ids").or_insert(json!([]));
        out.entry("errors").or_insert(json!([]));
        Ok(Value::Object(out))
    }

    /// `canvas_read`: `opts` go to the page's `read()` as they are (`full`,
    /// and the paging / narrowing options of pages that have them).
    pub async fn read(self: &Rc<Self>, opts: &Value) -> Result<Value, Failure> {
        self.wait_ready(READY_WITHIN).await?;
        let raw = self.run(&copper_pagejs::canvas::read_with(opts)).await?;
        let mut out =
            Self::object(Value::String(raw)).ok_or("the canvas page gave back nothing to read")?;
        if let Some(error) = read_refused(&out) {
            return Err(Failure(error));
        }
        if let Some(entry) = self.entry() {
            let mut canvas = out
                .get("canvas")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            canvas.insert("id".into(), json!(entry.id));
            canvas.insert("name".into(), json!(entry.name));
            canvas.insert("kind".into(), json!(entry.kind));
            out.insert("canvas".into(), Value::Object(canvas));
        }
        let empty = out
            .get("selection")
            .and_then(Value::as_array)
            .is_none_or(|a| a.is_empty());
        if empty && !self.selection.borrow().is_empty() {
            out.insert("selection".into(), json!(*self.selection.borrow()));
        }
        Ok(Value::Object(out))
    }

    pub async fn select(self: &Rc<Self>, ids: &[String]) -> Result<(), Failure> {
        self.wait_ready(READY_WITHIN).await?;
        self.run(&copper_pagejs::canvas::select(ids)).await?;
        *self.selection.borrow_mut() = ids.to_vec();
        Ok(())
    }

    pub async fn zoom_to(self: &Rc<Self>, ids: &[String]) -> Result<(), Failure> {
        self.wait_ready(READY_WITHIN).await?;
        self.run(&copper_pagejs::canvas::zoom_to(ids)).await?;
        Ok(())
    }

    pub async fn presence(self: &Rc<Self>) -> Result<Value, Failure> {
        self.wait_ready(READY_WITHIN).await?;
        let raw = self.run(&copper_pagejs::canvas::presence()).await?;
        Ok(serde_json::from_str(&raw).unwrap_or(json!([])))
    }

    /// The board as it looks (PNG), and — when the page's `settled` timed
    /// out — what was still loading on screen when it was taken.
    ///
    /// The page is `ready` before its images decode (by design), so a cold
    /// board would come out with blank pictures: wait (bounded) for what is
    /// on screen first (canvas-perf contract §E). Pages without `settled`
    /// are pictured at once.
    ///
    /// A canvas tab nobody is looking at is a hidden view, and WebKit runs
    /// no rendering updates for a hidden view: its images never decode, so
    /// the picture would stay blank however long it waited. For the picture
    /// the page is held backstage (Mac: the Backstage window `picture` needs)
    /// — visible to WebKit, not on the user's screen — and let go after.
    /// Held from the start, so a board still being handed over already
    /// decodes what it shows.
    pub async fn picture(self: &Rc<Self>) -> Result<(Vec<u8>, Option<String>), Failure> {
        let hold = PictureHold::take(&self.tab);
        self.wait_ready(READY_WITHIN).await?;
        if hold.as_ref().is_some_and(|h| h.changed) {
            glib::timeout_future(Duration::from_millis(120)).await;
        }
        let pending = self.settle(SETTLE_MS).await;
        let page = self.page().ok_or("the canvas page isn't loaded")?;
        let shot = page
            .snapshot(SnapshotRegion::Visible)
            .await
            .map_err(|e| Failure(format!("couldn't take the picture: {e}")))?;
        Ok((shot.png, pending))
    }

    /// `copperCanvas.settled(ms)`, bounded here as well (a page that never
    /// answers costs at most `ms` + 2 s). `Some(note)` only when the page
    /// said it had not settled.
    async fn settle(self: &Rc<Self>, ms: u64) -> Option<String> {
        let call = copper_pagejs::canvas::settled(ms);
        let asked = Instant::now();
        let answer = select2(
            Box::pin(self.run(&call)),
            glib::timeout_future(Duration::from_millis(ms + 2000)),
        )
        .await;
        let waited = asked.elapsed().as_millis();
        match answer {
            Some(Ok(v)) => {
                tracing::debug!(target: "copper_app::canvas", answer = %v, waited, "settled");
                unsettled_note(&v)
            }
            Some(Err(e)) => {
                tracing::debug!(target: "copper_app::canvas", error = %e, "settled");
                None
            }
            None => {
                tracing::debug!(target: "copper_app::canvas", waited, "settled: no answer");
                None
            }
        }
    }

    pub async fn chat_send(self: &Rc<Self>, input: &str) -> Result<Value, Failure> {
        self.wait_ready(READY_WITHIN).await?;
        self.prepare_chat().await;
        let raw = self.run(&copper_pagejs::canvas::chat_send(input)).await?;
        Ok(match raw {
            Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
            other => other,
        })
    }

    // MARK: keeping the log short

    fn compact_if_needed(self: &Rc<Self>, force: bool) {
        if !self.is_ready() || self.compaction_unsupported.get() {
            return;
        }
        let store = self.store();
        if !force && !store.wants_compaction() {
            return;
        }
        if self
            .compacting
            .get()
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30))
        {
            return;
        }
        self.compacting.set(Some(Instant::now()));
        let covered = store.entries();
        let me = self.clone();
        glib::spawn_future_local(async move {
            match me.run(&copper_pagejs::canvas::export_state()).await {
                Ok(Value::String(b)) if !b.is_empty() => {
                    let state = unb64(&b);
                    if state.is_empty() {
                        me.compaction_unsupported.set(true);
                    } else {
                        store.compact(&state, covered);
                    }
                }
                Ok(_) => me.compaction_unsupported.set(true),
                Err(e) => tracing::debug!(target: "copper_app::canvas", error = %e, "compaction"),
            }
            me.compacting.set(None);
        });
    }

    /// The whole document written as the snapshot before the page goes
    /// (two seconds at most).
    pub async fn checkpoint(self: &Rc<Self>) {
        if !self.is_ready() || self.compaction_unsupported.get() {
            return;
        }
        let store = self.store();
        let covered = store.entries();
        let Some(page) = self.page() else { return };
        let call = copper_pagejs::canvas::export_state();
        let fut = issue(
            &page,
            &call.function_body(),
            &JsonMap::new(),
            &engine_world(call.world()),
        );
        let timeout = glib::timeout_future(Duration::from_secs(2));
        let out = select2(fut, timeout).await;
        if let Some(Ok(Value::String(b))) = out {
            let state = unb64(&b);
            if !state.is_empty() {
                store.compact(&state, covered);
            }
        }
    }

    /// The page's room is no longer the right one: checkpoint, reload.
    pub fn restart_page(self: &Rc<Self>) {
        if self.restarting.replace(true) {
            return;
        }
        self.stop_room();
        let me = self.clone();
        glib::spawn_future_local(async move {
            me.checkpoint().await;
            me.restarting.set(false);
            if let Some(p) = me.page() {
                p.reload(false);
            }
        });
    }

    // MARK: the room

    fn start_room(self: &Rc<Self>) {
        let Some(entry) = self.entry() else { return };
        let Some(remote) = self.room_id.borrow().clone() else {
            return;
        };
        if registry::room_for(&entry).as_deref() != Some(remote.as_str()) {
            self.restart_page();
            return;
        }
        if self
            .room
            .borrow()
            .as_ref()
            .is_some_and(|r| r.remote_id() == remote)
        {
            return;
        }
        let Some(cloud) = crate::cloud::get() else {
            return;
        };
        let rgen = self.room_gen.get() + 1;
        self.room_gen.set(rgen);
        let (room, mut rx) = {
            let _rt = crate::cloud::runtime().enter();
            CanvasRoom::start(cloud.client.clone(), &remote)
        };
        *self.room.borrow_mut() = Some(room);
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(ev) = rx.recv().await {
                let Some(me) = weak.upgrade() else { break };
                if me.room_gen.get() != rgen {
                    break;
                }
                me.room_event(ev);
            }
        });
    }

    /// The room goes.
    pub fn stop_room(&self) {
        self.room_gen.set(self.room_gen.get() + 1);
        if let Some(r) = self.room.borrow_mut().take() {
            r.stop();
        }
        self.synced.set(false);
    }

    fn room_event(self: &Rc<Self>, ev: RoomEvent) {
        match ev {
            RoomEvent::Open => self.room_changed(true),
            RoomEvent::Closed => self.room_changed(false),
            RoomEvent::Data(frame) => self.deliver(frame),
            RoomEvent::Gone(code) => {
                tracing::info!(target: "copper_app::canvas", code, "canvas room gone");
                self.room_changed(false);
                *self.room.borrow_mut() = None;
                super::refresh_soon();
            }
            RoomEvent::RefreshList => super::refresh_soon(),
        }
    }

    fn room_changed(self: &Rc<Self>, open: bool) {
        tracing::info!(target: "copper_app::canvas", canvas = %self.canvas_id.borrow(), open, "canvas room");
        self.held.borrow_mut().clear();
        self.synced.set(false);
        if open {
            self.room_fresh.set(true);
            if self.page_socket.get() == PageSocket::Waiting {
                self.tell_open();
            } else if !self.page_ever_asked.get() {
                let weak = Rc::downgrade(self);
                glib::timeout_add_local_once(Duration::from_secs(2), move || {
                    if let Some(me) = weak.upgrade()
                        && !me.page_ever_asked.get()
                    {
                        me.page_socket.set(PageSocket::Waiting);
                        me.tell_open();
                    }
                });
            }
        } else {
            self.room_fresh.set(false);
            self.peers.borrow_mut().clear();
            if self.page_socket.get() != PageSocket::None {
                self.page_socket.set(PageSocket::None);
                self.post(copper_pagejs::canvas::ws_closed());
            }
        }
        super::recount();
        self.status_now();
    }

    fn tell_open(self: &Rc<Self>) {
        if !self.room_open() || !self.is_ready() || self.page_socket.get() != PageSocket::Waiting {
            return;
        }
        self.page_socket.set(PageSocket::Open);
        self.room_fresh.set(false);
        let held: Vec<Vec<u8>> = std::mem::take(&mut *self.held.borrow_mut());
        self.post(copper_pagejs::canvas::ws_open());
        for f in held {
            self.hand(&f);
        }
    }

    fn deliver(self: &Rc<Self>, frame: Vec<u8>) {
        let (ok, dropped, inn) = self.counts.get();
        self.counts.set((ok, dropped, inn + 1));
        self.note_incoming(&frame);
        match self.page_socket.get() {
            PageSocket::Open => self.hand(&frame),
            _ => {
                if !self.room_fresh.get() {
                    return;
                }
                let mut held = self.held.borrow_mut();
                held.push(frame);
                let n = held.len();
                if n > 2000 {
                    held.drain(..n - 2000);
                }
            }
        }
    }

    /// One frame to the page's socket; its document updates go on disk first.
    fn hand(self: &Rc<Self>, frame: &[u8]) {
        let (updates, step2) = copper_cloud::canvas::wire::sync(frame);
        if !updates.is_empty() {
            let store = self.store();
            for u in &updates {
                store.append_remote(u);
                self.hand_to_siblings(u, fingerprint(u));
            }
            registry::touched(&self.canvas_id.borrow());
        }
        self.post(copper_pagejs::canvas::ws_message(&b64(frame)));
        if step2 && !self.synced.get() {
            self.synced.set(true);
            self.store().set_pending(0);
            super::recount();
            self.status_now();
        }
        if !updates.is_empty() && self.store().wants_compaction() {
            self.compact_if_needed(false);
        }
    }

    fn note_incoming(&self, frame: &[u8]) {
        let Some(states) = copper_cloud::canvas::wire::awareness(frame) else {
            return;
        };
        let now = Instant::now();
        let mut peers = self.peers.borrow_mut();
        for (client, state) in states {
            match state {
                None => {
                    peers.remove(&client);
                }
                Some(s) => {
                    let user = s
                        .get("user")
                        .and_then(|u| u.get("id"))
                        .and_then(Value::as_str)
                        .or_else(|| s.get("id").and_then(Value::as_str))
                        .map(String::from)
                        .unwrap_or_else(|| format!("client:{client}"));
                    peers.insert(client, (user, now));
                }
            }
        }
    }

    // MARK: status, share, chat

    /// `local`, `personal-synced`, `shared-live` or `shared-offline`, and how
    /// many changes wait to go.
    pub fn status(&self) -> Value {
        let entry = self.entry();
        let shared = entry.as_ref().is_some_and(is_shared);
        let personal = entry.as_ref().is_some_and(is_personal);
        let mode = if shared {
            if self.room_synced() {
                "shared-live"
            } else {
                "shared-offline"
            }
        } else if personal && self.room_synced() {
            "personal-synced"
        } else {
            "local"
        };
        let online = self.room_id.borrow().is_some();
        let pending = if (online || shared) && !self.room_synced() {
            self.store().pending()
        } else {
            0
        };
        json!({"mode": mode, "pending": pending})
    }

    fn status_later(self: &Rc<Self>) {
        if !self.is_ready() || self.status_soon.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(me) = weak.upgrade() {
                me.status_soon.set(false);
                me.status_now();
            }
        });
    }

    pub fn status_now(&self) {
        if !self.is_ready() {
            return;
        }
        let now = self.status();
        let key = (
            now["mode"].as_str().unwrap_or("").to_string(),
            now["pending"].as_u64().unwrap_or(0) as usize,
        );
        if self.told_status.borrow().as_ref() == Some(&key) {
            return;
        }
        *self.told_status.borrow_mut() = Some(key);
        self.post(copper_pagejs::canvas::set_status(&now));
    }

    pub fn share_now(self: &Rc<Self>) {
        if !self.is_ready() {
            return;
        }
        let entry = self.entry();
        let can = entry.as_ref().is_some_and(is_shared) && registry::cloud_ready();
        let mut value = serde_json::Map::new();
        value.insert("canShare".into(), json!(can));
        if let Some(m) = entry.as_ref().and_then(|e| e.members) {
            value.insert("members".into(), json!(m));
        }
        if !can {
            let reason = match &entry {
                Some(e) if is_personal(e) => "Personal canvases can't be shared",
                Some(e) if registry::is_local(e) => "This canvas is on this device only",
                _ => "Sign in to Copper Cloud to share",
            };
            value.insert("reason".into(), json!(reason));
        }
        self.post(copper_pagejs::canvas::set_share(&Value::Object(value)));
        self.tell_chat();
    }

    fn chat_offered(&self) -> bool {
        self.entry()
            .is_some_and(|e| is_shared(&e) && e.remote_id.is_some() && registry::cloud_ready())
    }

    fn chat_payload(&self) -> Value {
        if !self.chat_offered() {
            return json!({"enabled": false, "open": false});
        }
        let mentions = crate::cloud::get()
            .and_then(|c| c.client.supports(copper_cloud::client::Feature::Mentions))
            == Some(true);
        let pref = super::chatprefs::get(&registry::identity().0, &self.canvas_id());
        let open = pref.open.unwrap_or(self.chat_open.get());
        self.chat_open.set(open);
        let mut out = json!({
            "enabled": true, "open": open, "mentionsApi": mentions,
            "mentioned": [], "invited": [],
        });
        if let Some(m) = self.members.borrow().as_ref() {
            out["members"] = json!(m);
        }
        if pref.read_id.is_some() || pref.read_at.is_some() {
            out["lastReadId"] = json!(pref.read_id);
            out["lastReadAt"] = json!(pref.read_at);
        } else {
            out["known"] = json!(false);
        }
        out
    }

    /// Tell the page where chat stands; members are read when missing.
    fn tell_chat(self: &Rc<Self>) {
        if !self.is_ready() {
            return;
        }
        self.post(copper_pagejs::canvas::set_chat(&self.chat_payload()));
        if self.chat_offered() && self.members.borrow().is_none() {
            let me = self.clone();
            glib::spawn_future_local(async move {
                me.load_members().await;
                me.post(copper_pagejs::canvas::set_chat(&me.chat_payload()));
            });
        }
    }

    async fn load_members(&self) {
        let (Some(cloud), Some(remote)) =
            (crate::cloud::get(), self.entry().and_then(|e| e.remote_id))
        else {
            return;
        };
        let api = cloud.canvas.clone();
        let Ok(Ok(list)) = crate::cloud::spawn(async move { api.members(&remote).await }).await
        else {
            return;
        };
        let people: Vec<Value> = list
            .iter()
            .map(|m| {
                let id = m.user_id.to_ascii_lowercase();
                json!({
                    "id": id,
                    "name": m.display_name.clone().filter(|n| !n.is_empty()).or(m.email.clone()).unwrap_or_default(),
                    "email": m.email.clone().unwrap_or_default(),
                    "color": super::stable_color(&id),
                })
            })
            .collect();
        *self.members.borrow_mut() = Some(people);
        self.members_read.set(Some(Instant::now()));
    }

    async fn prepare_chat(&self) {
        let stale = self
            .members_read
            .get()
            .is_none_or(|t| t.elapsed() > Duration::from_secs(60));
        if self.members.borrow().is_none() || stale {
            self.load_members().await;
        }
        let _ = self
            .run(&copper_pagejs::canvas::set_chat(&self.chat_payload()))
            .await;
    }

    fn mention(&self, body: &serde_json::Map<String, Value>) {
        let (Some(message), Some(remote)) = (
            body.get("messageId")
                .and_then(Value::as_str)
                .map(String::from),
            self.entry().and_then(|e| e.remote_id),
        ) else {
            return;
        };
        let users: Vec<String> = body
            .get("userIds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let excerpt: String = body
            .get("excerpt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        let Some(cloud) = crate::cloud::get() else {
            return;
        };
        // Nobody to tell, or a cloud without mentions (Mac: `mentionsApi != false`).
        if users.is_empty()
            || cloud
                .client
                .supports(copper_cloud::client::Feature::Mentions)
                == Some(false)
        {
            return;
        }
        let api = cloud.canvas.clone();
        crate::cloud::spawn(async move {
            if let Err(e) = api
                .notify_mentions(&remote, &message, &users, &excerpt)
                .await
            {
                tracing::debug!(target: "copper_app::canvas", error = %e.message, "mention notify");
            }
        });
    }

    // MARK: live frames' posters (super::poster)

    /// `{type: 'framePoster', …}`: queue a picture of that frame.
    fn frame_poster(self: &Rc<Self>, body: &serde_json::Map<String, Value>) {
        use super::poster::{Offer, parse};
        let request = match parse(body) {
            Ok(r) => r,
            Err(Some(name)) => {
                self.answer_poster(&name, None);
                return;
            }
            Err(None) => return,
        };
        match self.posters.offer(request, Instant::now()) {
            Offer::Refuse(names) => {
                for n in names {
                    self.answer_poster(&n, None);
                }
            }
            Offer::Queued => {
                if !self.posters.busy.replace(true) {
                    let me = self.clone();
                    glib::spawn_future_local(async move {
                        me.poster_batches().await;
                        me.posters.busy.set(false);
                    });
                }
            }
        }
    }

    fn answer_poster(&self, name: &str, url: Option<&str>) {
        self.posters.answered(name, url.is_some(), Instant::now());
        self.post(copper_pagejs::canvas::frame_poster(name, url));
    }

    async fn poster_batches(self: &Rc<Self>) {
        loop {
            // Requests that come together share one picture of the board.
            glib::timeout_future(Duration::from_millis(60)).await;
            let batch = self.posters.take();
            if batch.is_empty() {
                return;
            }
            let t0 = Instant::now();
            let answers = self.poster_batch(&batch).await;
            let made = answers.iter().filter(|(_, u)| u.is_some()).count();
            tracing::debug!(target: "copper_app::canvas", asked = batch.len(), made, ms = t0.elapsed().as_millis() as u64, "frame posters");
            for (name, url) in answers {
                self.answer_poster(&name, url.as_deref());
            }
        }
    }

    /// One snapshot of the board, each frame's rect cut out of it and
    /// encoded on a worker thread.
    async fn poster_batch(
        &self,
        batch: &[super::poster::Request],
    ) -> Vec<(String, Option<String>)> {
        use super::poster;
        let none = || -> Vec<(String, Option<String>)> {
            batch.iter().map(|r| (r.name.clone(), None)).collect()
        };
        let Some(page) = self.page() else {
            return none();
        };
        let widget = page.widget();
        let (ww, wh) = (widget.width(), widget.height());
        if !widget.is_mapped() || ww < 16 || wh < 16 {
            // Not on screen: the page's frames aren't either.
            return none();
        }
        let tex = match page.snapshot_texture().await {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "copper_app::canvas", error = %e, "frame poster snapshot");
                return none();
            }
        };
        if self.page().is_none_or(|p| p.widget() != widget) {
            return none();
        }
        let (tw, th) = (tex.width().max(0) as u32, tex.height().max(0) as u32);
        // CSS px → widget px (page zoom) → device px (the snapshot's scale).
        let scale = f64::from(tw) / f64::from(ww) * page.zoom().max(0.01);
        let jobs: Vec<(String, Option<super::poster::Rect>, u32)> = batch
            .iter()
            .map(|r| {
                (
                    r.name.clone(),
                    poster::device_rect(r.rect, scale, tw, th),
                    r.max_width,
                )
            })
            .collect();
        if jobs.iter().all(|(_, r, _)| r.is_none()) {
            return none();
        }
        let mut down = gdk4::TextureDownloader::new(&tex);
        down.set_format(gdk4::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = down.download_bytes();
        let work = gio::spawn_blocking(move || {
            let board = poster::rgba(tw, th, stride, &bytes);
            jobs.into_iter()
                .map(|(name, rect, max_width)| {
                    let url = board
                        .as_ref()
                        .zip(rect)
                        .and_then(|(b, r)| poster::render(b, r, max_width));
                    (name, url)
                })
                .collect::<Vec<_>>()
        });
        work.await.unwrap_or_else(|_| none())
    }

    fn frame_report(&self, body: Value) {
        let kind = body.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        tracing::debug!(target: "copper_app::canvas", kind, "frame report");
        let mut ev = self.frame_events.borrow_mut();
        ev.push(body.clone());
        let n = ev.len();
        if n > 50 {
            ev.drain(..n - 50);
        }
        drop(ev);
        if self.is_ready() {
            self.post(copper_pagejs::canvas::frame_event(&body));
        }
    }

    /// The window went light or dark.
    pub fn theme_changed(&self) {
        if self.is_ready() {
            self.post(copper_pagejs::canvas::theme(&super::theme()));
        }
    }

    /// Renamed here or on the cloud: the page is told (or reloads).
    pub fn renamed(self: &Rc<Self>) {
        let Some(entry) = self.entry() else { return };
        if !self.is_ready() {
            return;
        }
        let me = self.clone();
        glib::spawn_future_local(async move {
            let told = me
                .run(&copper_pagejs::canvas::rename(&entry.name))
                .await
                .unwrap_or(false);
            if !told {
                me.restart_page();
            }
        });
    }

    /// For the bench and `canvas_list`.
    pub fn describe(&self) -> Value {
        json!({
            "canvas": self.canvas_id(), "ready": self.is_ready(), "generation": self.generation.get(),
            "online": self.room_id.borrow().is_some(), "room": self.room_open(), "synced": self.room_synced(),
            "roomId": *self.room_id.borrow(), "status": self.status(), "collaborators": self.collaborators(),
            "selection": *self.selection.borrow(), "store": self.store().describe(),
            "clients": self.peers.borrow().len(), "lastError": *self.last_error.borrow(),
            "tab": self.tab.0, "socket": format!("{:?}", self.page_socket.get()),
            "framesOut": self.counts.get().0, "framesDropped": self.counts.get().1, "framesIn": self.counts.get().2,
            "stage": format!("{:?}", self.stage()), "presence": *self.presence.borrow(),
            "frameReports": self.frame_events.borrow().len(),
            "posters": self.posters.counts.get().0, "postersNull": self.posters.counts.get().1,
        })
    }

    /// The room or the store may have to change after a sign-in, a sign-out,
    /// another account, a sync switch or the list read.
    pub fn cloud_changed(self: &Rc<Self>) {
        let Some(entry) = self.entry() else { return };
        if !self.is_ready() {
            return;
        }
        let wants = registry::room_for(&entry);
        let key = registry::store_key(&entry);
        if wants != *self.room_id.borrow() || key != *self.store_key.borrow() {
            self.restart_page();
        } else {
            self.status_now();
            self.share_now();
        }
    }
}

/// A background canvas tab held backstage for one picture; let go when
/// dropped, unless an agent's own hold was there first.
struct PictureHold {
    tab: TabId,
    /// Taken here (not an agent's hold already in place).
    ours: bool,
    /// The page's visibility just changed: give WebKit a beat.
    changed: bool,
}

impl PictureHold {
    fn take(tab: &TabId) -> Option<PictureHold> {
        let shell = super::shell()?;
        let agents = crate::agents::agents()?;
        if shell.active_id().as_ref() == Some(tab) {
            return None;
        }
        let before = agents.stage.holds(tab);
        let changed = agents.stage.prepare(&shell, tab);
        Some(PictureHold {
            tab: tab.clone(),
            ours: !before && agents.stage.holds(tab),
            changed,
        })
    }
}

impl Drop for PictureHold {
    fn drop(&mut self) {
        if !self.ours {
            return;
        }
        if let (Some(shell), Some(agents)) = (super::shell(), crate::agents::agents()) {
            agents.stage.release(&shell, &self.tab);
        }
    }
}

/// What a picture waits for what's on screen (contract §E: `settled(2500)`).
pub const SETTLE_MS: u64 = 2500;

/// What was still pending when `settled` timed out, in the Mac's words
/// (`CanvasHost.pendingText`): "3 images, 1 web frame". `None` when the page
/// settled, predates `settled` (`null`), or named nothing pending.
pub fn unsettled_note(answer: &Value) -> Option<String> {
    if answer.get("settled").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let pending = answer.get("pending");
    let count = |k: &str| {
        pending
            .and_then(|p| p.get(k))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let parts: Vec<String> = [
        (count("images"), "image", "images"),
        (count("frames"), "web frame", "web frames"),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The page refused a read's options (an unknown frame, a bad region, a
/// stale or foreign cursor): its words are the answer, as a tool error (Mac
/// `CanvasHost.read`). An answer with shapes or an overview is never refused.
pub fn read_refused(out: &serde_json::Map<String, Value>) -> Option<String> {
    let error = out.get("error").and_then(Value::as_str)?;
    (!out.contains_key("shapes") && !out.contains_key("overview")).then(|| error.to_string())
}

/// Whether any op carries `key` at any depth (checklist `assignees` in an
/// add's shape or an update's patch; Mac `CanvasTools.mentions`).
pub fn mentions_key(ops: &[Value], key: &str) -> bool {
    fn walk(v: &Value, key: &str) -> bool {
        match v {
            Value::Object(m) => m.iter().any(|(k, v)| k == key || walk(v, key)),
            Value::Array(a) => a.iter().any(|v| walk(v, key)),
            _ => false,
        }
    }
    ops.iter().any(|op| walk(op, key))
}

/// The first of two futures to finish (`None` when the second did).
async fn select2<A, B>(a: Pin<Box<A>>, b: B) -> Option<A::Output>
where
    A: Future + ?Sized,
    B: Future,
{
    let mut a = a;
    let mut b = Box::pin(b);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(Some(v));
        }
        if b.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}
