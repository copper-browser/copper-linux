//! Addons from the linked Copper Cloud (contract §4.3, §6): what
//! `GET /v1/addons` resolves for the signed-in person, downloaded, verified
//! and staged by [`AddonSync`], and the network half of `copper.fetch`
//! ([`HttpFetcher`]). The rules themselves (verification, the plan, the
//! store, the bridge) are in [`copper_core::addons`]; this module only
//! talks to the network.
//!
//! [`AddonSync`] checks at launch (≈5 s after it starts), when the link or
//! account changes, every 6 hours, when the event stream says `addons`, and
//! on [`AddonSync::check_now`]. Removals and settings-only changes apply at
//! once; a new addon or version is downloaded, verified and staged as
//! *pending* until the person presses Install ([`AddonSync::install_pending`]).
//! What it knows is published as a [`watch`] of [`SyncStatus`]; what changes
//! for a slot goes out as [`Change`]s ([`AddonSync::subscribe`]).
//!
//! ```no_run
//! # async fn demo(cloud: copper_cloud::CloudClient) {
//! use copper_cloud::addons::AddonSync;
//! use copper_core::addons::SlotId;
//! let addons = AddonSync::new(cloud);
//! addons.start(); // inside a tokio runtime
//! let mut status = addons.watch();
//! # let _ = status.changed().await;
//! if !status.borrow().pending.is_empty() {
//!     addons.install_pending(None).unwrap();
//! }
//! let _sidebar = addons.active(SlotId::AgentSidebar);
//! # }
//! ```

use crate::client::{Answer, CloudClient};
use crate::error::{Failure, Result};
use copper_core::addons::bridge::{
    BridgeError, FetchFuture, FetchRequest, FetchResponse, Fetcher, Identity,
};
use copper_core::addons::manifest::is_loopback_host;
use copper_core::addons::plan::{self, Change};
use copper_core::addons::{
    ActiveAddon, AddonState, AddonStore, Bundle, Installed, Platform, ResolvedAddon, ResolvedSet,
    SlotId, verify_bundle,
};
use copper_core::date::IsoDate;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, broadcast, watch};
use url::Url;

/// How often to ask again.
pub const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// The first check after [`AddonSync::start`].
pub const LAUNCH_DELAY: Duration = Duration::from_secs(5);
/// `/v1/addons`.
pub const RESOLVED_PATH: &str = "/v1/addons";
/// The event-stream kind that means "ask again".
pub const EVENT_KIND: &str = "addons";

// MARK: requests

/// What `GET /v1/addons` answered, given the revision last applied (`etag`):
/// `None` when nothing changed, the empty set for a cloud without addons
/// (404). The shared request path can't send `If-None-Match`, so "nothing
/// changed" is the answer's `revision` equal to `etag`.
pub fn interpret_resolved(
    answer: Result<Answer>,
    etag: Option<&str>,
) -> Result<Option<ResolvedSet>> {
    let set = match answer {
        Ok(answer) => answer.json::<ResolvedSet>("GET /v1/addons")?,
        Err(f) if f.status == 404 => ResolvedSet::default(),
        Err(f) if f.status == 304 => return Ok(None),
        Err(f) => return Err(f),
    };
    if etag.is_some_and(|e| e.trim_matches('"') == set.revision) {
        return Ok(None);
    }
    Ok(Some(set))
}

/// `GET /v1/addons` (see [`interpret_resolved`]).
pub async fn fetch_resolved(
    client: &CloudClient,
    etag: Option<&str>,
) -> Result<Option<ResolvedSet>> {
    interpret_resolved(client.request("GET", RESOLVED_PATH, None, &[]).await, etag)
}

/// The path a bundle downloads from: the entry's `bundle` when it is a path
/// on this cloud under `/v1/addons/`, else `/v1/addons/<id>/bundle/<sha256>`.
pub fn bundle_path(addon: &ResolvedAddon) -> String {
    let given = addon.bundle.trim();
    let ok = given.starts_with("/v1/addons/")
        && !given.contains("..")
        && !given.contains("//")
        && !given.contains(['?', '#', '\\']);
    if ok {
        given.to_string()
    } else {
        format!("/v1/addons/{}/bundle/{}", addon.id, addon.sha256)
    }
}

/// A bundle that didn't verify, as a [`Failure`] (`bundle_<code>`).
pub fn bundle_failure(e: &copper_core::addons::BundleError) -> Failure {
    Failure::new(0, format!("bundle_{}", e.code()), e.to_string())
}

/// Download and verify one resolved addon's bundle: the served bytes and
/// what they hold. Bytes that don't verify are dropped.
pub async fn download_bundle(
    client: &CloudClient,
    addon: &ResolvedAddon,
) -> Result<(Vec<u8>, Bundle)> {
    let answer = client
        .request("GET", &bundle_path(addon), None, &[])
        .await?;
    verify(answer.body, addon)
}

fn verify(bytes: Vec<u8>, addon: &ResolvedAddon) -> Result<(Vec<u8>, Bundle)> {
    match verify_bundle(&bytes, &addon.sha256, addon) {
        Ok(bundle) => Ok((bytes, bundle)),
        Err(e) => {
            tracing::warn!(id = %addon.id, version = %addon.version, error = %e, "addon bundle refused");
            Err(bundle_failure(&e))
        }
    }
}

/// `copper.identity()` for the signed-in account.
pub fn identity(client: &CloudClient) -> Option<Identity> {
    let link = client.link()?;
    let account = client.account()?;
    let name = account.display_name.trim();
    Some(Identity {
        email: account.email.clone(),
        name: (!name.is_empty()).then(|| name.to_string()),
        user_id: account.user_id.clone(),
        cloud: link.host(),
    })
}

