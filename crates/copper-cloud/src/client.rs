//! The one authenticated, certificate-pinned client everything Cloud-shaped
//! talks through (Mac: `Cloud.swift`, `CloudAuth.swift`, the minting side of
//! `CloudPairing.swift`).
//!
//! Nothing here runs on its own. A link is made by pasting a link code, an
//! account by signing in — or both at once with a pairing code another
//! signed-in Copper made; until then no request leaves the machine. What is
//! kept lives in `cloud.json` (0600, atomic, via `copper_core::Store`): the
//! instance address, its key and certificate fingerprint, the account, the
//! session token and the sync switches. The token is never logged and never
//! handed to a page.
//!
//! Every method that does network work is `async` and must run inside a tokio
//! runtime; [`CloudClient::start`] (call once, inside the runtime) starts the
//! event stream when already signed in.

use crate::error::{Failure, Result};
use crate::link::{Code, Endpoint, Link, PairingCode, normalize_fingerprint, parse_code};
use crate::sync::history::Limits;
use crate::tls::Trust;
use copper_core::cloud::{CloudAccount, CloudFile, SyncPrefs};
use copper_core::store::Store;
use futures_util::StreamExt;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// One request's limit (Mac: 15 s).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The server's floor for a password.
pub const MINIMUM_PASSWORD: usize = 10;

/// Who is signed in, and as which device. Ids are kept the way the Mac keeps
/// them in `cloud.json` (uppercase UUIDs) and sent lowercase.
pub type Account = CloudAccount;

/// The device id as the server is told it (lowercase).
pub fn wire_id(id: &str) -> String {
    id.to_ascii_lowercase()
}

/// What the settings page and the MCP show about the cloud.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CloudStatus {
    pub linked: bool,
    /// `host:port`.
    pub host: Option<String>,
    /// The pin in pairs, `ab:cd:…`.
    pub fingerprint: Option<String>,
    pub signed_in: bool,
    pub account: Option<Account>,
    /// The last request or health check got an answer.
    pub reachable: bool,
    /// `GET /v1/info` → `version`, once known.
    pub server_version: Option<String>,
    pub device_id: String,
    pub device_name: String,
}

/// One event from `GET /v1/sync/events` (`doc`, `history`,
/// `history_deleted`, `canvas`, `resync`, `ready`), plus `open` each time the
/// stream (re)connects — anything may have been missed.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudEvent {
    pub kind: String,
    pub data: Value,
}

/// What `GET /v1/info` says.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
    pub signup: Option<bool>,
    pub access_mode: Option<String>,
    pub features: BTreeSet<String>,
    pub raw: Value,
}

/// Things a copper-cloud has from some version on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// `/v1/canvases/:id/links`, `/v1/canvas-links/:token`, `/join/:token`.
    ShareLinks,
    /// `GET /v1/people`.
    People,
    /// Invite reminders, `invitee`, withdrawing an invite.
    InviteReminders,
    /// Canvas chat mentions.
    Mentions,
}

impl Feature {
    /// The first copper-cloud release with it.
    pub fn since(self) -> &'static str {
        match self {
            Feature::ShareLinks | Feature::People => "0.3.0",
            Feature::InviteReminders => "0.5.0",
            Feature::Mentions => "0.6.0",
        }
    }
}

/// `"0.10.1"` against `"0.3.0"`, number by number; a pre-release or build
/// suffix is ignored, a missing part is 0.
pub fn version_at_least(text: &str, minimum: &str) -> bool {
    fn numbers(value: &str) -> Vec<u64> {
        let core = value
            .trim()
            .trim_start_matches(['v', 'V'])
            .split(['-', '+'])
            .next()
            .unwrap_or("");
        core.split('.')
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    }
    let (have, want) = (numbers(text), numbers(minimum));
    for i in 0..have.len().max(want.len()) {
        let a = have.get(i).copied().unwrap_or(0);
        let b = want.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    true
}

/// A pairing code this Copper minted (`POST /v1/auth/pairing`).
#[derive(Clone, PartialEq, Eq)]
pub struct MintedPairing {
    pub id: String,
    pub code: String,
    /// The whole code to show — always carries this link's pin.
    pub link: String,
    pub created_at: Option<String>,
    pub expires_at: String,
    pub device_name: Option<String>,
}

impl std::fmt::Debug for MintedPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedPairing")
            .field("id", &self.id)
            .field("expires_at", &self.expires_at)
            .field("device_name", &self.device_name)
            .finish_non_exhaustive()
    }
}

/// One of my unused, unexpired pairing codes (never the code itself).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PairingInfo {
    pub id: String,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    pub expires_at: String,
    #[serde(default)]
    pub used_at: Option<String>,
}

/// A device on the account (`GET /v1/devices`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub current: bool,
}

/// `GET /v1/auth/me`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Me {
    pub user: MeUser,
    #[serde(default)]
    pub device: Option<MeDevice>,
    #[serde(default)]
    pub sync_cursor: Option<SyncCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MeUser {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MeDevice {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SyncCursor {
    #[serde(default)]
    pub history_seq: i64,
}

/// What a sign-up, sign-in, pairing or single sign-on answers.
#[derive(Deserialize)]
pub(crate) struct Signed {
    pub(crate) token: String,
    pub(crate) user: MeUser,
    #[serde(default)]
    pub(crate) device: Option<MeDevice>,
    #[serde(default)]
    pub(crate) gate_key: Option<String>,
}

/// An answer: its status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Answer {
    /// The body as JSON.
    pub fn json<T: DeserializeOwned>(&self, what: &str) -> Result<T> {
        serde_json::from_slice(&self.body).map_err(|e| Failure::decode(self.status, what, e))
    }
}

struct State {
    file: CloudFile,
    link: Option<Link>,
    trust: Trust,
    http: reqwest::Client,
    reachable: bool,
    info: Option<ServerInfo>,
    info_for: Option<String>,
    lacking: BTreeSet<Feature>,
    history_limits: Option<Limits>,
    events_task: Option<JoinHandle<()>>,
}

struct Inner {
    store: Store,
    state: Mutex<State>,
    status: watch::Sender<CloudStatus>,
    events: broadcast::Sender<CloudEvent>,
    answers: AtomicU64,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(state) = self.state.get_mut()
            && let Some(task) = state.events_task.take()
        {
            task.abort();
        }
    }
}

