//! The native half of bridge v1 (`window.copper`, contract §5), the same on
//! every platform.
//!
//! The page posts one JSON string per call (`{v:1, id, method, params}`);
//! the platform hands it to [`handle_message`] (or to
//! [`BridgeRequest::parse`] and then [`dispatch`]), runs the returned
//! future, and evaluates [`Reply::to_js`] in the slot's page. Events go the
//! same way through [`emit_js`].
//!
//! [`dispatch`] does the platform-neutral methods itself — `info`,
//! `settings`, `storage.*` (1 MB per addon, keys ≤ 128 characters, a 0600
//! file), `fetch` (manifest host allowlist, https only but for loopback,
//! limits; executed by a [`Fetcher`] such as `copper_cloud::addons::HttpFetcher`)
//! — checks capabilities, and asks the platform's [`BridgeHost`] for the
//! rest (identity, open a tab, the active page, close the slot, the OAuth
//! window, the native agent).
//!
//! ## Threading
//!
//! `dispatch` is an `async fn` generic over the host. Its future is `Send`
//! when the host is `Sync` and the host's futures are `Send`; otherwise it
//! is a local future. A GTK or Win32 shell typically runs it on the UI
//! thread (`glib::spawn_future_local`, `copper_win_rt::spawn_local`) with a
//! host that touches widgets directly: nothing in it blocks for long
//! (storage is a small file; `fetch` runs on the fetcher's tokio runtime and
//! is only awaited here).

use super::manifest::{Capability, HostAllow, Manifest, is_loopback_host};
use super::slot::SlotId;
use super::state::{ActiveAddon, AddonStore, Installed};
use super::{BRIDGE_VERSION, Platform};
use crate::store::write_atomic;
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use url::Url;

/// The largest message accepted from a page.
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
/// `copper.storage`: all keys and values, as compact JSON.
pub const STORAGE_LIMIT_BYTES: usize = 1024 * 1024;
/// `copper.storage` key length, in characters.
pub const MAX_KEY_CHARS: usize = 128;
/// `copper.fetch` request body, in bytes.
pub const FETCH_MAX_BODY: usize = 2 * 1024 * 1024;
/// `copper.fetch` response body, in bytes.
pub const FETCH_MAX_RESPONSE: usize = 8 * 1024 * 1024;
/// `copper.fetch` timeout when none is given.
pub const FETCH_DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// `copper.fetch` longest timeout.
pub const FETCH_MAX_TIMEOUT: Duration = Duration::from_secs(120);
/// `copper.oauth.authorize` timeout when none is given.
pub const OAUTH_DEFAULT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// `copper.oauth.authorize` longest timeout.
pub const OAUTH_MAX_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The bridge's error codes (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// Capability or host not in the manifest.
    Denied,
    BadRequest,
    NotSignedIn,
    Cancelled,
    Timeout,
    Network,
    /// Not on this platform.
    Unavailable,
    Internal,
}

impl ErrorCode {
    /// The wire code.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Denied => "denied",
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::NotSignedIn => "not_signed_in",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Timeout => "timeout",
            ErrorCode::Network => "network",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Internal => "internal",
        }
    }
}

/// A refused or failed call: `{code, message}` in the page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct BridgeError {
    pub code: ErrorCode,
    pub message: String,
}

impl BridgeError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        BridgeError {
            code,
            message: message.into(),
        }
    }
    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Denied, message)
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Network, message)
    }
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Cancelled, message)
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    /// `{code, message}`.
    pub fn to_json(&self) -> Value {
        json!({"code": self.code.as_str(), "message": self.message})
    }
}

/// One call from the page.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeRequest {
    pub v: u32,
    pub id: u64,
    pub method: String,
    /// Always an object (`{}` when the page sent none).
    pub params: Value,
}

/// A message that isn't a call. With an `id`, answer it with
/// `Reply::err(id, error)`; without one, drop it (and log).
#[derive(Debug, Clone, PartialEq)]
pub struct BadMessage {
    pub id: Option<u64>,
    pub error: BridgeError,
}

impl BridgeRequest {
    /// Read one posted string. Refuses: more than [`MAX_MESSAGE_BYTES`], not
    /// a JSON object, no numeric `id`, `v` other than 1, no `method`,
    /// non-object `params`.
    pub fn parse(text: &str) -> Result<BridgeRequest, BadMessage> {
        let bad = |id, message: &str| BadMessage {
            id,
            error: BridgeError::bad_request(message),
        };
        if text.len() > MAX_MESSAGE_BYTES {
            return Err(bad(None, "message too large"));
        }
        let Ok(Value::Object(mut obj)) = serde_json::from_str::<Value>(text) else {
            return Err(bad(None, "not a JSON object"));
        };
        let Some(id) = obj.get("id").and_then(Value::as_u64) else {
            return Err(bad(None, "no id"));
        };
        if obj.get("v").and_then(Value::as_u64) != Some(u64::from(BRIDGE_VERSION)) {
            return Err(bad(Some(id), "unsupported bridge version"));
        }
        let method = match obj.get("method").and_then(Value::as_str) {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => return Err(bad(Some(id), "no method")),
        };
        let params = match obj.remove("params") {
            None | Some(Value::Null) => Value::Object(Map::new()),
            Some(p @ Value::Object(_)) => p,
            Some(_) => return Err(bad(Some(id), "params must be an object")),
        };
        Ok(BridgeRequest {
            v: BRIDGE_VERSION,
            id,
            method,
            params,
        })
    }
}