/// `link base|user id`, while linked and signed in: whose installs these are.
pub fn owner(client: &CloudClient) -> Option<String> {
    let link = client.link()?;
    let account = client.account()?;
    Some(format!(
        "{}|{}",
        link.endpoint.base(),
        account.user_id.to_ascii_lowercase()
    ))
}

// MARK: copper.fetch

/// The most redirects one `copper.fetch` follows.
pub const MAX_REDIRECTS: usize = 10;

/// `copper.fetch` over the network: a plain HTTP client (public trust
/// roots, no cookie store, no ambient credentials), redirects followed by
/// hand so every hop passes the addon's allowlist, `authorization` dropped
/// when a redirect leaves the origin, the request's timeout over the whole
/// exchange, and the body read only up to the limit. Cheap to clone.
///
/// Requests run on the tokio runtime the fetcher was made in (or was
/// given), so the future [`Fetcher::fetch`] returns can be awaited from any
/// executor — a GTK or Win32 main loop included. Dropping it cancels the
/// request.
#[derive(Clone)]
pub struct HttpFetcher {
    /// For loopback targets: never through a proxy.
    direct: reqwest::Client,
    /// Everything else: the system proxy settings apply.
    proxied: reqwest::Client,
    runtime: Option<tokio::runtime::Handle>,
}

impl std::fmt::Debug for HttpFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpFetcher").finish_non_exhaustive()
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpFetcher {
    /// A fetcher on the current tokio runtime, if there is one (else
    /// [`Fetcher::fetch`] must be awaited inside a runtime).
    pub fn new() -> Self {
        Self::with_runtime(tokio::runtime::Handle::try_current().ok())
    }

    /// A fetcher whose requests run on `runtime`.
    pub fn with_runtime(runtime: Option<tokio::runtime::Handle>) -> Self {
        let build = |direct: bool| {
            let mut b = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent(format!("Copper-Addon/{}", copper_core::version::app()));
            if direct {
                b = b.no_proxy();
            }
            b.build().unwrap_or_else(|e| {
                tracing::warn!(error = %e, "addon HTTP client: falling back to defaults");
                reqwest::Client::new()
            })
        };
        HttpFetcher {
            direct: build(true),
            proxied: build(false),
            runtime,
        }
    }

    fn client_for(&self, url: &Url) -> &reqwest::Client {
        if url.host_str().is_some_and(is_loopback_host) {
            &self.direct
        } else {
            &self.proxied
        }
    }

    /// Run a vetted request (see [`Fetcher`]).
    pub async fn run(
        &self,
        request: FetchRequest,
    ) -> std::result::Result<FetchResponse, BridgeError> {
        let timeout = request.timeout;
        match tokio::time::timeout(timeout, self.exchange(request)).await {
            Ok(r) => r,
            Err(_) => Err(BridgeError::timeout(format!(
                "no answer within {} ms",
                timeout.as_millis()
            ))),
        }
    }

    async fn exchange(
        &self,
        request: FetchRequest,
    ) -> std::result::Result<FetchResponse, BridgeError> {
        let FetchRequest {
            mut url,
            method,
            mut headers,
            mut body,
            max_response_bytes,
            allow,
            ..
        } = request;
        let mut method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| BridgeError::bad_request("bad method"))?;
        let mut hops = 0;
        loop {
            let mut req = self.client_for(&url).request(method.clone(), url.clone());
            for (name, value) in &headers {
                req = req.header(name.as_str(), value.as_str());
            }
            if let Some(b) = &body {
                req = req.body(b.clone());
            }
            let resp = req.send().await.map_err(network_error)?;
            let status = resp.status().as_u16();
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .map(str::to_string);
            if let (true, Some(location)) =
                (matches!(status, 301 | 302 | 303 | 307 | 308), location)
            {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(BridgeError::network("too many redirects"));
                }
                let next = url
                    .join(&location)
                    .map_err(|_| BridgeError::network("a redirect to a bad URL"))?;
                if !matches!(next.scheme(), "http" | "https") || !allow.allows(&next) {
                    return Err(BridgeError::denied(format!(
                        "a redirect to {} isn't allowed",
                        next.host_str().unwrap_or(next.scheme())
                    )));
                }
                if next.origin() != url.origin() {
                    headers.retain(|(n, _)| n != "authorization");
                }
                let to_get = status == 303 && method != reqwest::Method::HEAD
                    || matches!(status, 301 | 302) && method == reqwest::Method::POST;
                if to_get {
                    method = reqwest::Method::GET;
                    body = None;
                    headers.retain(|(n, _)| !n.starts_with("content-"));
                }
                url = next;
                continue;
            }
            let mut out_headers: BTreeMap<String, String> = BTreeMap::new();
            for (name, value) in resp.headers() {
                let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                out_headers
                    .entry(name.as_str().to_ascii_lowercase())
                    .and_modify(|v| {
                        v.push_str(", ");
                        v.push_str(&value);
                    })
                    .or_insert(value);
            }
            if resp
                .content_length()
                .is_some_and(|n| n > max_response_bytes as u64)
            {
                return Err(too_large(max_response_bytes));
            }
            let final_url = resp.url().to_string();
            let mut resp = resp;
            let mut data = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(network_error)? {
                if data.len() + chunk.len() > max_response_bytes {
                    return Err(too_large(max_response_bytes));
                }
                data.extend_from_slice(&chunk);
            }
            return Ok(FetchResponse {
                status,
                headers: out_headers,
                body: data,
                url: final_url,
            });
        }
    }
}