/// The Copper Cloud client. Cheap to clone; clones share everything.
#[derive(Clone)]
pub struct CloudClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for CloudClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudClient")
            .field("status", &self.status())
            .finish()
    }
}

/// What this machine calls itself, for a fresh `cloud.json` `deviceName`
/// (the Mac uses the computer name; the user can rename it later).
fn default_device_name() -> String {
    let host = host_name();
    // Two test worlds on one machine are two devices; say which is which.
    match std::env::var("COPPER_WORLD") {
        Ok(w) if !w.trim().is_empty() => format!("{host} ({})", w.trim()),
        _ => host,
    }
}

/// Linux: the kernel host name (`/proc/sys/kernel/hostname`, else
/// `/etc/hostname`), else `Linux`.
#[cfg(not(windows))]
fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Linux".to_string())
}

/// Windows: the computer name as the user typed it in Settings
/// (`GetComputerNameExW(ComputerNamePhysicalDnsHostname)`, case kept), else
/// `%COMPUTERNAME%` (the upper-case NetBIOS name), else `Windows`.
#[cfg(windows)]
fn host_name() -> String {
    windows_dns_host_name()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Windows".to_string())
}

#[cfg(windows)]
fn windows_dns_host_name() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{
        ComputerNamePhysicalDnsHostname, GetComputerNameExW,
    };
    // First call: the required size (in UTF-16 units, terminator included).
    let mut len: u32 = 0;
    // SAFETY: a null buffer with size 0 is the documented size query.
    unsafe {
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            std::ptr::null_mut(),
            &mut len,
        )
    };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: `buf` holds `len` writable u16s; on success `len` becomes the
    // number written, terminator excluded.
    let ok =
        unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return None;
    }
    buf.truncate(len as usize);
    String::from_utf16(&buf).ok()
}

/// `X-Copper-Instance`.
pub const GATE_HEADER: &str = "X-Copper-Instance";

impl CloudClient {
    /// Read `cloud.json` from `store` (minting this machine's device id the
    /// first time). Nothing is sent anywhere.
    pub fn open(store: Store) -> std::result::Result<Self, copper_core::StoreError> {
        let loaded = store.load_opt::<CloudFile>()?.value;
        let fresh = loaded.is_none();
        let mut file = loaded.unwrap_or_default();
        if file.device_id.trim().is_empty() {
            file.device_id = copper_core::ids::new_uuid();
        }
        if file.device_name.trim().is_empty() {
            file.device_name = default_device_name();
        }
        let link = file.link.as_ref().and_then(Link::from_saved);
        if file.token.is_none() {
            file.account = None;
        }
        let trust = Trust::new(link.as_ref().and_then(|l| l.fingerprint.as_deref()));
        let http = trust.http_client();
        let state = State {
            file,
            link,
            trust,
            http,
            reachable: false,
            info: None,
            info_for: None,
            lacking: BTreeSet::new(),
            history_limits: None,
            events_task: None,
        };
        let (status, _) = watch::channel(CloudStatus::default());
        let (events, _) = broadcast::channel(256);
        let client = CloudClient {
            inner: Arc::new(Inner {
                store,
                state: Mutex::new(state),
                status,
                events,
                answers: AtomicU64::new(0),
            }),
        };
        if fresh {
            client.with(|s| client.save(s));
        }
        client.publish();
        Ok(client)
    }

    /// Start what runs while signed in (the event stream) and read what the
    /// instance offers. Call once, inside a tokio runtime.
    pub fn start(&self) {
        if self.is_signed_in() {
            self.start_events();
        }
        if self.is_linked() {
            let me = self.clone();
            tokio::spawn(async move {
                me.load_info(false).await;
            });
        }
    }

    // MARK: state

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut guard = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    fn save(&self, state: &State) {
        if let Err(e) = self.inner.store.save(&state.file) {
            tracing::error!(error = %e, "cloud.json not written");
        }
    }