/// The answer to one call.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub id: u64,
    pub result: Result<Value, BridgeError>,
}

impl Reply {
    pub fn ok(id: u64, value: Value) -> Self {
        Reply {
            id,
            result: Ok(value),
        }
    }

    pub fn err(id: u64, error: BridgeError) -> Self {
        Reply {
            id,
            result: Err(error),
        }
    }

    /// `window.__copperAddon.reply(<id>,<true|false>,<json>)`, safe to
    /// evaluate as a script (see [`script_json`]).
    pub fn to_js(&self) -> String {
        let (ok, payload) = match &self.result {
            Ok(v) => (true, script_json(v)),
            Err(e) => (false, script_json(&e.to_json())),
        };
        format!("window.__copperAddon.reply({},{ok},{payload})", self.id)
    }
}

/// `window.__copperAddon.emit(<name>,<payload>)` for an event
/// (`visibility`, `theme`, `settings`, `agent`, `page`).
pub fn emit_js(name: &str, payload: &Value) -> String {
    format!(
        "window.__copperAddon.emit({},{})",
        script_json(&Value::String(name.to_string())),
        script_json(payload)
    )
}

/// JSON that is also safe inside a script: `<`, `>`, `&`, U+2028 and U+2029
/// are written as `\u` escapes (they can only occur inside strings, where
/// the escapes mean the same thing), so `</script>` and `<!--` never appear
/// and the text never breaks a line in older engines.
pub fn script_json(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// `copper.identity()`: who is signed in to the linked cloud.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub email: String,
    pub name: Option<String>,
    pub user_id: String,
    /// The cloud's `host[:port]`.
    pub cloud: String,
}

/// `copper.page.current()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageInfo {
    pub url: String,
    pub title: String,
}

/// `copper.agent.*` calls other than `state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCall {
    SetModel(String),
    Send(String),
    Stop,
    Clear,
}

/// `agent.state()` where there is no native agent.
pub fn agent_unavailable_state() -> Value {
    json!({"available": false, "busy": false, "models": [], "model": null, "items": []})
}

/// What the platform does for the bridge. Futures may be local (the UI
/// thread) or `Send`; see the module docs. Every method gets only calls the
/// manifest allows; arguments are already checked.
pub trait BridgeHost {
    /// Capabilities this platform implements (`copper.info().capabilities`
    /// is these ∩ the manifest's). Default: everything but `agent`.
    fn available(&self) -> Vec<Capability> {
        vec![
            Capability::Identity,
            Capability::Storage,
            Capability::Fetch,
            Capability::Oauth,
            Capability::OpenUrl,
            Capability::Page,
        ]
    }

    /// The signed-in cloud account, or `None`.
    fn identity(&self) -> impl Future<Output = Option<Identity>>;

    /// Open an http(s) URL in a new foreground tab.
    fn open_url(&self, url: Url) -> impl Future<Output = Result<(), BridgeError>>;

    /// The active tab's URL and title.
    fn page_current(&self) -> impl Future<Output = Option<PageInfo>>;

    /// Hide the slot (as Ctrl+E / ⌘E).
    fn close_slot(&self) -> impl Future<Output = ()>;

    /// Open a small window with the person's normal browsing profile at
    /// `url`, watch every navigation and redirect, and resolve with the
    /// first URL starting with `redirect_prefix` (cancelling that load and
    /// closing the window). Window closed → `cancelled`; `timeout` elapsed
    /// → `timeout`.
    fn oauth_authorize(
        &self,
        url: Url,
        redirect_prefix: String,
        timeout: Duration,
    ) -> impl Future<Output = Result<String, BridgeError>>;

    /// `copper.agent.state()`. Default: not available.
    fn agent_state(&self) -> impl Future<Output = Value> {
        async { agent_unavailable_state() }
    }

    /// `copper.agent.setModel/send/stop/clear`. Default: `unavailable`.
    fn agent_call(&self, call: AgentCall) -> impl Future<Output = Result<Value, BridgeError>> {
        let _ = call;
        async { Err(BridgeError::unavailable("no native agent on this platform")) }
    }
}

/// A `copper.fetch` that passed every check, for a [`Fetcher`] to run.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchRequest {
    pub url: Url,
    /// Uppercase.
    pub method: String,
    /// Checked names and values; forbidden ones (cookies, hop-by-hop,
    /// `host`, `content-length`, `proxy-*`) already dropped.
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// For the whole exchange, redirects and body included.
    pub timeout: Duration,
    /// Stop reading (`network`) past this many body bytes.
    pub max_response_bytes: usize,
    /// Every redirect target must pass this (else `denied`). Drop
    /// `authorization` when a redirect changes origin.
    pub allow: HostAllow,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FetchResponse {
    pub status: u16,
    /// Lowercased names; repeated headers joined with `", "`.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
    /// After redirects.
    pub url: String,
}

/// A boxed `Send` future of a fetch.
pub type FetchFuture = Pin<Box<dyn Future<Output = Result<FetchResponse, BridgeError>> + Send>>;

