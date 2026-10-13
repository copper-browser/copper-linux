//! What the MCP tools need from the browser: the [`Host`] (tabs, windows,
//! vault, canvas, cloud) and a [`Page`] per tab (script, pixels, input).
//!
//! The app implements both on the GTK main thread. Every method is called
//! from the thread that polls [`crate::Mcp::serve`], and the futures they
//! return are polled there too — none of them needs to be `Send`.
//!
//! Defaults: everything a first build can live without (vault fills,
//! canvases, Copper Cloud, crash history, process heat, drive events) has a
//! default that answers "not available" or does nothing, so a host can wire
//! the core first and the rest as it lands.

use std::future::Future;
use std::time::Duration;

use copper_core::cloud::CloudProvided;
use copper_core::intelligence::IntelligenceKeys;
use copper_engine::{Image, InputEvent, SnapshotRegion};
use copper_pagejs::jev::Extras;
use serde_json::{Map, Value};

pub use copper_core::TabId;

use crate::drive::DriveEvent;

/// One tab, as the tools see it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabInfo {
    pub id: TabId,
    /// Position in the row of the window agents act in (the one
    /// `browser_tabs list` numbers); `None` for a tab in another window, in
    /// another space, or parked.
    pub index: Option<usize>,
    pub title: String,
    /// `None` reads as `about:blank`.
    pub url: Option<String>,
    pub pinned: bool,
    /// The tab group / folder it is in, by name.
    pub group: Option<String>,
    /// Asleep (no web view right now); [`Host::prepare_tab`] wakes it.
    pub sleeping: bool,
    /// On the user's screen: the active tab of some window, or in a split.
    /// `AgentTabs.onScreen` on the Mac.
    pub selected: bool,
    /// A load is in flight.
    pub loading: bool,
    /// Sustained CPU of its web process, percent of a core (~10 s), when known.
    pub cpu: Option<f64>,
}

impl TabInfo {
    /// The address, or `about:blank`.
    pub fn url_or_blank(&self) -> &str {
        self.url.as_deref().unwrap_or("about:blank")
    }
}

/// A tab group and the titles of its tabs, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupInfo {
    pub name: String,
    pub members: Vec<String>,
}

/// What a tool answers: text, or a picture (base64 on the wire).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image { data: Vec<u8>, mime: String },
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text(s.into())
    }

    /// The MCP content block.
    pub fn json(&self) -> Value {
        use base64::Engine as _;
        match self {
            Content::Text(s) => serde_json::json!({ "type": "text", "text": s }),
            Content::Image { data, mime } => serde_json::json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(data),
                "mimeType": mime,
            }),
        }
    }
}

/// Model keys as this machine has them, and what Copper Cloud provides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntelligenceSource {
    pub local: IntelligenceKeys,
    pub cloud: Option<CloudProvided>,
}

/// One web page: the tools' only way into a tab's content.
///
/// `copper_engine::PageView` already has every piece; [`EnginePage`] adapts
/// one. Coordinates are CSS pixels from the viewport's top-left, the space of
/// `getBoundingClientRect()` and of [`InputEvent`].
pub trait Page: Clone + 'static {
    /// Run `body` as an async function body in `world` (`None`: the page's
    /// own world) and resolve to its JSON result (`null` for `undefined`).
    /// Every curated call goes through here
    /// (`copper_pagejs::Call::function_body`). A script error is
    /// `Err("JavaScript: <message>")`.
    fn call_async(
        &self,
        body: &str,
        world: Option<&str>,
    ) -> impl Future<Output = Result<Value, String>>;

    /// Paint the page into a PNG (the agent's screenshot).
    fn snapshot(&self, region: SnapshotRegion) -> impl Future<Output = Result<Image, String>>;

    /// Deliver real input (see `PageView::input`).
    fn input(&self, event: InputEvent) -> impl Future<Output = Result<(), String>>;

    /// Page zoom (1.0 = 100 %).
    fn zoom(&self) -> f64;

    /// The view's size in logical pixels (its GTK allocation): CSS pixels
    /// times the zoom. Screenshots come back at this size.
    fn view_size(&self) -> (f64, f64);

    /// Whether input can go in as real events (`Input.canPost`): the view is
    /// mapped somewhere, on screen or backstage. When false the tools use the
    /// page script's DOM events instead.
    fn can_post_input(&self) -> bool {
        true
    }
}