    fn publish(&self) {
        let status = self.with(|s| CloudStatus {
            linked: s.link.is_some(),
            host: s.link.as_ref().map(Link::host),
            fingerprint: s.link.as_ref().and_then(Link::fingerprint_pairs),
            signed_in: s.file.token.is_some() && s.file.account.is_some(),
            account: if s.file.token.is_some() {
                s.file.account.clone()
            } else {
                None
            },
            reachable: s.reachable,
            server_version: s.info.as_ref().and_then(|i| i.version.clone()),
            device_id: s.file.device_id.clone(),
            device_name: s.file.device_name.clone(),
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

    /// Where `cloud.json` and its siblings are kept.
    pub fn store(&self) -> &Store {
        &self.inner.store
    }

    /// Now.
    pub fn status(&self) -> CloudStatus {
        self.inner.status.borrow().clone()
    }

    /// Every change to [`CloudClient::status`].
    pub fn watch_status(&self) -> watch::Receiver<CloudStatus> {
        self.inner.status.subscribe()
    }

    /// The event stream's events, as they come.
    pub fn subscribe(&self) -> broadcast::Receiver<CloudEvent> {
        self.inner.events.subscribe()
    }

    /// The instance this Copper is linked to.
    pub fn link(&self) -> Option<Link> {
        self.with(|s| s.link.clone())
    }

    /// Who is signed in.
    pub fn account(&self) -> Option<Account> {
        self.with(|s| s.file.token.as_ref().and(s.file.account.clone()))
    }

    pub fn is_linked(&self) -> bool {
        self.with(|s| s.link.is_some())
    }

    pub fn is_signed_in(&self) -> bool {
        self.with(|s| s.file.token.is_some() && s.file.account.is_some())
    }

    /// This Copper's device id as kept (uppercase).
    pub fn device_id(&self) -> String {
        self.with(|s| s.file.device_id.clone())
    }

    /// Shown on other devices beside this one's tabs.
    pub fn device_name(&self) -> String {
        self.with(|s| s.file.device_name.clone())
    }

    /// Rename this device here only (see [`CloudClient::rename_device`]).
    pub fn set_device_name(&self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.with(|s| {
            if s.file.device_name != name {
                s.file.device_name = name.to_string();
                self.save(s);
            }
        });
        self.publish();
    }

    /// The sync switches and history cursors (`cloud.json` `sync`).
    pub fn sync_prefs(&self) -> SyncPrefs {
        self.with(|s| s.file.sync.clone())
    }

    /// Change the sync switches / cursors; written through when changed.
    pub fn update_sync_prefs(&self, f: impl FnOnce(&mut SyncPrefs)) {
        self.with(|s| {
            let before = s.file.sync.clone();
            f(&mut s.file.sync);
            if s.file.sync != before {
                self.save(s);
            }
        });
    }

    /// How many requests the server has answered (any status) this run.
    pub fn answers(&self) -> u64 {
        self.inner.answers.load(Ordering::Relaxed)
    }

    pub(crate) fn token(&self) -> Option<String> {
        self.with(|s| s.file.token.clone())
    }

    pub(crate) fn trust(&self) -> Trust {
        self.with(|s| s.trust.clone())
    }

    fn set_reachable(&self, value: bool) {
        let changed = self.with(|s| std::mem::replace(&mut s.reachable, value) != value);
        if changed {
            self.publish();
        }
    }

    // MARK: linking

    pub(crate) fn pin_for(
        endpoint: &Endpoint,
        fingerprint: Option<&str>,
        trusting: Option<&str>,
    ) -> Result<Option<String>> {
        if !endpoint.https {
            return Ok(None);
        }
        if let Some(fp) = fingerprint {
            return Ok(Some(fp.to_string()));
        }
        match trusting {
            Some(t) => normalize_fingerprint(t).map(Some).ok_or_else(|| {
                Failure::new(0, "fingerprint", "That fingerprint isn't 64 hex characters")
            }),
            None => Ok(None),
        }
    }

    fn probe_failure(error: &reqwest::Error, probe: &Trust, endpoint: &Endpoint) -> Failure {
        if probe.pin().is_none()
            && endpoint.https
            && let Some(seen) = probe.seen()
        {
            let mut f = Failure::new(
                0,
                "untrusted",
                "This code has no fingerprint to pin the instance's certificate by, and no public certificate authority vouches for it. Compare the fingerprint below with `copper-cloud doctor` on the server before trusting it.",
            );
            f.seen = Some(seen);
            return f;
        }
        probe.explain(error)
    }

    pub(crate) async fn healthz(
        http: &reqwest::Client,
        probe: &Trust,
        endpoint: &Endpoint,
    ) -> Result<()> {
        let resp = http
            .get(endpoint.url("healthz"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| Self::probe_failure(&e, probe, endpoint))?;
        if resp.status().as_u16() != 200 {
            return Err(Failure::new(
                resp.status().as_u16(),
                "health",
                "The server answered, but not as a Copper Cloud instance",
            ));
        }
        Ok(())
    }

    /// `GET /healthz` through a pin, without keeping anything: is a Copper
    /// Cloud there, and is its certificate the one meant? Read-only; safe
    /// against any instance.
    pub async fn probe_health(endpoint: &Endpoint, fingerprint: Option<&str>) -> Result<String> {
        let probe = Trust::new(fingerprint);
        let http = probe.http_client();
        let resp = http
            .get(endpoint.url("healthz"))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| Self::probe_failure(&e, &probe, endpoint))?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status != 200 {
            return Err(Failure::new(
                status,
                "health",
                "The server answered, but not as a Copper Cloud instance",
            ));
        }
        Ok(text)
    }

    /// Paste handling: a link code connects, a pairing code connects and signs
    /// in. Returns what the text was.
    pub async fn connect_code(&self, text: &str, trusting: Option<&str>) -> Result<Code> {
        let current = self.link();
        let code = parse_code(text, current.as_ref()).ok_or_else(|| {
            if let Some(sso) = crate::link::parse_sso_link(text) {
                return Failure::new(
                    0,
                    "sso_link",
                    format!(
                        "That's a sign-in link for {} — sign in with your company account (Continue with {})",
                        sso.host(),
                        crate::sso::provider_title(&sso.provider)
                    ),
                );
            }
            Failure::new(
                0,
                "code",
                "That isn't a link code or a pairing code — paste the whole copper-cloud://… text",
            )
        })?;
        match &code {
            Code::Link(link) => {
                self.connect(link.clone(), trusting).await?;
            }
            Code::Pairing(pairing) => self.pair(pairing, trusting).await?,
        }
        Ok(code)
    }

    /// Reach the instance with the pin before keeping anything: `/healthz`
    /// must answer through a certificate that matches the fingerprint, and
    /// the key must be accepted. Then the link is kept. A link to another
    /// instance (or another key) signs the account here out first.
    ///
    /// A code without a fingerprint fails with `untrusted` and the
    /// fingerprint the server showed ([`Failure::seen`]); pass it back as
    /// `trusting` once a person has compared it, and the instance is pinned
    /// to it from then on.
    pub async fn connect(&self, mut link: Link, trusting: Option<&str>) -> Result<Link> {
        link.fingerprint = Self::pin_for(&link.endpoint, link.fingerprint.as_deref(), trusting)?;
        let probe = Trust::new(link.fingerprint.as_deref());
        let http = probe.http_client();
        Self::healthz(&http, &probe, &link.endpoint).await?;
        // Any /v1 path is behind the gate, and /auth/me without a session
        // tells the two refusals apart.
        let resp = http
            .get(link.endpoint.url("v1/auth/me"))
            .header(GATE_HEADER, &link.key)
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| probe.explain(&e))?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await.unwrap_or_default();
        if status == 401 && Failure::from_body(&body, 401).code == "instance_key" {
            return Err(Failure::new(
                401,
                "instance_key",
                "The instance refused this key — copy the link code again",
            ));
        }
        let forget = self.with(|s| {
            let forget = s
                .link
                .as_ref()
                .is_some_and(|old| old.endpoint != link.endpoint || old.key != link.key);
            if s.link.as_ref().map(|l| l.endpoint.base()) != Some(link.endpoint.base()) {
                s.info = None;
                s.info_for = None;
                s.lacking.clear();
                s.history_limits = None;
            }
            s.link = Some(link.clone());
            s.file.link = Some(link.to_saved());
            s.trust = probe.fresh();
            s.http = s.trust.http_client();
            s.reachable = true;
            forget
        });
        if forget {
            self.forget_account();
        }
        self.with(|s| self.save(s));
        self.publish();
        tracing::info!(host = %link.host(), "connected");
        let me = self.clone();
        tokio::spawn(async move {
            me.load_info(true).await;
        });
        Ok(link)
    }

    /// Link and sign in with a pairing code, in one step: `/healthz` through
    /// the code's pin, then `POST /v1/auth/pair` without the gate header (the
    /// code is the credential). The answer's link (address, `gate_key`,
    /// fingerprint), account and token are kept in one write.
    pub async fn pair(&self, code: &PairingCode, trusting: Option<&str>) -> Result<()> {
        let pin = Self::pin_for(&code.endpoint, code.fingerprint.as_deref(), trusting)?;
        let probe = Trust::new(pin.as_deref());
        let http = probe.http_client();
        Self::healthz(&http, &probe, &code.endpoint).await?;
        let device = self.device_body();
        let resp = http
            .post(code.endpoint.url("v1/auth/pair"))
            .header("Accept", "application/json")
            .json(&json!({"code": code.code, "device": device}))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| probe.explain(&e))?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(pair_failure(Failure::from_body(&body, status)));
        }
        let answer: Signed = serde_json::from_slice(&body)
            .map_err(|e| Failure::decode(status, "the pairing code", e))?;
        self.keep_signed_in(&code.endpoint, pin, &probe, answer)
            .await
            .map(|_| ())
    }