/// Runs vetted `copper.fetch` requests: no cookies, no ambient credentials,
/// redirects followed only to allowed hosts, the limits in the request.
pub trait Fetcher: Send + Sync {
    fn fetch(&self, request: FetchRequest) -> FetchFuture;
}

/// Everything a call needs to know about the addon and the app. Rebuild or
/// update it when settings or the theme change.
#[derive(Clone)]
pub struct Context {
    pub slot: SlotId,
    pub addon: Installed,
    pub manifest: Manifest,
    /// Resolved settings (`copper.settings()`).
    pub settings: Value,
    pub platform: Platform,
    /// Copper's version.
    pub app_version: String,
    pub theme_dark: bool,
    /// Where `data/<id>/storage.json` lives.
    pub store: AddonStore,
    /// `None` = `copper.fetch` is `unavailable`.
    pub fetcher: Option<Arc<dyn Fetcher>>,
}

impl std::fmt::Debug for Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("slot", &self.slot)
            .field("addon", &self.addon.id)
            .field("platform", &self.platform)
            .field("fetcher", &self.fetcher.is_some())
            .finish_non_exhaustive()
    }
}

impl Context {
    /// A context for what a slot shows. `theme_dark` starts `false`;
    /// `fetcher` starts `None`.
    pub fn new(
        active: &ActiveAddon,
        platform: Platform,
        app_version: impl Into<String>,
        store: AddonStore,
    ) -> Self {
        Context {
            slot: active.slot,
            addon: active.installed.clone(),
            manifest: active.manifest.clone(),
            settings: active.installed.settings.clone(),
            platform,
            app_version: app_version.into(),
            theme_dark: false,
            store,
            fetcher: None,
        }
    }

    /// The manifest asks for `cap`.
    pub fn grants(&self, cap: Capability) -> bool {
        self.manifest.permissions.grants(cap)
    }

    fn available<H: BridgeHost>(&self, host: &H) -> Vec<Capability> {
        let mut out: Vec<Capability> = host
            .available()
            .into_iter()
            .filter(|c| *c != Capability::Fetch || self.fetcher.is_some())
            .collect();
        if !out.contains(&Capability::Storage) {
            out.push(Capability::Storage);
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Parse and dispatch one posted string. `None` when it can't be answered
/// (no id); log those.
pub async fn handle_message<H: BridgeHost>(ctx: &Context, host: &H, text: &str) -> Option<Reply> {
    match BridgeRequest::parse(text) {
        Ok(req) => Some(dispatch(ctx, host, req).await),
        Err(bad) => {
            tracing::debug!(error = %bad.error, "addon bridge message refused");
            bad.id.map(|id| Reply::err(id, bad.error))
        }
    }
}

/// Answer one call (see the module docs). Unknown methods are
/// `bad_request`, as on the Mac (`AddonBridge.call`): a newer shim on an
/// older Copper, or `voice.*` where there is no dictation.
pub async fn dispatch<H: BridgeHost>(ctx: &Context, host: &H, req: BridgeRequest) -> Reply {
    let result = run(ctx, host, &req).await;
    Reply { id: req.id, result }
}

fn need<H: BridgeHost>(ctx: &Context, host: &H, cap: Capability) -> Result<(), BridgeError> {
    if !ctx.grants(cap) {
        return Err(BridgeError::denied(format!(
            "the addon's manifest doesn't ask for {cap:?} ({})",
            cap.as_str()
        )));
    }
    if !ctx.available(host).contains(&cap) {
        return Err(BridgeError::unavailable(format!(
            "{} isn't available on this platform",
            cap.as_str()
        )));
    }
    Ok(())
}

async fn run<H: BridgeHost>(
    ctx: &Context,
    host: &H,
    req: &BridgeRequest,
) -> Result<Value, BridgeError> {
    let p = &req.params;
    match req.method.as_str() {
        "info" => {
            let available = ctx.available(host);
            let caps: Vec<&str> = ctx
                .manifest
                .permissions
                .capabilities()
                .into_iter()
                .filter(|c| available.contains(c))
                .map(Capability::as_str)
                .collect();
            Ok(json!({
                "bridge": BRIDGE_VERSION,
                "platform": ctx.platform.as_str(),
                "appVersion": ctx.app_version,
                "slot": ctx.slot.as_str(),
                "addon": {"id": ctx.addon.id, "version": ctx.addon.version},
                "capabilities": caps,
                "theme": {"dark": ctx.theme_dark},
            }))
        }
        "settings" => Ok(ctx.settings.clone()),
        "identity" => {
            need(ctx, host, Capability::Identity)?;
            Ok(match host.identity().await {
                Some(who) => serde_json::to_value(who).unwrap_or(Value::Null),
                None => Value::Null,
            })
        }
        m @ ("storage.get" | "storage.set" | "storage.remove" | "storage.keys") => {
            need(ctx, host, Capability::Storage)?;
            storage(&ctx.store.storage_path(&ctx.addon.id), m, p)
        }
        "fetch" => {
            need(ctx, host, Capability::Fetch)?;
            fetch(ctx, p).await
        }
        "oauth.authorize" => {
            need(ctx, host, Capability::Oauth)?;
            oauth(ctx, host, p).await
        }
        "openUrl" => {
            need(ctx, host, Capability::OpenUrl)?;
            let url = parse_url(p, "url")?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(BridgeError::bad_request("only http and https URLs open"));
            }
            host.open_url(url).await?;
            Ok(Value::Bool(true))
        }
        "page.current" => {
            need(ctx, host, Capability::Page)?;
            Ok(match host.page_current().await {
                Some(page) => serde_json::to_value(page).unwrap_or(Value::Null),
                None => Value::Null,
            })
        }
        "ui.close" => {
            host.close_slot().await;
            Ok(Value::Bool(true))
        }
        "agent.state" => {
            if !ctx.grants(Capability::Agent) {
                need(ctx, host, Capability::Agent)?;
            }
            if ctx.available(host).contains(&Capability::Agent) {
                Ok(host.agent_state().await)
            } else {
                Ok(agent_unavailable_state())
            }
        }
        m @ ("agent.setModel" | "agent.send" | "agent.stop" | "agent.clear") => {
            need(ctx, host, Capability::Agent)?;
            let call = match m {
                "agent.setModel" => AgentCall::SetModel(text_param(p, "id")?),
                "agent.send" => AgentCall::Send(text_param(p, "text")?),
                "agent.stop" => AgentCall::Stop,
                _ => AgentCall::Clear,
            };
            host.agent_call(call).await
        }
        other => Err(BridgeError::bad_request(format!(
            "No bridge method \"{other}\""
        ))),
    }
}

fn text_param(p: &Value, key: &str) -> Result<String, BridgeError> {
    p.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| BridgeError::bad_request(format!("{key} must be a string")))
}

fn parse_url(p: &Value, key: &str) -> Result<Url, BridgeError> {
    let text = text_param(p, key)?;
    Url::parse(&text).map_err(|_| BridgeError::bad_request(format!("{key} is not a URL")))
}

/// `timeoutMs`: absent, zero or negative = `default`; capped at `max`.
fn timeout_param(p: &Value, default: Duration, max: Duration) -> Result<Duration, BridgeError> {
    match p.get("timeoutMs") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => {
            let ms = v
                .as_f64()
                .ok_or_else(|| BridgeError::bad_request("timeoutMs must be a number"))?;
            if !ms.is_finite() || ms <= 0.0 {
                Ok(default)
            } else {
                Ok(Duration::from_millis(ms.min(max.as_millis() as f64) as u64))
            }
        }
    }
}

