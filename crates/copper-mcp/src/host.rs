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
use copper_engine::{
    Capture, CaptureRequest, ChosenFiles, Cookie, DialogHandler, DialogId, FileChooserHandler,
    GeoPosition, InputEvent, NetLog,
};
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

/// A space, as `browser_spaces` lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpaceInfo {
    /// UUID, as the session keeps it.
    pub id: String,
    pub name: String,
    /// Its cookie jar, when it has one of its own.
    pub profile: Option<String>,
    /// Loose tabs in its row.
    pub tabs: usize,
    /// The space the window shows.
    pub on_screen: bool,
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

/// What the person said to a `browser_handoff` bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffAnswer {
    Done,
    NotNow,
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

    /// Paint an area of the page at a scale, as raw pixels, for the
    /// agent's screenshot (see `PageView::capture`).
    fn capture(&self, request: CaptureRequest) -> impl Future<Output = Result<Capture, String>>;

    /// Deliver real input (see `PageView::input`).
    fn input(&self, event: InputEvent) -> impl Future<Output = Result<(), String>>;

    /// Page zoom (1.0 = 100 %).
    fn zoom(&self) -> f64;

    /// The view's size in logical pixels (its GTK allocation): CSS pixels
    /// times the zoom. A scale-1 screenshot of the viewport has this size.
    fn view_size(&self) -> (f64, f64);

    /// Whether input can go in as real events (`Input.canPost`): the view is
    /// mapped somewhere, on screen or backstage. When false the tools use the
    /// page script's DOM events instead.
    fn can_post_input(&self) -> bool {
        true
    }

    // -- page tools (tools-page) ----------------------------------------------
    // `copper_engine::PageView`'s "agents' page tools" methods; the defaults
    // are a page that has none of them.

    /// Who takes the page's questions first (`PageView::set_dialog_handler`).
    fn set_dialog_handler(&self, handler: Option<DialogHandler>) {
        let _ = handler;
    }
    /// Answer a held question (`PageView::answer_dialog`).
    fn answer_dialog(&self, id: DialogId, accept: bool, text: Option<&str>) -> bool {
        let _ = (id, accept, text);
        false
    }
    /// Who takes the page's file choosers first.
    fn set_file_chooser_handler(&self, handler: Option<FileChooserHandler>) {
        let _ = handler;
    }
    /// Answer the held file chooser (`None`: cancel).
    fn answer_file_chooser(&self, files: ChosenFiles) -> bool {
        let _ = files;
        false
    }
    /// Keep the page's requests from now on.
    fn watch_network(&self) {}
    /// The page's requests, once watched.
    fn network(&self) -> Option<NetLog> {
        None
    }
    /// Print the page to a paged PDF at `path`.
    fn print_pdf(&self, path: &std::path::Path) -> impl Future<Output = Result<(), String>> {
        let _ = path;
        std::future::ready(Err("this build can't print pages".to_string()))
    }
    /// Whether one of Copper's own scripts is waiting on this page right
    /// now (`Tools.Page.waiting`): a question held then would hold it too.
    fn waiting(&self) -> bool {
        false
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
    /// Go back (forward) in the tab's history; false when there is no page
    /// to go to, and nothing happened.
    fn go_back(&self, id: &TabId) -> bool;
    fn go_forward(&self, id: &TabId) -> bool;

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

    // -- browser state (tools-state) ----------------------------------------------

    /// The user's history, newest first (`browser_history`): `query` as the
    /// History panel searches it (`History::search`), or the most recent
    /// with none; at most `limit` rows.
    fn history(&self, query: &str, limit: usize) -> Vec<copper_core::history::Trace> {
        let _ = (query, limit);
        Vec::new()
    }

    /// The bookmarks tree (`browser_bookmarks`).
    fn bookmarks(&self) -> Vec<copper_core::Bookmark> {
        Vec::new()
    }

    /// Keep `url` at the end of the bookmarks; `false` when it is kept already.
    fn add_bookmark(&self, url: &str, title: &str) -> bool {
        let _ = (url, title);
        false
    }

    /// Every space, in order (`browser_spaces`).
    fn spaces(&self) -> Vec<SpaceInfo> {
        Vec::new()
    }

    /// The id of the space a tab is in; `None` for a pin or no such tab.
    fn space_of(&self, tab: &TabId) -> Option<String> {
        let _ = tab;
        None
    }

    /// Move a loose tab to the end of another space's row.
    fn move_to_space(&self, tab: &TabId, space: &str) -> bool {
        let _ = (tab, space);
        false
    }

    // -- vault save (tools-state) --

    /// `browser_vault_save` / `browser_vault_note` and the CLI's
    /// `bitwarden save -|note -`: `op` save | note. Answers
    /// `{ok, id, created, updated, name, folder}` or `{ok:false, error}`,
    /// never a value it was given (`Bitwarden.vaultWrite`).
    fn vault_write(&self, op: &str, args: &Map<String, Value>) -> impl Future<Output = Value> {
        let _ = (op, args);
        async {
            serde_json::json!({ "ok": false, "error": "passwords are not available in this build" })
        }
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

    /// A canvas is in use (`CanvasTools.inUse`): one made by or shared with
    /// the user, or a board kept on this device. Until then tools/list offers
    /// only `catalogue::CANVAS_DOORS`.
    fn canvas_in_use(&self) -> bool {
        true
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

    // -- downloads (tools-state) ---------------------------------------------------

    /// `browser_downloads`: this session's downloads tab `tab` started (or
    /// a popup it opened), newest first, and the downloads folder.
    fn tab_downloads(&self, tab: &TabId) -> crate::downloads::TabDownloads {
        let _ = tab;
        crate::downloads::TabDownloads::default()
    }

    // -- handoff (tools-state) ----------------------------------------------------

    /// `browser_handoff` (`Handoff.ask` + `wait`): the bar in the window that
    /// holds `tab` asks the person to do a step (one ask per tab: a second
    /// call updates its message), and the call waits up to `timeout`.
    /// `Ok(None)`: no answer in time, the bar stays up. The tab is not
    /// touched; only the person's Show goes to it.
    fn handoff(
        &self,
        tab: &TabId,
        agent: &str,
        message: &str,
        timeout: Duration,
    ) -> impl Future<Output = Result<Option<HandoffAnswer>, String>> {
        let _ = (tab, agent, message, timeout);
        async { Err("Handing a step to the user is not available in this build yet".to_string()) }
    }

    // -- page tools (tools-page) ----------------------------------------------

    /// Run `task` on the host's loop, on its own (a held question's or file
    /// chooser's time limit). The default drops it: no time limit.
    fn spawn(&self, task: std::pin::Pin<Box<dyn Future<Output = ()>>>) {
        drop(task);
    }

    /// Where `browser_pdf_save` puts a PDF it is not told a place for.
    fn downloads_folder(&self) -> std::path::PathBuf {
        copper_core::platform::home_dir()
            .unwrap_or_default()
            .join("Downloads")
    }

    /// Every cookie in the tab's cookie jar (its profile's).
    fn cookie_jar(&self, id: &TabId) -> impl Future<Output = Result<Vec<Cookie>, String>> {
        let _ = id;
        std::future::ready(Err("cookies are not available in this build".to_string()))
    }
    /// Set a cookie in the tab's jar.
    fn set_cookie(&self, id: &TabId, cookie: Cookie) -> impl Future<Output = Result<(), String>> {
        let _ = (id, cookie);
        std::future::ready(Err("cookies are not available in this build".to_string()))
    }
    /// Delete a cookie from the tab's jar.
    fn delete_cookie(
        &self,
        id: &TabId,
        cookie: Cookie,
    ) -> impl Future<Output = Result<(), String>> {
        let _ = (id, cookie);
        std::future::ready(Err("cookies are not available in this build".to_string()))
    }
    /// Whether the user shared `url`'s site with agents (Settings ›
    /// Passwords › Agent access: every account, or one for the site) — the
    /// gate on HttpOnly cookies, as on `browser_sign_in`.
    fn site_shared_with_agents(&self, url: &str) -> impl Future<Output = bool> {
        let _ = url;
        std::future::ready(false)
    }

    /// Whether `site` (`host` or `host:port`) is allowed `kind`
    /// (`geolocation`, `camera`, `microphone`).
    fn site_permission(&self, site: &str, kind: &str) -> bool {
        let _ = (site, kind);
        false
    }
    /// Allow `kind` for `site`, or (`false`) forget it so the site is asked.
    fn set_site_permission(&self, site: &str, kind: &str, allowed: bool) {
        let _ = (site, kind, allowed);
    }
    /// The position pages granted geolocation are told.
    fn geolocation(&self) -> Option<GeoPosition> {
        None
    }
    fn set_geolocation(&self, position: Option<GeoPosition>) {
        let _ = position;
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
        let page = self.page.page_id();
        async move {
            let _waiting = Waiting::enter(page);
            future.await.map_err(engine_error)
        }
    }

    fn capture(&self, request: CaptureRequest) -> impl Future<Output = Result<Capture, String>> {
        let future = self.page.capture(request);
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

    fn set_dialog_handler(&self, handler: Option<DialogHandler>) {
        self.page.set_dialog_handler(handler);
    }

    fn answer_dialog(&self, id: DialogId, accept: bool, text: Option<&str>) -> bool {
        self.page.answer_dialog(id, accept, text)
    }

    fn set_file_chooser_handler(&self, handler: Option<FileChooserHandler>) {
        self.page.set_file_chooser_handler(handler);
    }

    fn answer_file_chooser(&self, files: ChosenFiles) -> bool {
        self.page.answer_file_chooser(files)
    }

    fn watch_network(&self) {
        self.page.watch_network();
    }

    fn network(&self) -> Option<NetLog> {
        self.page.network()
    }

    fn print_pdf(&self, path: &std::path::Path) -> impl Future<Output = Result<(), String>> {
        let future = self.page.print_pdf(path);
        async move { future.await.map_err(engine_error) }
    }

    fn waiting(&self) -> bool {
        WAITING.with(|w| w.borrow().get(&self.page.page_id()).is_some_and(|n| *n > 0))
    }
}

thread_local! {
    /// Pages with a script of Copper's in flight, and how many.
    static WAITING: std::cell::RefCell<std::collections::HashMap<copper_engine::PageId, u32>> =
        std::cell::RefCell::default();
}

/// Counts one script in flight on a page while it lives.
struct Waiting(copper_engine::PageId);

impl Waiting {
    fn enter(page: copper_engine::PageId) -> Waiting {
        WAITING.with(|w| *w.borrow_mut().entry(page).or_default() += 1);
        Waiting(page)
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        WAITING.with(|w| {
            let mut w = w.borrow_mut();
            if let Some(n) = w.get_mut(&self.0) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    w.remove(&self.0);
                }
            }
        });
    }
}