    /// What pairing and single sign-on share once the server said yes: the
    /// link (the address, the answer's `gate_key` as the key, the pin), the
    /// account and the token are kept in one write, and the device id (and
    /// name) the server answered with are adopted — a code an integration
    /// minted for a device decides which device this is. The session this
    /// Copper had before is ended; the event stream starts.
    pub(crate) async fn keep_signed_in(
        &self,
        endpoint: &Endpoint,
        pin: Option<String>,
        probe: &Trust,
        answer: Signed,
    ) -> Result<Account> {
        let key = answer.gate_key.unwrap_or_default().trim().to_string();
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(Failure::new(
                0,
                "decode",
                "The server signed this Copper in but sent no instance credential",
            ));
        }
        let link = Link {
            endpoint: endpoint.clone(),
            key,
            fingerprint: pin,
        };
        let account = self.account_from(&answer.user, answer.device.as_ref());
        // The session this Copper had before ends on its own instance.
        let previous = self.with(|s| {
            let prev = match (&s.link, &s.file.token) {
                (Some(old), Some(token)) if *token != answer.token => {
                    Some((s.http.clone(), old.clone(), token.clone()))
                }
                _ => None,
            };
            if let Some(task) = s.events_task.take() {
                task.abort();
            }
            if s.link.as_ref().map(|l| l.endpoint.base()) != Some(link.endpoint.base()) {
                s.info = None;
                s.info_for = None;
                s.lacking.clear();
                s.history_limits = None;
            }
            s.link = Some(link.clone());
            s.file.link = Some(link.to_saved());
            s.file.account = Some(account.clone());
            s.file.token = Some(answer.token.clone());
            if let Some(device) = &answer.device {
                if !device.id.trim().is_empty() {
                    s.file.device_id = device.id.trim().to_ascii_uppercase();
                }
                if let Some(name) = device.name.as_deref().map(str::trim)
                    && !name.is_empty()
                {
                    s.file.device_name = name.to_string();
                }
            }
            s.trust = probe.fresh();
            s.http = s.trust.http_client();
            s.reachable = true;
            self.save(s);
            prev
        });
        if let Some((http, old, token)) = previous {
            tokio::spawn(logout(http, old, token));
        }
        self.start_events();
        self.publish();
        tracing::info!(host = %link.host(), "linked and signed in");
        let me = self.clone();
        tokio::spawn(async move {
            me.load_info(true).await;
        });
        let _ = self.me().await;
        Ok(self.account().unwrap_or(account))
    }

    fn account_from(&self, user: &MeUser, device: Option<&MeDevice>) -> Account {
        CloudAccount {
            user_id: user.id.to_ascii_uppercase(),
            email: user.email.clone(),
            display_name: user
                .display_name
                .clone()
                .unwrap_or_else(|| user.email.clone()),
            device_id: device
                .map(|d| d.id.to_ascii_uppercase())
                .unwrap_or_else(|| self.device_id().to_ascii_uppercase()),
            extra: Default::default(),
        }
    }

    /// Sign out, forget the instance, stop syncing. The server is told about
    /// the sign-out if it can be reached; nothing waits for it.
    pub fn disconnect(&self) {
        let logout_with = self.with(|s| match (&s.link, &s.file.token) {
            (Some(link), Some(token)) => Some((s.http.clone(), link.clone(), token.clone())),
            _ => None,
        });
        if let Some((http, link, token)) = logout_with
            && tokio::runtime::Handle::try_current().is_ok()
        {
            tokio::spawn(logout(http, link, token));
        }
        self.forget_account();
        self.with(|s| {
            s.link = None;
            s.file.link = None;
            s.info = None;
            s.info_for = None;
            s.lacking.clear();
            s.history_limits = None;
            s.reachable = false;
            s.trust = Trust::new(None);
            s.http = s.trust.http_client();
            self.save(s);
        });
        self.publish();
        tracing::info!("disconnected");
    }

    /// `/healthz` through the pinned client: is the instance there? Sets
    /// `reachable`; never signs anyone in or out.
    pub async fn ping(&self) -> bool {
        let Some((http, link)) = self.with(|s| s.link.clone().map(|l| (s.http.clone(), l))) else {
            return false;
        };
        let ok = http
            .get(link.endpoint.url("healthz"))
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .is_ok_and(|r| r.status().as_u16() == 200);
        self.set_reachable(ok);
        ok
    }

    // MARK: what the instance can do

    /// `GET /v1/info`, kept for this link. Asked once per link unless
    /// `force`; a failure keeps what was known.
    pub async fn load_info(&self, force: bool) -> Option<ServerInfo> {
        let base = self.link()?.endpoint.base();
        if !force
            && let Some(info) = self.with(|s| {
                (s.info_for.as_deref() == Some(&base))
                    .then(|| s.info.clone())
                    .flatten()
            })
        {
            return Some(info);
        }
        let answer = match self.request("GET", "/v1/info", None, &[]).await {
            Ok(a) => a,
            Err(_) => return self.with(|s| s.info.clone()),
        };
        let raw: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
        let text = |k: &str| {
            raw.get(k)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
        };
        let info = ServerInfo {
            name: text("name"),
            version: text("version").filter(|v| !v.is_empty()),
            signup: raw.get("signup").and_then(Value::as_bool),
            access_mode: text("access_mode"),
            features: raw
                .get("features")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            raw: raw.clone(),
        };
        let still = self.with(|s| {
            if s.link.as_ref().map(|l| l.endpoint.base()) != Some(base.clone()) {
                return false;
            }
            if info.version.is_some()
                && s.info.as_ref().and_then(|i| i.version.clone()) != info.version
            {
                s.lacking.clear();
            }
            s.history_limits = Some(Limits::from_info(Some(&raw)));
            s.info = Some(info.clone());
            s.info_for = Some(base.clone());
            true
        });
        self.publish();
        still.then_some(info)
    }

    /// What the instance says it runs, once known.
    pub fn server_version(&self) -> Option<String> {
        self.with(|s| s.info.as_ref().and_then(|i| i.version.clone()))
    }

    /// True or false once known; `None` while the instance hasn't said.
    pub fn supports(&self, feature: Feature) -> Option<bool> {
        self.with(|s| {
            if s.lacking.contains(&feature) {
                return Some(false);
            }
            let version = s.info.as_ref()?.version.as_deref()?;
            Some(version_at_least(version, feature.since()))
        })
    }

    /// The instance answered a feature's route with a bare `404 not_found`
    /// while its version was unknown: it predates it.
    pub fn lacks(&self, feature: Feature) {
        self.with(|s| {
            s.lacking.insert(feature);
        });
    }

    /// The history limits for this link, asking `/v1/info` once if they
    /// aren't known yet; copper-cloud's defaults if it doesn't answer.
    pub async fn history_limits(&self) -> Limits {
        if let Some(l) = self.with(|s| s.history_limits) {
            return l;
        }
        self.load_info(false).await;
        self.with(|s| s.history_limits).unwrap_or(Limits::STANDARD)
    }

    /// Smaller limits a refusal said (kept for this link).
    pub fn set_history_limits(&self, limits: Limits) {
        self.with(|s| s.history_limits = Some(limits));
    }

    /// Whether history can be deleted on the cloud (`history_delete` in
    /// `/v1/info`'s features); `None` until it answers.
    pub fn deletes_history(&self) -> Option<bool> {
        self.with(|s| {
            s.info
                .as_ref()
                .map(|i| i.features.contains("history_delete"))
        })
    }

    // MARK: account

    fn checked_email(email: &str) -> Result<String> {
        let email = email.trim();
        let parts: Vec<&str> = email.split('@').filter(|p| !p.is_empty()).collect();
        if parts.len() != 2
            || !parts[1].contains('.')
            || email.contains(' ')
            || email.matches('@').count() != 1
        {
            return Err(Failure::new(
                0,
                "email",
                "That doesn't look like an email address",
            ));
        }
        Ok(email.to_string())
    }

    pub(crate) fn device_body(&self) -> Value {
        json!({"id": wire_id(&self.device_id()), "name": self.device_name()})
    }

    /// Make an account on the linked instance and sign in as it.
    pub async fn sign_up(
        &self,
        email: &str,
        password: &str,
        display_name: &str,
    ) -> Result<Account> {
        let email = Self::checked_email(email)?;
        if password.chars().count() < MINIMUM_PASSWORD {
            return Err(Failure::new(
                0,
                "password",
                format!("Use at least {MINIMUM_PASSWORD} characters for the password"),
            ));
        }
        let name = display_name.trim();
        if name.is_empty() {
            return Err(Failure::new(
                0,
                "display_name",
                "Add a name to show on shared canvases",
            ));
        }
        let body = json!({"email": email, "password": password, "display_name": name, "device": self.device_body()});
        self.signed_in("/v1/auth/signup", body).await
    }

    /// Sign in to an account on the linked instance.
    pub async fn sign_in(&self, email: &str, password: &str) -> Result<Account> {
        let email = Self::checked_email(email)?;
        if password.is_empty() {
            return Err(Failure::new(0, "password", "Enter your password"));
        }
        let body = json!({"email": email, "password": password, "device": self.device_body()});
        self.signed_in("/v1/auth/login", body).await
    }

    async fn signed_in(&self, path: &str, body: Value) -> Result<Account> {
        if !self.is_linked() {
            return Err(Failure::new(0, "not_linked", "Connect to a cloud first"));
        }
        let answer: Signed = self
            .request_json("POST", path, Some(body), &[])
            .await
            .map_err(auth_failure)?;
        let account = self.account_from(&answer.user, answer.device.as_ref());
        self.adopt(answer.token, account.clone());
        let _ = self.me().await;
        Ok(self.account().unwrap_or(account))
    }

    /// Keep a fresh sign-in. A session it replaces is ended on the server.
    fn adopt(&self, token: String, account: Account) {
        let previous = self.with(|s| {
            let prev = match (&s.link, &s.file.token) {
                (Some(link), Some(old)) if *old != token => {
                    Some((s.http.clone(), link.clone(), old.clone()))
                }
                _ => None,
            };
            s.file.token = Some(token);
            s.file.account = Some(account);
            self.save(s);
            prev
        });
        if let Some((http, link, old)) = previous {
            tokio::spawn(logout(http, link, old));
        }
        self.start_events();
        self.publish();
    }

    /// Ends the session on the server too, when it can be reached. Signed out
    /// here either way.
    pub async fn sign_out(&self) {
        if !self.is_signed_in() {
            return;
        }
        let _ = self.request("POST", "/v1/auth/logout", None, &[]).await;
        self.forget_account();
        tracing::info!("signed out");
    }

    /// Signed out here: the token and the account go, the event stream stops.
    pub fn forget_account(&self) {
        let had = self.with(|s| {
            if let Some(task) = s.events_task.take() {
                task.abort();
            }
            let had = s.file.token.is_some() || s.file.account.is_some();
            s.file.token = None;
            s.file.account = None;
            if had {
                self.save(s);
            }
            had
        });
        if had {
            self.publish();
        }
    }

    /// Ask who this session is, and keep the answer. A 401 `session` signs
    /// out.
    pub async fn me(&self) -> Result<Me> {
        if self.token().is_none() {
            return Err(Failure::not_signed_in());
        }
        let me: Me = self.request_json("GET", "/v1/auth/me", None, &[]).await?;
        let changed = self.with(|s| {
            let Some(account) = s.file.account.as_mut() else {
                return false;
            };
            let email = me.user.email.clone();
            let name = me
                .user
                .display_name
                .clone()
                .unwrap_or_else(|| account.display_name.clone());
            if account.email == email && account.display_name == name {
                return false;
            }
            account.email = email;
            account.display_name = name;
            self.save(s);
            true
        });
        if changed {
            self.publish();
        }
        Ok(me)
    }

    /// Rename this device, here and on the server (`PATCH /v1/devices/{id}`).
    pub async fn rename_device(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(());
        }
        self.set_device_name(name);
        let Some(account) = self.account() else {
            return Ok(());
        };
        self.request(
            "PATCH",
            &format!("/v1/devices/{}", wire_id(&account.device_id)),
            Some(json!({"name": name})),
            &[],
        )
        .await?;
        Ok(())
    }

    /// The account's devices, most recently seen first.
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>> {
        self.request_json("GET", "/v1/devices", None, &[]).await
    }

    /// Revoke a device's sessions and delete its `tabs:` document.
    pub async fn remove_device(&self, id: &str) -> Result<()> {
        self.request("DELETE", &format!("/v1/devices/{}", wire_id(id)), None, &[])
            .await
            .map(|_| ())
    }

    // MARK: pairing another device

    /// Mint a single-use pairing code (ten minutes) that links another Copper
    /// to this instance and signs it in as this account.
    pub async fn mint_pairing_code(&self, device_name: Option<&str>) -> Result<MintedPairing> {
        let Some(link) = self.link().filter(|_| self.is_signed_in()) else {
            return Err(Failure::new(
                0,
                "not_signed_in",
                "Sign in first — a pairing code signs the other device in as you",
            ));
        };
        let mut body = json!({});
        if let Some(name) = device_name.map(str::trim).filter(|n| !n.is_empty()) {
            body["device_name"] = json!(name);
        }
        #[derive(Deserialize)]
        struct Wire {
            id: String,
            code: String,
            link: String,
            #[serde(default)]
            created_at: Option<String>,
            expires_at: String,
            #[serde(default)]
            device_name: Option<String>,
        }
        let wire: Wire = self
            .request_json("POST", "/v1/auth/pairing", Some(body), &[])
            .await
            .map_err(mint_failure)?;
        let mut shown = wire.link.trim().to_string();
        match parse_code(&shown, None) {
            Some(Code::Pairing(mut parsed)) if parsed.code == wire.code => {
                if parsed.fingerprint.is_none()
                    && parsed.endpoint.https
                    && link.fingerprint.is_some()
                {
                    parsed.fingerprint = link.fingerprint.clone();
                    shown = parsed.link();
                }
            }
            _ => {
                shown = PairingCode {
                    endpoint: link.endpoint.clone(),
                    code: wire.code.clone(),
                    fingerprint: link.fingerprint.clone(),
                }
                .link();
            }
        }
        Ok(MintedPairing {
            id: wire.id,
            code: wire.code,
            link: shown,
            created_at: wire.created_at,
            expires_at: wire.expires_at,
            device_name: wire.device_name,
        })
    }

    /// My unused, unexpired pairing codes, newest first.
    pub async fn pairing_codes(&self) -> Result<Vec<PairingInfo>> {
        if !self.is_signed_in() {
            return Err(Failure::new(0, "not_signed_in", "Not signed in"));
        }
        self.request_json("GET", "/v1/auth/pairing", None, &[])
            .await
            .map_err(mint_failure)
    }

    /// Revoke one of my unused codes.
    pub async fn revoke_pairing_code(&self, id: &str) -> Result<()> {
        if !self.is_signed_in() {
            return Err(Failure::new(0, "not_signed_in", "Not signed in"));
        }
        match self
            .request(
                "DELETE",
                &format!("/v1/auth/pairing/{}", id.to_ascii_lowercase()),
                None,
                &[],
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(mut f) => {
                if f.status == 404 {
                    f.message = "That code is gone already — used, expired or revoked".into();
                }
                Err(f)
            }
        }
    }

    // MARK: asking

    /// One request to the instance: the gate key, the bearer token once
    /// signed in, a JSON body, a 15 s limit, the pinned client. A GET is tried
    /// up to three times when the network or a gateway fails it; nothing else
    /// is ever repeated. A non-2xx answer is the server's own `{error,
    /// message}`; a 401 `session` signs this Copper out.
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        json: Option<Value>,
        query: &[(&str, &str)],
    ) -> Result<Answer> {
        let body = match json {
            Some(v) => {
                Some(serde_json::to_vec(&v).map_err(|e| Failure::new(0, "encode", e.to_string()))?)
            }
            None => None,
        };
        self.send(method, path, body, query).await
    }

    /// [`CloudClient::request`] with a body that is JSON already (history
    /// entries are measured before they go).
    pub async fn request_raw(
        &self,
        method: &str,
        path: &str,
        body: Vec<u8>,
        query: &[(&str, &str)],
    ) -> Result<Answer> {
        self.send(method, path, Some(body), query).await
    }

    /// [`CloudClient::request`], decoded.
    pub async fn request_json<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        json: Option<Value>,
        query: &[(&str, &str)],
    ) -> Result<T> {
        let answer = self.request(method, path, json, query).await?;
        answer.json(&format!("{method} {path}"))
    }

    /// The full URL for `path` with `query` (sorted by key, as the Mac does).
    pub(crate) fn url_for(
        endpoint: &Endpoint,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<url::Url> {
        let mut url = url::Url::parse(&endpoint.url(path))
            .map_err(|_| Failure::new(0, "bad_path", format!("Not a path: {path}")))?;
        if !query.is_empty() {
            let mut sorted = query.to_vec();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let mut pairs = url.query_pairs_mut();
            for (k, v) in sorted {
                pairs.append_pair(k, v);
            }
        }
        Ok(url)
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        query: &[(&str, &str)],
    ) -> Result<Answer> {
        let Some((link, token)) = self.with(|s| s.link.clone().map(|l| (l, s.file.token.clone())))
        else {
            return Err(Failure::not_linked());
        };
        let url = Self::url_for(&link.endpoint, path, query)?;
        let method_name = method.to_ascii_uppercase();
        let verb = reqwest::Method::from_bytes(method_name.as_bytes())
            .map_err(|_| Failure::new(0, "bad_method", method_name.clone()))?;
        let idempotent = verb == reqwest::Method::GET;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let (http, trust) = self.with(|s| (s.http.clone(), s.trust.clone()));
            let mut req = http
                .request(verb.clone(), url.clone())
                .timeout(REQUEST_TIMEOUT)
                .header(GATE_HEADER, &link.key)
                .header("Accept", "application/json");
            if let Some(token) = &token {
                req = req.bearer_auth(token);
            }
            if let Some(body) = &body {
                req = req
                    .header("Content-Type", "application/json")
                    .body(body.clone());
            }
            let result = req.send().await;
            match result {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let data = match resp.bytes().await {
                        Ok(b) => b.to_vec(),
                        Err(e) => {
                            let f = trust.explain(&e);
                            if idempotent && attempt < 3 && f.is_network() {
                                tokio::time::sleep(Duration::from_millis(600 * attempt)).await;
                                continue;
                            }
                            return Err(f);
                        }
                    };
                    self.set_reachable(true);
                    self.inner.answers.fetch_add(1, Ordering::Relaxed);
                    if (200..300).contains(&status) {
                        return Ok(Answer { status, body: data });
                    }
                    if idempotent && attempt < 3 && matches!(status, 502..=504) {
                        tokio::time::sleep(Duration::from_millis(600 * attempt)).await;
                        continue;
                    }
                    let mut failure = Failure::from_body(&data, status);
                    if status == 409 {
                        failure.body = Some(data);
                    }
                    if status == 401
                        && (failure.code == "session"
                            || (failure.code == "unauthorized" && path.starts_with("/v1/auth/me")))
                        && token.is_some()
                    {
                        tracing::info!("the server ended this session — signed out");
                        self.forget_account();
                    }
                    return Err(failure);
                }
                Err(e) => {
                    let failure = trust.explain(&e);
                    if failure.is_network() {
                        self.set_reachable(false);
                    }
                    if idempotent && attempt < 3 && failure.is_network() {
                        tokio::time::sleep(Duration::from_millis(600 * attempt)).await;
                        continue;
                    }
                    return Err(failure);
                }
            }
        }
    }

    // MARK: events

    fn start_events(&self) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        let task = tokio::spawn(events_loop(weak));
        self.with(|s| {
            if let Some(old) = s.events_task.replace(task) {
                old.abort();
            }
        });
    }

    fn deliver(&self, data: Value) {
        let kind = data
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if kind == "open" {
            self.set_reachable(true);
            // Back after a gap: the instance may have been upgraded meanwhile.
            let me = self.clone();
            tokio::spawn(async move {
                me.load_info(true).await;
            });
        }
        let _ = self.inner.events.send(CloudEvent { kind, data });
    }
}