// MARK: storage

static STORAGE_LOCK: Mutex<()> = Mutex::new(());

fn storage(path: &Path, method: &str, p: &Value) -> Result<Value, BridgeError> {
    let _guard = STORAGE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut map = read_storage(path);
    if method == "storage.keys" {
        return Ok(Value::Array(
            map.keys().map(|k| Value::String(k.clone())).collect(),
        ));
    }
    let key = text_param(p, "key")?;
    let chars = key.chars().count();
    if chars == 0 || chars > MAX_KEY_CHARS {
        return Err(BridgeError::bad_request(format!(
            "a storage key is 1–{MAX_KEY_CHARS} characters"
        )));
    }
    match method {
        "storage.get" => Ok(map.get(&key).cloned().unwrap_or(Value::Null)),
        "storage.remove" => {
            if map.remove(&key).is_some() {
                write_storage(path, &map)?;
            }
            Ok(Value::Bool(true))
        }
        _ => {
            let value = p.get("value").cloned().unwrap_or(Value::Null);
            map.insert(key, value);
            let bytes =
                serde_json::to_vec(&map).map_err(|e| BridgeError::internal(e.to_string()))?;
            if bytes.len() > STORAGE_LIMIT_BYTES {
                return Err(BridgeError::bad_request("storage is full (1 MB per addon)"));
            }
            write_storage(path, &map)?;
            Ok(Value::Bool(true))
        }
    }
}

fn read_storage(path: &Path) -> BTreeMap<String, Value> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            tracing::warn!(file = %path.display(), error = %e, "addon storage unreadable; starting empty");
            BTreeMap::new()
        }),
        Err(_) => BTreeMap::new(),
    }
}

fn write_storage(path: &Path, map: &BTreeMap<String, Value>) -> Result<(), BridgeError> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir).map_err(|e| BridgeError::internal(e.to_string()))?;
    }
    let bytes = serde_json::to_vec(map).map_err(|e| BridgeError::internal(e.to_string()))?;
    write_atomic(path, &bytes, true, false).map_err(|e| BridgeError::internal(e.to_string()))
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

// MARK: fetch

const METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// Request headers dropped silently (as `fetch()` does): cookies,
/// hop-by-hop, and the ones the HTTP client owns.
fn forbidden_header(name: &str) -> bool {
    matches!(
        name,
        "cookie"
            | "cookie2"
            | "host"
            | "content-length"
            | "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "upgrade"
            | "te"
            | "trailer"
            | "expect"
    ) || name.starts_with("proxy-")
        || name.starts_with("sec-")
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|c| c == b'\t' || (c >= 0x20 && c != 0x7f))
}