fn too_large(limit: usize) -> BridgeError {
    BridgeError::network(format!(
        "the response is over {} MB",
        limit.div_ceil(1024 * 1024)
    ))
}

fn network_error(e: reqwest::Error) -> BridgeError {
    if e.is_timeout() {
        BridgeError::timeout("the request timed out")
    } else {
        // No URL or headers in the message: addons may put secrets there.
        BridgeError::network(match () {
            _ if e.is_connect() => "couldn't connect",
            _ if e.is_body() || e.is_decode() => "the answer broke off",
            _ => "the request failed",
        })
    }
}

/// Aborts the task when dropped (the page went away mid-request).
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Fetcher for HttpFetcher {
    fn fetch(&self, request: FetchRequest) -> FetchFuture {
        let me = self.clone();
        match &self.runtime {
            Some(rt) => {
                let mut task = AbortOnDrop(rt.spawn(async move { me.run(request).await }));
                Box::pin(async move {
                    (&mut task.0)
                        .await
                        .unwrap_or_else(|_| Err(BridgeError::cancelled("the request was stopped")))
                })
            }
            None => Box::pin(async move { me.run(request).await }),
        }
    }
}

// MARK: the service

/// One addon, for Settings › Addons and the update UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub slot: SlotId,
    /// Not turned off here ("Use in Agent sidebar").
    pub enabled: bool,
    /// From [`copper_core::addons::state::DEV_DIR_ENV`], not the cloud.
    pub dev: bool,
}

impl AddonSummary {
    fn of(slot: SlotId, i: &Installed, state: &AddonState, dev: bool) -> Self {
        AddonSummary {
            id: i.id.clone(),
            name: i.display_name().to_string(),
            version: i.version.clone(),
            slot,
            enabled: dev || !state.is_disabled(&i.id),
            dev,
        }
    }

    /// "<Addon name> <version> for the Agent sidebar".
    pub fn update_text(&self) -> String {
        format!(
            "{} {} for the {}",
            self.name,
            self.version,
            self.slot.name()
        )
    }
}

/// What [`AddonSync`] knows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SyncStatus {
    /// Staged updates waiting for Install, one per slot.
    pub pending: Vec<AddonSummary>,
    /// What is installed, one per slot (turned off ones included).
    pub installed: Vec<AddonSummary>,
    /// The developer folder, when in use.
    pub dev: Option<AddonSummary>,
    /// Why the last check didn't finish, for a person.
    pub last_error: Option<String>,
    /// When a check last got an answer.
    pub checked_at: Option<IsoDate>,
    /// A check is running.
    pub checking: bool,
    /// Linked and signed in (else: "Addons come from your Copper Cloud.
    /// Link one in Settings › Cloud.").
    pub linked: bool,
    /// The cloud's `host[:port]`.
    pub cloud: Option<String>,
}

struct Inner {
    client: CloudClient,
    store: AddonStore,
    platform: Platform,
    state: Mutex<AddonState>,
    status: watch::Sender<SyncStatus>,
    changes: broadcast::Sender<Change>,
    /// One check at a time.
    checking: tokio::sync::Mutex<()>,
    kick: Notify,
    last_error: Mutex<Option<String>>,
    running: Mutex<bool>,
}

/// Keeps the addons in `<data>/addons/` in step with the linked cloud (see
/// the module docs). Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct AddonSync {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AddonSync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddonSync")
            .field("status", &*self.inner.status.borrow())
            .finish_non_exhaustive()
    }
}

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// Not linked or not signed in.
    NoCloud,
    /// The cloud's revision is the one applied.
    Unchanged,
    /// Applied; `staged` new versions are pending.
    Applied { staged: usize },
    /// Some of it applied; this went wrong (kept for the next check).
    Failed(String),
}

impl AddonSync {
    /// Read `addons/state.json` under the client's data folder. At launch:
    /// what ran before the last install is let go, unreferenced files are
    /// deleted, and installs that belong to another link or account are
    /// removed. Nothing is fetched until [`AddonSync::start`] or
    /// [`AddonSync::check_now`].
    pub fn new(client: CloudClient) -> Self {
        let store = AddonStore::new(client.store().paths());
        Self::with_store(client, store, Platform::current())
    }