async fn logout(http: reqwest::Client, link: Link, token: String) {
    let _ = http
        .post(link.endpoint.url("v1/auth/logout"))
        .header(GATE_HEADER, &link.key)
        .bearer_auth(token)
        .timeout(Duration::from_secs(8))
        .send()
        .await;
}

/// The pairing refusals, in words (Mac: `Cloud.pairFailure`).
pub fn pair_failure(mut failure: Failure) -> Failure {
    match (failure.status, failure.code.as_str()) {
        (_, "pairing_code") => {
            failure.message = "That pairing code doesn't work any more — it was used already, revoked, or is past its 10 minutes. Make a new one on the other device.".into()
        }
        (_, "account_disabled") => {
            failure.message = "The account that made this code is disabled on this instance — ask its admin".into()
        }
        (429, _) | (_, "rate_limited") => failure.message = "Too many tries — wait a minute and try again".into(),
        (404, _) | (401, "instance_key") => {
            failure.message = "This instance doesn't take pairing codes yet — update copper-cloud, or connect with a link code and sign in".into()
        }
        (403, _) if failure.message.is_empty() || failure.message == "forbidden" => {
            failure.message = "The instance refused to pair this device — ask its admin".into()
        }
        _ => {}
    }
    failure
}

/// The sign-up / sign-in refusals, in words (Mac: `Cloud.authFailure`).
pub fn auth_failure(mut failure: Failure) -> Failure {
    match (failure.status, failure.code.as_str()) {
        (401, "credentials") | (401, "unauthorized") => {
            failure.message = "Wrong email or password".into()
        }
        (409, _) | (_, "email_taken") => {
            failure.message = "There's already an account with that email — sign in instead".into()
        }
        (403, _) | (_, "signup_disabled") => {
            if !failure.message.to_lowercase().contains("invite") {
                failure.message =
                    "This instance isn't taking new accounts — ask its admin for an invite".into();
            }
        }
        (429, _) => failure.message = "Too many tries — wait a minute and try again".into(),
        (401, "instance_key") => {
            failure.message =
                "The instance refused this Copper's key — connect again with a fresh link code"
                    .into()
        }
        _ => {}
    }
    failure
}