/// Check a `copper.fetch` call against the manifest and the limits.
pub fn fetch_request(manifest: &Manifest, p: &Value) -> Result<FetchRequest, BridgeError> {
    let url = parse_url(p, "url")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(BridgeError::bad_request("copper.fetch takes https URLs"));
    }
    let allow = manifest.permissions.host_allow();
    if !allow.allows(&url) {
        let host = url.host_str().unwrap_or("");
        return Err(BridgeError::denied(
            if url.scheme() == "http" && !is_loopback_host(host) {
                format!("http://{host} isn't allowed; use https")
            } else {
                format!("{host} isn't in the addon's permissions.hosts")
            },
        ));
    }
    let method = match p.get("method") {
        None | Some(Value::Null) => "GET".to_string(),
        Some(Value::String(m)) => m.to_ascii_uppercase(),
        Some(_) => return Err(BridgeError::bad_request("method must be a string")),
    };
    if !METHODS.contains(&method.as_str()) {
        return Err(BridgeError::bad_request(format!(
            "method {method} isn't supported"
        )));
    }
    let mut headers = Vec::new();
    match p.get("headers") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                let Some(value) = value.as_str() else {
                    return Err(BridgeError::bad_request(format!(
                        "header {name} must be a string"
                    )));
                };
                let lower = name.to_ascii_lowercase();
                if !valid_header_name(&lower) || !valid_header_value(value) {
                    return Err(BridgeError::bad_request(format!("bad header {name}")));
                }
                if !forbidden_header(&lower) {
                    headers.push((lower, value.to_string()));
                }
            }
        }
        Some(_) => return Err(BridgeError::bad_request("headers must be an object")),
    }
    let body = match p.get("body") {
        None | Some(Value::Null) => None,
        // An empty body on a GET is no body.
        Some(Value::String(s)) if s.is_empty() && matches!(method.as_str(), "GET" | "HEAD") => None,
        Some(Value::String(s)) => {
            if s.len() > FETCH_MAX_BODY {
                return Err(BridgeError::bad_request("the body is over 2 MB"));
            }
            if matches!(method.as_str(), "GET" | "HEAD") {
                return Err(BridgeError::bad_request(format!(
                    "a {method} request has no body"
                )));
            }
            Some(s.clone())
        }
        Some(_) => return Err(BridgeError::bad_request("body must be a string")),
    };
    Ok(FetchRequest {
        url,
        method,
        headers,
        body,
        timeout: timeout_param(p, FETCH_DEFAULT_TIMEOUT, FETCH_MAX_TIMEOUT)?,
        max_response_bytes: FETCH_MAX_RESPONSE,
        allow,
    })
}

async fn fetch(ctx: &Context, p: &Value) -> Result<Value, BridgeError> {
    let fetcher = ctx
        .fetcher
        .clone()
        .ok_or_else(|| BridgeError::unavailable("copper.fetch isn't available"))?;
    let request = fetch_request(&ctx.manifest, p)?;
    let response = fetcher.fetch(request).await?;
    Ok(json!({
        "status": response.status,
        "headers": response.headers,
        "body": String::from_utf8_lossy(&response.body),
        "url": response.url,
    }))
}

// MARK: oauth