    /// [`AddonSync::new`] with an explicit store and platform (tests).
    pub fn with_store(client: CloudClient, store: AddonStore, platform: Platform) -> Self {
        let mut state = store.load();
        let had_previous = !state.previous.is_empty();
        state.forget_previous();
        let (status, _) = watch::channel(SyncStatus::default());
        let (changes, _) = broadcast::channel(64);
        let me = AddonSync {
            inner: Arc::new(Inner {
                client,
                store,
                platform,
                state: Mutex::new(state),
                status,
                changes,
                checking: tokio::sync::Mutex::new(()),
                kick: Notify::new(),
                last_error: Mutex::new(None),
                running: Mutex::new(false),
            }),
        };
        me.follow_owner();
        if had_previous {
            me.save();
        }
        me.with_state(|s| me.inner.store.gc(s));
        me.publish();
        me
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut AddonState) -> R) -> R {
        f(&mut self.inner.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn save(&self) {
        let result = self.with_state(|s| self.inner.store.save(s));
        if let Err(e) = result {
            tracing::warn!(error = %e, "addons/state.json not written");
        }
    }

    fn emit(&self, changes: Vec<Change>) {
        for c in changes {
            let _ = self.inner.changes.send(c);
        }
    }

    fn set_error(&self, error: Option<String>) {
        *self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = error;
    }

    fn publish(&self) {
        let linked = owner(&self.inner.client).is_some();
        let cloud = self.inner.client.link().map(|l| l.host());
        let checking = *self.inner.running.lock().unwrap_or_else(|e| e.into_inner());
        let last_error = self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let status = self.with_state(|s| {
            let mut out = SyncStatus {
                linked,
                cloud,
                checking,
                last_error,
                checked_at: s.last_check,
                ..Default::default()
            };
            for &slot in SlotId::ALL {
                if let Some(i) = s.installed.get(slot.as_str()) {
                    out.installed.push(AddonSummary::of(slot, i, s, false));
                }
                if let Some(p) = s.pending.get(slot.as_str()) {
                    out.pending.push(AddonSummary::of(slot, p, s, false));
                }
            }
            out.dev = s.dev.as_ref().and_then(|d| {
                let slot = d.slot.parse::<SlotId>().ok()?;
                Some(AddonSummary::of(slot, d, s, true))
            });
            out
        });
        self.inner.status.send_if_modified(|old| {
            let changed = *old != status;
            *old = status;
            changed
        });
    }

    /// What it knows now.
    pub fn status(&self) -> SyncStatus {
        self.inner.status.borrow().clone()
    }

    /// Every change to [`AddonSync::status`].
    pub fn watch(&self) -> watch::Receiver<SyncStatus> {
        self.inner.status.subscribe()
    }

    /// What changes for a slot: reload it, fire `settings`, show "Update
    /// available".
    pub fn subscribe(&self) -> broadcast::Receiver<Change> {
        self.inner.changes.subscribe()
    }

    /// The store.
    pub fn store(&self) -> &AddonStore {
        &self.inner.store
    }

    /// A copy of `state.json` as it is now.
    pub fn state(&self) -> AddonState {
        self.with_state(|s| s.clone())
    }

    /// What `slot` shows now, ready to serve and run (`None` = its default).
    pub fn active(&self, slot: SlotId) -> Option<ActiveAddon> {
        self.with_state(|s| self.inner.store.active(s, slot))
    }

    /// Ask now and every [`INTERVAL`] (first after [`LAUNCH_DELAY`]); follow
    /// link and account changes and the `addons` event. Call once, inside a
    /// tokio runtime.
    pub fn start(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut status = me.inner.client.watch_status();
            let mut events = me.inner.client.subscribe();
            tokio::time::sleep(LAUNCH_DELAY).await;
            me.check().await;
            let mut tick = tokio::time::interval(INTERVAL);
            tick.tick().await;
            let mut offline = false;
            loop {
                tokio::select! {
                    changed = status.changed() => {
                        if changed.is_err() { return; }
                        let reachable = status.borrow_and_update().reachable;
                        if me.follow_owner() || (offline && reachable) {
                            me.check().await;
                        }
                        me.publish();
                    }
                    event = events.recv() => match event {
                        Ok(e) if e.kind == EVENT_KIND => { me.check().await; }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    _ = tick.tick() => { me.check().await; }
                    _ = me.inner.kick.notified() => { me.check().await; }
                }
                offline = self_offline(&me);
            }
        });
    }

    /// Ask for a check from anywhere (the UI thread included) without
    /// waiting; [`AddonSync::start`]'s loop runs it.
    pub fn kick(&self) {
        self.inner.kick.notify_one();
    }

    /// "Check for addon updates": ask now and wait for the outcome.
    pub async fn check_now(&self) -> CheckOutcome {
        self.check().await
    }

    /// The link or account changed: installs of another owner go (and so
    /// does their addons' `copper.storage` when a different person signs
    /// in); with no owner, the slots show their defaults. Returns whether
    /// it changed.
    fn follow_owner(&self) -> bool {
        let now = owner(&self.inner.client);
        let (changed, changes) = self.with_state(|s| {
            let had_installs = !s.installed.is_empty() || !s.pending.is_empty();
            match (&now, &s.owner) {
                (Some(a), Some(b)) if a == b => return (false, vec![]),
                (None, _) if !had_installs => return (false, vec![]),
                _ => {}
            }
            let mut changes = vec![];
            for &slot in SlotId::ALL {
                if let Some(old) = s.uninstall(slot) {
                    changes.push(Change::Removed { slot, id: old.id });
                }
                if s.pending.remove(slot.as_str()).is_some() {
                    changes.push(Change::PendingDropped { slot });
                }
            }
            s.revision = None;
            s.removed.clear();
            if let Some(new) = &now {
                if s.owner.as_ref().is_some_and(|o| o != new) {
                    self.inner.store.wipe_data();
                }
                s.owner = Some(new.clone());
            }
            (true, changes)
        });
        if changed {
            self.save();
            self.emit(changes);
        }
        changed
    }

    async fn check(&self) -> CheckOutcome {
        let client = self.inner.client.clone();
        self.check_with(|addon| {
            let client = client.clone();
            async move { download_bundle(&client, &addon).await }
        })
        .await
    }

    /// One check, with `download` fetching bundles (tests pass their own).
    pub async fn check_with<D, F>(&self, download: D) -> CheckOutcome
    where
        D: Fn(ResolvedAddon) -> F,
        F: Future<Output = Result<(Vec<u8>, Bundle)>>,
    {
        let _one = self.inner.checking.lock().await;
        self.follow_owner();
        if owner(&self.inner.client).is_none() {
            self.publish();
            return CheckOutcome::NoCloud;
        }
        *self.inner.running.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.publish();
        let etag = self.with_state(|s| s.revision.clone());
        let answer = fetch_resolved(&self.inner.client, etag.as_deref()).await;
        let outcome = match answer {
            Ok(None) => {
                self.with_state(|s| s.last_check = Some(IsoDate::now()));
                self.save();
                self.set_error(None);
                CheckOutcome::Unchanged
            }
            Ok(Some(set)) => self.apply_with(set, download).await,
            Err(f) => {
                tracing::info!(status = f.status, code = %f.code, "addons check failed");
                self.set_error(Some(if f.is_network() {
                    "Copper Cloud isn't reachable".to_string()
                } else {
                    f.message.clone()
                }));
                CheckOutcome::Failed(f.message)
            }
        };
        *self.inner.running.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.publish();
        outcome
    }

    /// Apply one `/v1/addons` answer: removals and settings now, then
    /// download → verify → stage each new version as pending. The revision
    /// is remembered only when everything staged.
    pub async fn apply_with<D, F>(&self, set: ResolvedSet, download: D) -> CheckOutcome
    where
        D: Fn(ResolvedAddon) -> F,
        F: Future<Output = Result<(Vec<u8>, Bundle)>>,
    {
        let owner_then = owner(&self.inner.client);
        let platform = self.inner.platform;
        let (p, changes) = self.with_state(|s| {
            let p = plan::plan(s, &set, platform);
            let changes = p.apply_immediate(s, &set.revision);
            s.last_check = Some(IsoDate::now());
            (p, changes)
        });
        for i in &p.ignored {
            tracing::info!(id = %i.id, slot = %i.slot, reason = ?i.reason, "addon not used here");
        }
        self.save();
        self.emit(changes);
        self.publish();
        let mut error = None;
        let mut staged = 0;
        for addon in p.stage {
            let Some(slot) = addon.slot_id() else {
                continue;
            };
            let already = self.with_state(|s| {
                s.pending
                    .get(slot.as_str())
                    .is_some_and(|q| q.sha256 == addon.sha256)
            });
            if already {
                continue;
            }
            let result = match self.reuse_staged(&addon) {
                Some(ok) => Ok(ok),
                None => download(addon.clone()).await,
            };
            if owner(&self.inner.client) != owner_then {
                return CheckOutcome::Failed("the cloud account changed".into());
            }
            let entry = result.and_then(|(bytes, bundle)| {
                self.inner
                    .store
                    .stage(&bytes, &bundle, &addon)
                    .map_err(|e| bundle_failure(&e))
            });
            match entry {
                Ok(entry) => {
                    self.with_state(|s| s.set_pending(slot, entry));
                    staged += 1;
                    self.emit(vec![Change::Pending { slot }]);
                }
                Err(f) => {
                    tracing::warn!(id = %addon.id, error = %f, "addon update not staged");
                    error = Some(format!("{} {}: {}", addon.name, addon.version, f.message));
                }
            }
        }
        let gc = self.with_state(|s| {
            if error.is_none() {
                s.revision = Some(set.revision.clone());
            }
            s.clone()
        });
        self.save();
        self.inner.store.gc(&gc);
        self.set_error(error.clone());
        self.publish();
        match error {
            Some(e) => CheckOutcome::Failed(e),
            None => CheckOutcome::Applied { staged },
        }
    }

    /// A staged file from an earlier, unfinished check.
    fn reuse_staged(&self, addon: &ResolvedAddon) -> Option<(Vec<u8>, Bundle)> {
        let bytes = std::fs::read(self.inner.store.staging_path(&addon.sha256)).ok()?;
        verify(bytes, addon).ok()
    }

    /// Install the pending updates for `slots` (all when `None`): each
    /// slot's view reloads with the new addon ([`Change::Installed`]). No
    /// tokio needed.
    pub fn install_pending(
        &self,
        slots: Option<&[SlotId]>,
    ) -> std::result::Result<Vec<Change>, copper_core::StoreError> {
        let out = self.with_state(|s| plan::install_pending(&self.inner.store, s, slots))?;
        if let Some((_, e)) = out.failed.first() {
            self.set_error(Some(format!("The update didn't install: {e}")));
        }
        self.emit(out.changes.clone());
        self.publish();
        Ok(out.changes)
    }

    /// "Use in Agent sidebar" on or off for addon `id` here.
    pub fn set_enabled(&self, id: &str, enabled: bool) {
        let changes = self.with_state(|s| {
            if !s.set_disabled(id, !enabled) {
                return vec![];
            }
            SlotId::ALL
                .iter()
                .filter(|slot| s.installed.get(slot.as_str()).is_some_and(|i| i.id == id))
                .map(|&slot| Change::Toggled {
                    slot,
                    id: id.to_string(),
                    enabled,
                })
                .collect()
        });
        self.save();
        self.emit(changes);
        self.publish();
    }

    /// "Remove": addon `id` goes from this Copper until the cloud's answer
    /// changes.
    pub fn remove(&self, id: &str) {
        let changes = self.with_state(|s| {
            let pending: Vec<SlotId> = SlotId::ALL
                .iter()
                .copied()
                .filter(|slot| s.pending.get(slot.as_str()).is_some_and(|p| p.id == id))
                .collect();
            let installed: Vec<SlotId> = SlotId::ALL
                .iter()
                .copied()
                .filter(|slot| s.installed.get(slot.as_str()).is_some_and(|i| i.id == id))
                .collect();
            s.remove_locally(id);
            installed
                .into_iter()
                .map(|slot| Change::Removed {
                    slot,
                    id: id.to_string(),
                })
                .chain(
                    pending
                        .into_iter()
                        .map(|slot| Change::PendingDropped { slot }),
                )
                .collect::<Vec<_>>()
        });
        self.save();
        self.emit(changes);
        self.publish();
    }
}