/// The pairing-code minting refusals (Mac: `Cloud.mintFailure`).
pub fn mint_failure(mut failure: Failure) -> Failure {
    match (failure.status, failure.code.as_str()) {
        (404, _) | (405, _) => {
            failure.message =
                "This instance doesn't make pairing codes yet — update copper-cloud".into()
        }
        (429, _) | (_, "rate_limited") => {
            failure.message = "Too many tries — wait a minute and try again".into()
        }
        _ => {}
    }
    failure
}

enum Outcome {
    Opened,
    Refused(u16, Vec<u8>),
    Failed(Failure),
}

/// `GET /v1/sync/events`, held open while signed in. Reconnects after 1, 2,
/// 4 … 30 s (±30 % jitter).
async fn events_loop(weak: Weak<Inner>) {
    let mut delay = 1.0_f64;
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let client = CloudClient { inner };
        let Some((link, token, http, trust)) = client.with(|s| match (&s.link, &s.file.token) {
            (Some(l), Some(t)) => Some((l.clone(), t.clone(), s.http.clone(), s.trust.clone())),
            _ => None,
        }) else {
            return;
        };
        let outcome = read_events(&client, &http, &trust, &link, &token).await;
        match outcome {
            Outcome::Opened => delay = 1.0,
            Outcome::Refused(status, body) => {
                let failure = Failure::from_body(&body, status);
                if status == 401 {
                    if failure.code == "session" {
                        tracing::info!("the server ended this session — signed out");
                        // forget_account aborts this task; do it from another.
                        let c = client.clone();
                        tokio::spawn(async move { c.forget_account() });
                    }
                    return;
                }
                tracing::warn!(message = %failure.message, "events refused");
            }
            Outcome::Failed(f) => {
                if f.is_network() {
                    client.set_reachable(false);
                }
            }
        }
        drop(client);
        let wait = delay * (0.7 + fastrand::f64() * 0.6);
        delay = (delay * 2.0).min(30.0);
        tokio::time::sleep(Duration::from_secs_f64(wait)).await;
    }
}