/// The browser, as the agent tools drive it. See the module docs for
/// threading. Implemented by the app; tests use a fake.
pub trait Host: 'static {
    type Page: Page;

    // -- tabs --------------------------------------------------------------

    /// Every tab in every window and space, once each. Those in the agent
    /// window's row carry their `index`; `browser_tabs list` prints that row
    /// in index order, then the rest (other spaces and windows) in this order.
    fn tabs(&self) -> Vec<TabInfo>;

    /// The tab on the user's screen in the agent window (`browser.active`).
    fn active_tab(&self) -> Option<TabId>;

    /// One tab by id, wherever it is.
    fn tab(&self, id: &TabId) -> Option<TabInfo> {
        self.tabs().into_iter().find(|t| &t.id == id)
    }

    /// Open `url` in a new tab of the agent window. `foreground: false`
    /// opens it behind the user's tab — their view does not change.
    fn open_tab(&self, url: &str, foreground: bool) -> TabId;

    /// Put a tab in front of the user, on purpose (`AgentTabs.show`).
    fn show_tab(&self, id: &TabId);

    /// Close a tab wherever it is.
    fn close_tab(&self, id: &TabId);

    /// Make a tab ready to be worked on: awake, built, and — if nobody is
    /// looking at it — hung backstage so it paints and takes input
    /// (`AgentTabs.prepare` / `Backstage.stage`). Called before every call on
    /// the tab. Returns true when the page's visibility just changed, so the
    /// caller gives WebKit a beat before reading it.
    fn prepare_tab(&self, id: &TabId) -> bool;

    /// The agents are done with a backstage tab (5 minutes without a call,
    /// or it closed): take it back out and let it be a background tab again.
    fn release_tab(&self, id: &TabId) {
        let _ = id;
    }

    /// The tab's page, once it has one (after [`Host::prepare_tab`]).
    fn page(&self, id: &TabId) -> Option<Self::Page>;

    /// Navigate the tab (`Tab.go(to:)`): `url` is already a full address.
    fn navigate(&self, id: &TabId, url: &str);
    fn go_back(&self, id: &TabId);
    fn go_forward(&self, id: &TabId);

    /// A load is in flight in the tab.
    fn is_loading(&self, id: &TabId) -> bool {
        self.tab(id).is_some_and(|t| t.loading)
    }

    /// Resize the agent window's frame.
    fn resize_window(&self, width: f64, height: f64) -> Result<(), String>;

    // -- groups --------------------------------------------------------------

    fn groups(&self) -> Vec<GroupInfo> {
        Vec::new()
    }
    /// Put the tab in the group called `name`, made if new.
    fn group_assign(&self, id: &TabId, name: &str) {
        let _ = (id, name);
    }
    fn group_remove(&self, id: &TabId) {
        let _ = id;
    }

    // -- timing and the user ---------------------------------------------------

    /// Resolve after `duration` (on the host's loop: `glib::timeout_future`).
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()>;

    /// The status line at the bottom of the window ("Agent · browser_click").
    fn announce(&self, text: &str) {
        let _ = text;
    }

    /// The driver pane, tab badges and pill follow these.
    fn drive(&self, event: &DriveEvent) {
        let _ = event;
    }

    /// GPU process CPU, percent of a core, for the tab list.
    fn gpu_percent(&self) -> Option<f64> {
        None
    }

    /// `## Processes (% of one core)` lines for `browser_perf_probe`
    /// (Markdown, ending in a blank line), or empty.
    fn processes_line(&self, id: &TabId) -> String {
        let _ = id;
        String::new()
    }

    // -- vault -------------------------------------------------------------------

    /// `browser_sign_in`: fill (and submit) a shared saved account in the
    /// tab. The answer object is printed with sorted keys; it never holds a
    /// secret.
    fn sign_in(
        &self,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Value, String>> {
        let _ = (id, args);
        async { Err("Saved sign-ins are not available in this build yet".to_string()) }
    }

    /// `browser_autofill`: fill a shared card, identity or field.
    fn autofill(
        &self,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Value, String>> {
        let _ = (id, args);
        async { Err("Autofill from the vault is not available in this build yet".to_string()) }
    }

    /// The agent-only controls Jev may offer on this tab: SIGN_IN when it has
    /// a password field and a shared account for its host, AUTOFILL_* when
    /// matching fields and a shared item exist (`Ultrafast.availability`).
    fn jev_extras(&self, id: &TabId) -> impl Future<Output = Extras> {
        let _ = id;
        async { Extras::default() }
    }

    // -- canvases, cloud, crashes, controls ---------------------------------------

    /// Every `canvas_*` tool (`CanvasTools.call`).
    fn canvas(
        &self,
        tool: &str,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Vec<Content>, String>> {
        let _ = (tool, args);
        async { Err("Canvases are not available in this build yet".to_string()) }
    }

    /// `cloud_history_delete` (`CloudHistoryTool.call`).
    fn cloud_history_delete(
        &self,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Vec<Content>, String>> {
        let _ = args;
        async { Err("Copper Cloud history is not available in this build yet".to_string()) }
    }

    /// `browser_crashes`: the crash index as JSON text (`Crashes.answer`).
    fn crashes(&self, args: &Map<String, Value>) -> Result<String, String> {
        let _ = args;
        Err("Crash history is not available in this build yet".to_string())
    }

    /// The loopback-only `copper/link`, `copper/intelligence`,
    /// `copper/claude` and `copper/bitwarden` methods (the CLI's controls).
    /// `method` is the full name. The result never holds a secret.
    fn control(&self, method: &str, params: &Map<String, Value>) -> impl Future<Output = Value> {
        let _ = params;
        let text = format!("{method} is not available in this build yet");
        async move { serde_json::json!({ "error": text }) }
    }

    // -- models ------------------------------------------------------------------

    /// This machine's keys (`intelligence.json`) and the cloud's
    /// (`cloud-intelligence.json`). The default reads both files.
    fn intelligence(&self) -> IntelligenceSource {
        crate::intelligence::read_files(&copper_core::Store::from_env())
    }

    /// The Claude-account lane (`Claude.ask`): one system + user message,
    /// answered as text with the model that answered. Only used when the
    /// effective lane is `claude`.
    fn claude_ask(
        &self,
        system: &str,
        user: &str,
        model: &str,
        max_tokens: u32,
        timeout: Duration,
    ) -> impl Future<Output = Result<(String, String), String>> {
        let _ = (system, user, model, max_tokens, timeout);
        async { Err("The Claude account lane is not available in this build yet".to_string()) }
    }

    /// Whether the Claude account is signed in (the `claude` lane's readiness).
    fn claude_ready(&self) -> bool {
        false
    }
}

/// [`Page`] for any `copper_engine::PageView`, plus the two things the
/// engine trait does not know: the view's size and whether it is mapped.
pub struct EnginePage<P> {
    pub page: std::rc::Rc<P>,
    /// The view's size in logical pixels (the widget's allocation).
    pub view_size: std::rc::Rc<dyn Fn() -> (f64, f64)>,
    /// The view has a surface (on screen or backstage).
    pub mapped: std::rc::Rc<dyn Fn() -> bool>,
}

impl<P> Clone for EnginePage<P> {
    fn clone(&self) -> Self {
        EnginePage {
            page: self.page.clone(),
            view_size: self.view_size.clone(),
            mapped: self.mapped.clone(),
        }
    }
}

fn engine_error(e: copper_engine::EngineError) -> String {
    match e {
        copper_engine::EngineError::JavaScript(m) => format!("JavaScript: {m}"),
        other => other.to_string(),
    }
}

impl<P: copper_engine::PageView + 'static> Page for EnginePage<P> {
    fn call_async(
        &self,
        body: &str,
        world: Option<&str>,
    ) -> impl Future<Output = Result<Value, String>> {
        let world = match world {
            Some(name) => copper_engine::World::isolated(name),
            None => copper_engine::World::Page,
        };
        let future = self.page.call_async(body, &Map::new(), &world);
        async move { future.await.map_err(engine_error) }
    }

    fn snapshot(&self, region: SnapshotRegion) -> impl Future<Output = Result<Image, String>> {
        let future = self.page.snapshot(region);
        async move { future.await.map_err(engine_error) }
    }

    fn input(&self, event: InputEvent) -> impl Future<Output = Result<(), String>> {
        let future = self.page.input(event);
        async move { future.await.map(|_trust| ()).map_err(engine_error) }
    }

    fn zoom(&self) -> f64 {
        self.page.zoom()
    }

    fn view_size(&self) -> (f64, f64) {
        (self.view_size)()
    }

    fn can_post_input(&self) -> bool {
        (self.mapped)()
    }
}