fn self_offline(me: &AddonSync) -> bool {
    me.inner
        .last_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
        == Some("Copper Cloud isn't reachable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::addons::bridge::{BridgeHost, Context, ErrorCode, PageInfo, handle_message};
    use copper_core::addons::manifest::Manifest;
    use copper_core::addons::testkit;
    use copper_core::store::{Paths, Store};
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn client(dir: &std::path::Path) -> CloudClient {
        CloudClient::open(Store::new(Paths::new(dir.join("data"), dir.join("config")))).unwrap()
    }

    #[test]
    fn resolved_answers() {
        let body = testkit::resolved_body("r_1", &[]);
        let ok = |b: &[u8]| {
            Ok(Answer {
                status: 200,
                body: b.to_vec(),
            })
        };
        assert_eq!(
            interpret_resolved(ok(&body), None)
                .unwrap()
                .unwrap()
                .revision,
            "r_1"
        );
        assert!(
            interpret_resolved(ok(&body), Some("r_1"))
                .unwrap()
                .is_none()
        );
        assert!(
            interpret_resolved(ok(&body), Some("\"r_1\""))
                .unwrap()
                .is_none()
        );
        assert!(
            interpret_resolved(ok(&body), Some("r_0"))
                .unwrap()
                .is_some()
        );
        let empty = interpret_resolved(Err(Failure::new(404, "not_found", "")), Some("r_1"))
            .unwrap()
            .unwrap();
        assert!(empty.addons.is_empty() && empty.revision.is_empty());
        assert!(
            interpret_resolved(Err(Failure::new(304, "x", "")), Some("r_1"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            interpret_resolved(Err(Failure::new(503, "x", "")), None)
                .unwrap_err()
                .status,
            503
        );
        assert_eq!(
            interpret_resolved(ok(b"[1]"), None).unwrap_err().code,
            "decode"
        );
    }

    #[test]
    fn bundle_paths() {
        let mut a = ResolvedAddon {
            id: "x.y".into(),
            sha256: "ab".into(),
            bundle: "/v1/addons/x.y/bundle/ab".into(),
            ..Default::default()
        };
        assert_eq!(bundle_path(&a), "/v1/addons/x.y/bundle/ab");
        for bad in [
            "https://evil.example.net/b",
            "/v1/addons/../admin",
            "/v1/other",
            "",
        ] {
            a.bundle = bad.into();
            assert_eq!(bundle_path(&a), "/v1/addons/x.y/bundle/ab", "{bad}");
        }
    }

    fn served(version: &str, html: &[u8]) -> (Vec<u8>, ResolvedAddon) {
        let m = testkit::manifest_json("example.sidebar", version);
        let bytes = testkit::served_bundle(&m, &[("index.html", html)]);
        let r = testkit::resolved_for(&m, &bytes);
        (bytes, r)
    }

    #[tokio::test]
    async fn sync_applies_stages_installs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AddonStore::at(tmp.path().join("addons"));
        let sync = AddonSync::with_store(client(tmp.path()), store.clone(), Platform::Linux);
        let mut changes = sync.subscribe();
        assert!(!sync.status().linked);
        assert_eq!(sync.check_now().await, CheckOutcome::NoCloud);

        let (bytes, r) = served("1.0.0", b"<p>1</p>");
        let set = ResolvedSet {
            revision: "r_1".into(),
            addons: vec![r.clone()],
            ..Default::default()
        };
        let served_bytes = bytes.clone();
        let fetches = Arc::new(Mutex::new(0));
        let counter = fetches.clone();
        let download = move |addon: ResolvedAddon| {
            let bytes = served_bytes.clone();
            let counter = counter.clone();
            async move {
                *counter.lock().unwrap() += 1;
                verify(bytes, &addon)
            }
        };
        assert_eq!(
            sync.apply_with(set.clone(), &download).await,
            CheckOutcome::Applied { staged: 1 }
        );
        assert_eq!(
            changes.try_recv().unwrap(),
            Change::Pending {
                slot: SlotId::AgentSidebar
            }
        );
        let status = sync.status();
        assert_eq!(status.pending.len(), 1);
        assert_eq!(
            status.pending[0].update_text(),
            "Example sidebar 1.0.0 for the Agent sidebar"
        );
        assert!(
            sync.active(SlotId::AgentSidebar).is_none(),
            "not until Install"
        );
        assert_eq!(sync.state().revision.as_deref(), Some("r_1"));

        let installed = sync.install_pending(None).unwrap();
        assert!(matches!(installed[0], Change::Installed { .. }));
        let _ = changes.try_recv();
        let active = sync.active(SlotId::AgentSidebar).unwrap();
        assert_eq!(
            std::fs::read(active.dir.join("index.html")).unwrap(),
            b"<p>1</p>"
        );
        assert!(sync.status().pending.is_empty());

        // Settings-only: immediate, no download.
        let mut r2 = r.clone();
        r2.settings = json!({"apiUrl": "https://api.example.com/v2"});
        let set2 = ResolvedSet {
            revision: "r_2".into(),
            addons: vec![r2],
            ..Default::default()
        };
        assert_eq!(
            sync.apply_with(set2, &download).await,
            CheckOutcome::Applied { staged: 0 }
        );
        assert!(matches!(
            changes.try_recv().unwrap(),
            Change::Settings { .. }
        ));
        assert_eq!(*fetches.lock().unwrap(), 1);
        assert_eq!(
            sync.active(SlotId::AgentSidebar)
                .unwrap()
                .installed
                .settings["apiUrl"],
            "https://api.example.com/v2"
        );

        // Turned off here: the default shows.
        sync.set_enabled("example.sidebar", false);
        assert!(matches!(
            changes.try_recv().unwrap(),
            Change::Toggled { enabled: false, .. }
        ));
        assert!(sync.active(SlotId::AgentSidebar).is_none());
        assert!(!sync.status().installed[0].enabled);
        sync.set_enabled("example.sidebar", true);
        let _ = changes.try_recv();

        // A download that fails verification: nothing staged, revision kept.
        let (_, r3) = served("1.1.0", b"<p>3</p>");
        let set3 = ResolvedSet {
            revision: "r_3".into(),
            addons: vec![r3],
            ..Default::default()
        };
        let bad = |addon: ResolvedAddon| async move { verify(b"{}".to_vec(), &addon) };
        assert!(matches!(
            sync.apply_with(set3, bad).await,
            CheckOutcome::Failed(_)
        ));
        assert_eq!(sync.state().revision.as_deref(), Some("r_2"));
        assert!(sync.status().last_error.is_some());
        assert!(
            sync.active(SlotId::AgentSidebar).is_some(),
            "the current one keeps running"
        );

        // Removed on the cloud: immediate.
        let empty = ResolvedSet {
            revision: "r_4".into(),
            ..Default::default()
        };
        sync.apply_with(empty, &download).await;
        assert!(matches!(
            changes.try_recv().unwrap(),
            Change::Removed { .. }
        ));
        assert!(sync.active(SlotId::AgentSidebar).is_none());

        // A fresh start keeps what was saved.
        let again = AddonSync::with_store(client(tmp.path()), store.clone(), Platform::Linux);
        assert_eq!(again.state().revision.as_deref(), Some("r_4"));
        assert!(again.state().previous.is_empty());
    }

    #[tokio::test]
    async fn local_remove_until_the_cloud_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AddonStore::at(tmp.path().join("addons"));
        let sync = AddonSync::with_store(client(tmp.path()), store, Platform::Linux);
        let (bytes, r) = served("1.0.0", b"<p>1</p>");
        let download = move |addon: ResolvedAddon| {
            let bytes = bytes.clone();
            async move { verify(bytes, &addon) }
        };
        let set = ResolvedSet {
            revision: "r_1".into(),
            addons: vec![r],
            ..Default::default()
        };
        sync.apply_with(set.clone(), &download).await;
        sync.install_pending(None).unwrap();
        sync.remove("example.sidebar");
        assert!(sync.active(SlotId::AgentSidebar).is_none());
        assert_eq!(
            sync.apply_with(set.clone(), &download).await,
            CheckOutcome::Applied { staged: 0 }
        );
        let mut changed = set;
        changed.revision = "r_2".into();
        assert_eq!(
            sync.apply_with(changed, &download).await,
            CheckOutcome::Applied { staged: 1 }
        );
    }

    // MARK: copper.fetch against a local server

    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        headers: Vec<(String, String)>,
    }

    /// A tiny HTTP/1.1 server on 127.0.0.1 with scripted routes.
    async fn server() -> (u16, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let mut lines = text.split("\r\n");
                    let path = lines
                        .next()
                        .and_then(|l| l.split(' ').nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let headers: Vec<(String, String)> = lines
                        .take_while(|l| !l.is_empty())
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    log.lock().unwrap().push(Seen {
                        path: path.clone(),
                        headers,
                    });
                    let (status, extra, body): (&str, String, Vec<u8>) = match path.as_str() {
                        "/ok" => (
                            "200 OK",
                            "Content-Type: text/plain\r\nX-Multi: a\r\nX-Multi: b\r\nSet-Cookie: s=1\r\n".into(),
                            b"hello".to_vec(),
                        ),
                        "/to-ok" => ("302 Found", "Location: /ok\r\nSet-Cookie: t=1\r\n".into(), vec![]),
                        "/to-localhost" => (
                            "307 Temporary Redirect",
                            format!("Location: http://localhost:{port}/ok\r\n"),
                            vec![],
                        ),
                        "/to-evil" => (
                            "302 Found",
                            "Location: https://evil.example.net/steal\r\n".into(),
                            vec![],
                        ),
                        "/loop" => ("302 Found", "Location: /loop\r\n".into(), vec![]),
                        "/big" => ("200 OK", String::new(), vec![b'x'; 4096]),
                        "/slow" => {
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            ("200 OK", String::new(), vec![])
                        }
                        _ => ("404 Not Found", String::new(), b"nope".to_vec()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (port, seen)
    }

    struct Host;

    impl BridgeHost for Host {
        async fn identity(&self) -> Option<Identity> {
            None
        }
        async fn open_url(&self, _url: Url) -> std::result::Result<(), BridgeError> {
            Ok(())
        }
        async fn page_current(&self) -> Option<PageInfo> {
            None
        }
        async fn close_slot(&self) {}
        async fn oauth_authorize(
            &self,
            _url: Url,
            _redirect_prefix: String,
            _timeout: Duration,
        ) -> std::result::Result<String, BridgeError> {
            Err(BridgeError::cancelled("closed"))
        }
    }

    fn context(dir: &std::path::Path, hosts: Value) -> Context {
        let mut m = testkit::manifest_json("example.sidebar", "1.0.0");
        m["permissions"]["hosts"] = hosts;
        let manifest = Manifest::from_value(m).unwrap();
        Context {
            slot: SlotId::AgentSidebar,
            addon: Installed {
                id: "example.sidebar".into(),
                version: "1.0.0".into(),
                ..Default::default()
            },
            settings: json!({}),
            manifest,
            platform: Platform::Linux,
            app_version: "0.0.0".into(),
            theme_dark: false,
            store: AddonStore::at(dir.join("addons")),
            fetcher: Some(Arc::new(HttpFetcher::new())),
        }
    }

    async fn fetch(ctx: &Context, params: Value) -> std::result::Result<Value, BridgeError> {
        let text = json!({"v": 1, "id": 1, "method": "fetch", "params": params}).to_string();
        handle_message(ctx, &Host, &text).await.unwrap().result
    }

    #[tokio::test]
    async fn fetch_through_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let (port, seen) = server().await;
        let ctx = context(tmp.path(), json!(["127.0.0.1", "localhost"]));
        let base = format!("http://127.0.0.1:{port}");

        let r = fetch(
            &ctx,
            json!({"url": format!("{base}/ok"), "headers": {"X-Token": "t", "Cookie": "c=1"}}),
        )
        .await
        .unwrap();
        assert_eq!(r["status"], 200);
        assert_eq!(r["body"], "hello");
        assert_eq!(r["headers"]["content-type"], "text/plain");
        assert_eq!(r["headers"]["x-multi"], "a, b");
        let first = seen.lock().unwrap().last().cloned().unwrap();
        assert!(
            first
                .headers
                .iter()
                .any(|(k, v)| k == "x-token" && v == "t")
        );
        assert!(
            !first.headers.iter().any(|(k, _)| k == "cookie"),
            "no cookies"
        );

        // A redirect on the same origin is followed; cookies set on the way
        // are not sent back.
        let r = fetch(&ctx, json!({"url": format!("{base}/to-ok")}))
            .await
            .unwrap();
        assert_eq!(r["status"], 200);
        assert_eq!(r["url"], format!("{base}/ok"));
        let after = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(after.path, "/ok");
        assert!(!after.headers.iter().any(|(k, _)| k == "cookie"));

        // To another allowed origin: followed, Authorization dropped.
        let r = fetch(
            &ctx,
            json!({"url": format!("{base}/to-localhost"), "headers": {"Authorization": "Bearer s"}}),
        )
        .await
        .unwrap();
        assert_eq!(r["status"], 200);
        let log = seen.lock().unwrap().clone();
        let hop = &log[log.len() - 2];
        assert_eq!(hop.path, "/to-localhost");
        assert!(hop.headers.iter().any(|(k, _)| k == "authorization"));
        let landed = log.last().unwrap();
        assert!(
            landed
                .headers
                .iter()
                .any(|(k, v)| k == "host" && v.starts_with("localhost"))
        );
        assert!(!landed.headers.iter().any(|(k, _)| k == "authorization"));

        // To a host that isn't allowed: denied, never contacted.
        let e = fetch(&ctx, json!({"url": format!("{base}/to-evil")}))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied);

        let e = fetch(&ctx, json!({"url": format!("{base}/loop")}))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Network);

        let r = fetch(&ctx, json!({"url": format!("{base}/missing")}))
            .await
            .unwrap();
        assert_eq!(
            (r["status"].clone(), r["body"].clone()),
            (json!(404), json!("nope"))
        );

        let e = fetch(
            &ctx,
            json!({"url": format!("{base}/slow"), "timeoutMs": 200}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout);

        // Not listed: denied before anything is sent.
        let only = context(tmp.path(), json!(["api.example.com"]));
        let before = seen.lock().unwrap().len();
        let e = fetch(&only, json!({"url": format!("{base}/ok")}))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied);
        assert_eq!(seen.lock().unwrap().len(), before);

        // A port in the rule pins the port.
        let pinned = context(
            tmp.path(),
            json!([format!("127.0.0.1:{}", port.wrapping_add(1))]),
        );
        let e = fetch(&pinned, json!({"url": format!("{base}/ok")}))
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Denied);
    }

    #[tokio::test]
    async fn fetch_response_limit() {
        let (port, _) = server().await;
        let m = testkit::manifest_json("example.sidebar", "1.0.0");
        let mut m = Manifest::from_value(m).unwrap();
        m.permissions.hosts = vec!["127.0.0.1".into()];
        let mut req = copper_core::addons::bridge::fetch_request(
            &m,
            &json!({"url": format!("http://127.0.0.1:{port}/big")}),
        )
        .unwrap();
        req.max_response_bytes = 1000;
        let e = HttpFetcher::new().fetch(req.clone()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::Network);
        req.max_response_bytes = 4096;
        assert_eq!(
            HttpFetcher::new().fetch(req).await.unwrap().body.len(),
            4096
        );
    }

    #[test]
    fn fetch_from_another_executor() {
        // The UI thread awaits a fetch made on a tokio runtime elsewhere.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (port, _) = rt.block_on(server());
        let fetcher = HttpFetcher::with_runtime(Some(rt.handle().clone()));
        let m = testkit::manifest_json("example.sidebar", "1.0.0");
        let mut m = Manifest::from_value(m).unwrap();
        m.permissions.hosts = vec!["127.0.0.1".into()];
        let req = copper_core::addons::bridge::fetch_request(
            &m,
            &json!({"url": format!("http://127.0.0.1:{port}/ok")}),
        )
        .unwrap();
        // Polled with a plain thread-parking loop: no tokio context here.
        let out = block_on_plain(fetcher.fetch(req));
        assert_eq!(out.unwrap().status, 200);
    }

    fn block_on_plain<F: Future>(f: F) -> F::Output {
        use std::task::{Context as Cx, Poll, Wake, Waker};
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut cx = Cx::from_waker(&waker);
        let mut f = std::pin::pin!(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park_timeout(Duration::from_millis(50));
        }
    }
}