async fn read_events(
    client: &CloudClient,
    http: &reqwest::Client,
    trust: &Trust,
    link: &Link,
    token: &str,
) -> Outcome {
    let resp = match http
        .get(link.endpoint.url("v1/sync/events"))
        .header(GATE_HEADER, &link.key)
        .header("Accept", "text/event-stream")
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(trust.explain(&e)),
    };
    let status = resp.status().as_u16();
    if status != 200 {
        let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
        return Outcome::Refused(status, body.into_iter().take(4096).collect());
    }
    client.deliver(json!({"type": "open"}));
    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    loop {
        // A keepalive comes every 15 s; two minutes of nothing is a dead line.
        match tokio::time::timeout(Duration::from_secs(120), stream.next()).await {
            Err(_) | Ok(None) => break,
            Ok(Some(Err(e))) => {
                for event in parser.finish() {
                    client.deliver(event);
                }
                return Outcome::Failed(trust.explain(&e));
            }
            Ok(Some(Ok(bytes))) => {
                for event in parser.feed(&bytes) {
                    client.deliver(event);
                }
            }
        }
    }
    for event in parser.finish() {
        client.deliver(event);
    }
    Outcome::Opened
}

/// Server-sent events, read byte by byte so blank lines (the end of an event)
/// are seen. Events whose data has no `type` get the event name as one; data
/// that isn't a JSON object is dropped.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    name: String,
    payload: Vec<String>,
}