async fn oauth<H: BridgeHost>(ctx: &Context, host: &H, p: &Value) -> Result<Value, BridgeError> {
    let allow = ctx.manifest.permissions.host_allow();
    let url = parse_url(p, "url")?;
    if !allow.allows(&url) {
        return Err(BridgeError::denied(format!(
            "{} isn't in the addon's permissions.hosts",
            url.host_str().unwrap_or("")
        )));
    }
    let prefix = text_param(p, "redirectPrefix")?;
    let parsed =
        Url::parse(&prefix).map_err(|_| BridgeError::bad_request("redirectPrefix is not a URL"))?;
    let loopback = parsed.scheme() == "http"
        && parsed.host_str().is_some_and(is_loopback_host)
        && parsed.port().is_some();
    if !loopback {
        if parsed.scheme() != "https" {
            return Err(BridgeError::bad_request(
                "redirectPrefix must be http://127.0.0.1:<port>/…, http://localhost:<port>/… or https",
            ));
        }
        if !allow.allows(&parsed) {
            return Err(BridgeError::denied(
                "redirectPrefix's host isn't in the addon's permissions.hosts",
            ));
        }
    }
    let timeout = timeout_param(p, OAUTH_DEFAULT_TIMEOUT, OAUTH_MAX_TIMEOUT)?;
    let got = host.oauth_authorize(url, prefix.clone(), timeout).await?;
    if !got.starts_with(&prefix) {
        return Err(BridgeError::internal("the redirect didn't match"));
    }
    Ok(json!({"url": got}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::testkit;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context as TaskContext, Poll, Waker};

    pub(crate) fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = TaskContext::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    #[derive(Default)]
    struct Host {
        opened: Mutex<Vec<String>>,
        closed: AtomicUsize,
        oauth: Mutex<Vec<(String, String, Duration)>>,
    }

    impl BridgeHost for Host {
        async fn identity(&self) -> Option<Identity> {
            Some(Identity {
                email: "ann@example.com".into(),
                name: Some("Ann".into()),
                user_id: "u1".into(),
                cloud: "cloud.example.com:8443".into(),
            })
        }
        async fn open_url(&self, url: Url) -> Result<(), BridgeError> {
            self.opened.lock().unwrap().push(url.to_string());
            Ok(())
        }
        async fn page_current(&self) -> Option<PageInfo> {
            Some(PageInfo {
                url: "https://example.com/".into(),
                title: "Example".into(),
            })
        }
        async fn close_slot(&self) {
            self.closed.fetch_add(1, Ordering::Relaxed);
        }
        async fn oauth_authorize(
            &self,
            url: Url,
            redirect_prefix: String,
            timeout: Duration,
        ) -> Result<String, BridgeError> {
            self.oauth
                .lock()
                .unwrap()
                .push((url.to_string(), redirect_prefix.clone(), timeout));
            Ok(format!("{redirect_prefix}?code=abc"))
        }
    }

    struct Recorder(Mutex<Vec<FetchRequest>>);

    impl Fetcher for Recorder {
        fn fetch(&self, request: FetchRequest) -> FetchFuture {
            self.0.lock().unwrap().push(request.clone());
            Box::pin(async move {
                Ok(FetchResponse {
                    status: 200,
                    headers: [("content-type".to_string(), "text/plain".to_string())].into(),
                    body: b"ok".to_vec(),
                    url: request.url.to_string(),
                })
            })
        }
    }

    fn ctx(dir: &Path, caps: &[&str]) -> Context {
        let mut manifest = testkit::manifest_json("example.sidebar", "1.0.0");
        manifest["permissions"]["capabilities"] = json!(caps);
        manifest["permissions"]["hosts"] = json!(["api.example.com", "*.example.org", "127.0.0.1"]);
        let manifest = Manifest::from_value(manifest).unwrap();
        Context {
            slot: SlotId::AgentSidebar,
            addon: Installed {
                id: "example.sidebar".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
            settings: json!({"apiUrl": "https://api.example.com/v1"}),
            manifest,
            platform: Platform::Linux,
            app_version: "0.3.0".into(),
            theme_dark: true,
            store: AddonStore::at(dir.join("addons")),
            fetcher: Some(Arc::new(Recorder(Mutex::new(Vec::new())))),
        }
    }

    const ALL: &[&str] = &[
        "identity", "storage", "fetch", "oauth", "open-url", "page", "agent",
    ];

    fn call(ctx: &Context, host: &Host, method: &str, params: Value) -> Result<Value, BridgeError> {
        let text = json!({"v": 1, "id": 7, "method": method, "params": params}).to_string();
        let reply = block_on(handle_message(ctx, host, &text)).unwrap();
        assert_eq!(reply.id, 7);
        reply.result
    }

    #[test]
    fn parse_refusals() {
        assert!(BridgeRequest::parse("nope").unwrap_err().id.is_none());
        assert!(
            BridgeRequest::parse(r#"{"v":1,"method":"info"}"#)
                .unwrap_err()
                .id
                .is_none()
        );
        let e = BridgeRequest::parse(r#"{"v":2,"id":3,"method":"info"}"#).unwrap_err();
        assert_eq!((e.id, e.error.code), (Some(3), ErrorCode::BadRequest));
        let e = BridgeRequest::parse(r#"{"v":1,"id":3}"#).unwrap_err();
        assert_eq!(e.id, Some(3));
        let e = BridgeRequest::parse(r#"{"v":1,"id":3,"method":"info","params":[1]}"#).unwrap_err();
        assert_eq!(e.id, Some(3));
        let ok = BridgeRequest::parse(r#"{"v":1,"id":3,"method":"info"}"#).unwrap();
        assert_eq!(ok.params, json!({}));
        let big = format!(
            r#"{{"v":1,"id":1,"method":"x","params":{{"a":"{}"}}}}"#,
            "a".repeat(MAX_MESSAGE_BYTES)
        );
        assert!(BridgeRequest::parse(&big).is_err());
    }

    #[test]
    fn info_and_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let c = ctx(tmp.path(), ALL);
        let host = Host::default();
        let info = call(&c, &host, "info", json!({})).unwrap();
        assert_eq!(info["bridge"], 1);
        assert_eq!(info["platform"], "linux");
        assert_eq!(info["appVersion"], "0.3.0");
        assert_eq!(info["slot"], "agent-sidebar");
        assert_eq!(
            info["addon"],
            json!({"id": "example.sidebar", "version": "1.0.0"})
        );
        assert_eq!(info["theme"], json!({"dark": true}));
        assert_eq!(
            info["capabilities"],
            json!(["identity", "storage", "fetch", "oauth", "open-url", "page"]),
            "agent isn't available here"
        );
        assert_eq!(
            call(&c, &host, "settings", json!({})).unwrap(),
            json!({"apiUrl": "https://api.example.com/v1"})
        );
        let mut no_fetcher = c.clone();
        no_fetcher.fetcher = None;
        let info = call(&no_fetcher, &host, "info", json!({})).unwrap();
        assert!(
            !info["capabilities"]
                .as_array()
                .unwrap()
                .contains(&json!("fetch"))
        );
        assert_eq!(
            call(
                &no_fetcher,
                &host,
                "fetch",
                json!({"url": "https://api.example.com/"})
            )
            .unwrap_err()
            .code,
            ErrorCode::Unavailable
        );
    }

    #[test]
    fn host_methods_and_capabilities() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Host::default();
        let c = ctx(tmp.path(), ALL);
        assert_eq!(
            call(&c, &host, "identity", json!({})).unwrap()["userId"],
            "u1"
        );
        assert_eq!(
            call(&c, &host, "page.current", json!({})).unwrap()["title"],
            "Example"
        );
        assert_eq!(
            call(
                &c,
                &host,
                "openUrl",
                json!({"url": "https://example.com/a"})
            )
            .unwrap(),
            true
        );
        assert_eq!(
            call(&c, &host, "openUrl", json!({"url": "file:///etc/passwd"}))
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
        assert_eq!(host.opened.lock().unwrap().len(), 1);
        assert_eq!(call(&c, &host, "ui.close", json!({})).unwrap(), true);
        assert_eq!(host.closed.load(Ordering::Relaxed), 1);
        assert_eq!(
            call(&c, &host, "agent.state", json!({})).unwrap()["available"],
            false
        );
        assert_eq!(
            call(&c, &host, "agent.send", json!({"text": "hi"}))
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        // Unknown, and voice.* without dictation: bad_request, the Mac's words.
        for m in ["nope.method", "voice.start", "voice.state"] {
            let e = call(&c, &host, m, json!({})).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadRequest);
            assert_eq!(e.message, format!("No bridge method \"{m}\""));
        }
        // Without the capability in the manifest: denied.
        let bare = ctx(tmp.path(), &[]);
        for (m, p) in [
            ("identity", json!({})),
            ("page.current", json!({})),
            ("openUrl", json!({"url": "https://example.com/"})),
            ("storage.get", json!({"key": "a"})),
            ("fetch", json!({"url": "https://api.example.com/"})),
            ("agent.state", json!({})),
            (
                "oauth.authorize",
                json!({"url": "https://api.example.com/auth", "redirectPrefix": "http://127.0.0.1:53682/cb"}),
            ),
        ] {
            assert_eq!(
                call(&bare, &host, m, p).unwrap_err().code,
                ErrorCode::Denied,
                "{m}"
            );
        }
        // info, settings and ui.close need nothing.
        assert!(call(&bare, &host, "info", json!({})).is_ok());
        assert!(call(&bare, &host, "ui.close", json!({})).is_ok());
    }

    #[test]
    fn storage_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Host::default();
        let c = ctx(tmp.path(), ALL);
        assert_eq!(
            call(&c, &host, "storage.get", json!({"key": "a"})).unwrap(),
            Value::Null
        );
        call(
            &c,
            &host,
            "storage.set",
            json!({"key": "a", "value": {"n": [1, 2]}}),
        )
        .unwrap();
        call(&c, &host, "storage.set", json!({"key": "b", "value": "x"})).unwrap();
        assert_eq!(
            call(&c, &host, "storage.get", json!({"key": "a"})).unwrap(),
            json!({"n": [1, 2]})
        );
        assert_eq!(
            call(&c, &host, "storage.keys", json!({})).unwrap(),
            json!(["a", "b"])
        );
        call(&c, &host, "storage.remove", json!({"key": "a"})).unwrap();
        assert_eq!(
            call(&c, &host, "storage.keys", json!({})).unwrap(),
            json!(["b"])
        );
        let path = c.store.storage_path("example.sidebar");
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Limits.
        let long = "k".repeat(MAX_KEY_CHARS + 1);
        assert_eq!(
            call(&c, &host, "storage.set", json!({"key": long, "value": 1}))
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            call(&c, &host, "storage.get", json!({"key": ""}))
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
        assert_eq!(
            call(&c, &host, "storage.get", json!({})).unwrap_err().code,
            ErrorCode::BadRequest
        );
        let half = "v".repeat(STORAGE_LIMIT_BYTES / 2);
        call(
            &c,
            &host,
            "storage.set",
            json!({"key": "h1", "value": half}),
        )
        .unwrap();
        assert_eq!(
            call(
                &c,
                &host,
                "storage.set",
                json!({"key": "h2", "value": half})
            )
            .unwrap_err()
            .code,
            ErrorCode::BadRequest,
            "over 1 MB in total"
        );
        assert_eq!(
            call(&c, &host, "storage.get", json!({"key": "h2"})).unwrap(),
            Value::Null,
            "nothing written"
        );
        // Another addon has its own file.
        let mut other = c.clone();
        other.addon.id = "example.other".into();
        assert_eq!(
            call(&other, &host, "storage.keys", json!({})).unwrap(),
            json!([])
        );
    }

    #[test]
    fn fetch_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Host::default();
        let c = ctx(tmp.path(), ALL);
        let r = call(
            &c,
            &host,
            "fetch",
            json!({
                "url": "https://api.example.com/v1/x",
                "method": "post",
                "headers": {"Authorization": "Bearer t", "Cookie": "a=b", "Content-Type": "application/json"},
                "body": "{}",
                "timeoutMs": 999_999
            }),
        )
        .unwrap();
        assert_eq!(r["status"], 200);
        assert_eq!(r["body"], "ok");
        assert_eq!(r["headers"]["content-type"], "text/plain");
        let req = fetch_request(
            &c.manifest,
            &json!({"url": "https://a.example.org/", "headers": {"X-A": "1", "Cookie": "x", "Proxy-Authorization": "y"}}),
        )
        .unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.headers, vec![("x-a".to_string(), "1".to_string())]);
        assert_eq!(req.timeout, FETCH_DEFAULT_TIMEOUT);
        let req = fetch_request(
            &c.manifest,
            &json!({"url": "https://api.example.com/", "method": "PUT", "body": "x", "timeoutMs": 999_999}),
        )
        .unwrap();
        assert_eq!(req.timeout, FETCH_MAX_TIMEOUT);
        assert_eq!(req.body.as_deref(), Some("x"));
        let code = |p: Value| fetch_request(&c.manifest, &p).unwrap_err().code;
        assert_eq!(
            code(json!({"url": "https://evil.example.net/"})),
            ErrorCode::Denied
        );
        assert_eq!(
            code(json!({"url": "https://example.org/"})),
            ErrorCode::Denied
        );
        assert_eq!(
            code(json!({"url": "http://api.example.com/"})),
            ErrorCode::Denied
        );
        assert_eq!(
            code(json!({"url": "ftp://api.example.com/"})),
            ErrorCode::BadRequest
        );
        assert_eq!(code(json!({"url": "not a url"})), ErrorCode::BadRequest);
        assert_eq!(
            code(json!({"url": "https://api.example.com/", "method": "TRACE"})),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code(json!({"url": "https://api.example.com/", "body": "x"})),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code(
                json!({"url": "https://api.example.com/", "method": "POST", "body": "b".repeat(FETCH_MAX_BODY + 1)})
            ),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code(json!({"url": "https://api.example.com/", "headers": {"X": "a\r\nb"}})),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code(json!({"url": "https://api.example.com/", "headers": {"X": 1}})),
            ErrorCode::BadRequest
        );
        // http to loopback when listed.
        assert!(fetch_request(&c.manifest, &json!({"url": "http://127.0.0.1:4555/x"})).is_ok());
        assert_eq!(
            code(json!({"url": "http://localhost:4555/x"})),
            ErrorCode::Denied
        );
    }

    #[test]
    fn oauth_checks() {
        let tmp = tempfile::tempdir().unwrap();
        let host = Host::default();
        let c = ctx(tmp.path(), ALL);
        let r = call(
            &c,
            &host,
            "oauth.authorize",
            json!({"url": "https://api.example.com/authorize?x=1", "redirectPrefix": "http://127.0.0.1:53682/oauth/callback"}),
        )
        .unwrap();
        assert_eq!(r["url"], "http://127.0.0.1:53682/oauth/callback?code=abc");
        assert_eq!(host.oauth.lock().unwrap()[0].2, OAUTH_DEFAULT_TIMEOUT);
        let code = |p: Value| call(&c, &host, "oauth.authorize", p).unwrap_err().code;
        assert_eq!(
            code(
                json!({"url": "https://evil.example.net/a", "redirectPrefix": "http://127.0.0.1:1/cb"})
            ),
            ErrorCode::Denied
        );
        assert_eq!(
            code(
                json!({"url": "https://api.example.com/a", "redirectPrefix": "http://127.0.0.1/cb"})
            ),
            ErrorCode::BadRequest,
            "loopback needs a port"
        );
        assert_eq!(
            code(
                json!({"url": "https://api.example.com/a", "redirectPrefix": "copper-addon://x/cb"})
            ),
            ErrorCode::BadRequest
        );
        assert_eq!(
            code(
                json!({"url": "https://api.example.com/a", "redirectPrefix": "https://evil.example.net/cb"})
            ),
            ErrorCode::Denied
        );
        assert!(
            call(&c, &host, "oauth.authorize", json!({"url": "https://api.example.com/a", "redirectPrefix": "https://a.example.org/cb", "timeoutMs": 1000})).is_ok()
        );
    }

    #[test]
    fn reply_js_is_script_safe() {
        let r = Reply::ok(5, json!({"t": "</script><!-- \u{2028}\u{2029} & >"}));
        let js = r.to_js();
        assert_eq!(
            js,
            r#"window.__copperAddon.reply(5,true,{"t":"\u003c/script\u003e\u003c!-- \u2028\u2029 \u0026 \u003e"})"#
        );
        assert!(!js.contains("</script"));
        assert!(!js.contains('\u{2028}'));
        // Still the same JSON once unescaped.
        let payload = &js["window.__copperAddon.reply(5,true,".len()..js.len() - 1];
        assert_eq!(
            serde_json::from_str::<Value>(payload).unwrap(),
            json!({"t": "</script><!-- \u{2028}\u{2029} & >"})
        );
        let e = Reply::err(6, BridgeError::denied("no")).to_js();
        assert_eq!(
            e,
            r#"window.__copperAddon.reply(6,false,{"code":"denied","message":"no"})"#
        );
        assert_eq!(
            emit_js("visibility", &json!({"visible": true})),
            r#"window.__copperAddon.emit("visibility",{"visible":true})"#
        );
    }

    #[test]
    fn dispatch_is_send_for_a_sync_host() {
        fn assert_send<T: Send>(_: &T) {}
        let tmp = tempfile::tempdir().unwrap();
        let c = ctx(tmp.path(), ALL);
        let host = Host::default();
        let req = BridgeRequest::parse(r#"{"v":1,"id":1,"method":"info"}"#).unwrap();
        let fut = dispatch(&c, &host, req);
        assert_send(&fut);
        assert!(block_on(fut).result.is_ok());
    }
}