impl SseParser {
    /// Feed bytes; returns the events they completed.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Value> {
        let mut out = vec![];
        for &b in bytes {
            if b == b'\n' {
                let text = String::from_utf8_lossy(&self.line).into_owned();
                self.line.clear();
                if text.is_empty() {
                    if let Some(e) = self.dispatch() {
                        out.push(e);
                    }
                    continue;
                }
                if text.starts_with(':') {
                    continue;
                }
                let (key, value) = match text.split_once(':') {
                    Some((k, v)) => (k.to_string(), v.strip_prefix(' ').unwrap_or(v).to_string()),
                    None => (text.clone(), String::new()),
                };
                if key == "event" {
                    self.name = value;
                } else if key == "data" {
                    self.payload.push(value);
                }
            } else if b != b'\r' {
                self.line.push(b);
                if self.line.len() > 1_000_000 {
                    self.line.clear();
                }
            }
        }
        out
    }

    /// The stream ended: what was pending.
    pub fn finish(&mut self) -> Vec<Value> {
        self.dispatch().into_iter().collect()
    }

    fn dispatch(&mut self) -> Option<Value> {
        let name = std::mem::take(&mut self.name);
        let payload = std::mem::take(&mut self.payload);
        if payload.is_empty() {
            return None;
        }
        let mut object = serde_json::from_str::<Value>(&payload.join("\n")).ok()?;
        let map = object.as_object_mut()?;
        if !map.contains_key("type") && !name.is_empty() {
            map.insert("type".into(), Value::String(name));
        }
        Some(object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(version_at_least("0.10.1", "0.3.0"));
        assert!(version_at_least("0.3.0", "0.3.0"));
        assert!(!version_at_least("0.2.9", "0.3.0"));
        assert!(version_at_least("v0.6.0-rc.1", "0.6.0"));
        assert!(version_at_least("1", "0.8.0"));
        assert!(!version_at_least("0.8", "0.8.1"));
    }

    #[test]
    fn sse_events() {
        let mut p = SseParser::default();
        let mut out = p.feed(b"event: ready\ndata: {\"device_id\":\"x\"}\n\n: keepalive\n\nevent: doc\r\ndata: {\"type\":\"doc\",\"domain\":\"spaces\",");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "ready");
        out = p
            .feed(b"\"version\":8}\n\nevent: resync\ndata: {\"skipped\":12}\n\ndata: not json\n\n");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["domain"], "spaces");
        assert_eq!(out[1]["type"], "resync");
        assert!(p.feed(b"event: x\ndata: {\"a\":1}").is_empty());
        assert!(p.feed(b"\n").is_empty());
        assert_eq!(p.finish()[0]["type"], "x");
    }

    #[test]
    fn failures_in_words() {
        let f = auth_failure(Failure::from_body(
            br#"{"error":"conflict","message":"conflict"}"#,
            409,
        ));
        assert!(f.message.contains("already an account"));
        let f = pair_failure(Failure::from_body(
            br#"{"error":"pairing_code","message":"unauthorized"}"#,
            401,
        ));
        assert!(f.message.contains("pairing code doesn't work"));
        let f = auth_failure(Failure::from_body(
            br#"{"error":"forbidden","message":"needs an invite"}"#,
            403,
        ));
        assert_eq!(f.message, "needs an invite");
    }

    #[test]
    fn query_is_sorted_and_encoded() {
        let e = Endpoint {
            https: true,
            host: "h".into(),
            port: Some(8443),
        };
        let url = CloudClient::url_for(
            &e,
            "/v1/sync/history",
            &[("since", "2026-10-01T14:00:00+02:00"), ("device", "x")],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://h:8443/v1/sync/history?device=x&since=2026-10-01T14%3A00%3A00%2B02%3A00"
        );
    }

    #[test]
    fn opens_and_keeps_cloud_json_private() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(copper_core::Paths {
            data: dir.path().join("data"),
            config: dir.path().join("config"),
        });
        let client = CloudClient::open(store.clone()).unwrap();
        let id = client.device_id();
        assert_eq!(id, id.to_uppercase());
        assert!(!client.is_linked());
        let path = store.path(copper_core::DataFile::Cloud);
        assert!(path.is_file());
        // Unix: 0600. Windows has no mode bits; the store relies on the
        // per-user profile ACL there (copper_core::store::write_atomic).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // A second open reads the same device.
        let again = CloudClient::open(store).unwrap();
        assert_eq!(again.device_id(), id);
    }
}
