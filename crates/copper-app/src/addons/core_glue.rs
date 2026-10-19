//! The one door between this lane and the shared addon core
//! (`copper_core::addons`, `copper_cloud::addons`; CONTRACT §8). Every core
//! call the GTK side makes is here, so a change in the core's shape is one
//! file's work — and the Windows shell can read this as the worked example.
//! (The Mac carries the same rules in Swift: `Fork/Addons/CloudAddons.swift`
//! and `AddonStore.swift`.)
//!
//! * [`start`] makes the app's `AddonSync` over the cloud client, starts it
//!   on the cloud's tokio runtime, and follows its status watch and change
//!   stream on the main loop (→ `addons::changed`).
//! * [`active`], [`list`], [`pending`], [`status`]: what the UI draws.
//! * [`set_enabled`], [`remove`], [`check_now`], [`install_pending`]: what
//!   the person does. File work runs on the cloud runtime, never on the
//!   main loop.
//! * [`serve`]: `copper-addon://` answers (the core's `scheme::serve`).
//! * [`dispatch`]: one bridge message, run by the core on the cloud runtime
//!   with this lane's [`Host`]; the reply's JS comes back on the main
//!   thread.
//!
//! No cloud client (an unreadable `cloud.json`) still runs a developer
//! folder (`COPPER_ADDONS_DEV_DIR`), from the store alone.

use super::host::Host;
use super::oauth::Outcome;
use crate::shell::Shell;
use copper_cloud::addons::{AddonSync, HttpFetcher, SyncStatus};
use copper_core::addons::bridge::{
    AgentCall, BridgeError, Fetcher, Identity, PageInfo, agent_unavailable_state,
};
use copper_core::addons::{
    ActiveAddon, AddonState, AddonStore, BRIDGE_JS, BridgeHost, Capability, Context, Platform,
    SlotId, handle_message,
};
use serde_json::Value;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

/// The slot this lane shows.
const SLOT: SlotId = SlotId::AgentSidebar;

/// The addon the slot shows, ready to serve and run.
#[derive(Debug, Clone, PartialEq)]
pub struct Active {
    pub id: String,
    pub name: String,
    pub version: String,
    pub sha256: String,
    /// Resolved settings (`copper.settings()`).
    pub settings: Value,
    /// The unpacked bundle (or the developer folder).
    pub dir: PathBuf,
    /// From `COPPER_ADDONS_DEV_DIR`.
    pub dev: bool,
    core: ActiveAddon,
}

impl Active {
    fn of(a: ActiveAddon) -> Active {
        Active {
            id: a.installed.id.clone(),
            name: a.installed.display_name().to_string(),
            version: a.installed.version.clone(),
            sha256: a.installed.sha256.clone(),
            settings: a.installed.settings.clone(),
            dir: a.dir.clone(),
            dev: a.dev,
            core: a,
        }
    }
}

/// One addon for Settings › Addons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    pub name: String,
    pub version: String,
    /// "Agent sidebar".
    pub slot_name: String,
    /// The cloud it came from (`host:port`).
    pub source: String,
    /// Not turned off here.
    pub enabled: bool,
    /// The staged version waiting for Install.
    pub pending: Option<String>,
    pub installed: bool,
    pub dev: bool,
}

/// A staged update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub id: String,
    pub name: String,
    pub version: String,
    pub slot_name: String,
}

/// Where the cloud check stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// Linked and signed in.
    pub linked: bool,
    pub checking: bool,
    pub error: Option<String>,
    /// "3 minutes ago".
    pub checked: Option<String>,
}

/// One `copper-addon://` answer.
pub struct Served {
    pub status: u16,
    pub mime: &'static str,
    pub body: Vec<u8>,
    pub headers: Vec<(&'static str, String)>,
}

struct Glue {
    sync: Option<AddonSync>,
    store: AddonStore,
    /// Without a cloud client: the state as loaded (the developer folder).
    local: RefCell<AddonState>,
    fetcher: Arc<dyn Fetcher>,
}

thread_local! {
    static GLUE: RefCell<Option<Rc<Glue>>> = const { RefCell::new(None) };
}

fn glue() -> Option<Rc<Glue>> {
    GLUE.with(|g| g.borrow().clone())
}

/// This run is the addons self-test (which stages its own fixture and must
/// not have an unlinked check take it away again).
fn selftest_run() -> bool {
    cfg!(debug_assertions) && std::env::var("COPPER_SELFTEST").is_ok_and(|v| v == "addons")
}

/// Make and start the app's addon sync; `on_change` runs on the main loop
/// whenever its status or a slot changes.
pub fn start(shell: &Shell, on_change: fn(&Shell)) {
    if glue().is_some() {
        return;
    }
    let rt = crate::cloud::runtime();
    let fetcher: Arc<dyn Fetcher> = Arc::new(HttpFetcher::with_runtime(Some(rt.handle().clone())));
    let sync = crate::cloud::get().map(|c| {
        let _rt = rt.enter();
        AddonSync::new(c.client.clone())
    });
    let store = match &sync {
        Some(s) => s.store().clone(),
        None => AddonStore::new(shell.store().paths()),
    };
    let local = if sync.is_some() {
        AddonState::default()
    } else {
        store.load()
    };
    GLUE.with(|g| {
        *g.borrow_mut() = Some(Rc::new(Glue {
            sync: sync.clone(),
            store,
            local: RefCell::new(local),
            fetcher,
        }))
    });
    let Some(sync) = sync else {
        tracing::info!("no Copper Cloud client: addons from a developer folder only");
        return;
    };
    if selftest_run() {
        tracing::info!("addons self-test: the cloud check is not started");
    } else {
        let _rt = rt.enter();
        sync.start();
    }
    let mut status = sync.watch();
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        while status.changed().await.is_ok() {
            let Some(s) = weak.upgrade() else { break };
            on_change(&s);
        }
    });
    let mut changes = sync.subscribe();
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match changes.recv().await {
                Ok(c) => tracing::info!(change = ?c, "addon change"),
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
            let Some(s) = weak.upgrade() else { break };
            on_change(&s);
        }
    });
}

fn sync_status() -> SyncStatus {
    glue()
        .and_then(|g| g.sync.as_ref().map(AddonSync::status))
        .unwrap_or_default()
}

/// What the Agent sidebar shows now (`None`: Copper's placeholder).
pub fn active() -> Option<Active> {
    let g = glue()?;
    let a = match &g.sync {
        Some(s) => s.active(SLOT),
        None => g.store.active(&g.local.borrow(), SLOT),
    }?;
    Some(Active::of(a))
}

/// The installed, pending and developer addons, one per slot.
pub fn list() -> Vec<Listed> {
    let st = sync_status();
    let source = st.cloud.clone().unwrap_or_default();
    let mut out: Vec<Listed> = Vec::new();
    if let Some(d) = active().filter(|a| a.dev) {
        out.push(Listed {
            id: d.id.clone(),
            name: d.name.clone(),
            version: d.version.clone(),
            slot_name: SLOT.name().to_string(),
            source: String::new(),
            enabled: true,
            pending: None,
            installed: false,
            dev: true,
        });
    }
    for i in &st.installed {
        out.push(Listed {
            id: i.id.clone(),
            name: i.name.clone(),
            version: i.version.clone(),
            slot_name: i.slot.name().to_string(),
            source: source.clone(),
            enabled: i.enabled,
            pending: None,
            installed: true,
            dev: false,
        });
    }
    for p in &st.pending {
        match out.iter_mut().find(|l| l.id == p.id && !l.dev) {
            Some(l) => l.pending = Some(p.version.clone()),
            None => out.push(Listed {
                id: p.id.clone(),
                name: p.name.clone(),
                version: p.version.clone(),
                slot_name: p.slot.name().to_string(),
                source: source.clone(),
                enabled: p.enabled,
                pending: Some(p.version.clone()),
                installed: false,
                dev: false,
            }),
        }
    }
    out
}

/// The staged update for the Agent sidebar, if any.
pub fn pending() -> Option<Pending> {
    sync_status()
        .pending
        .into_iter()
        .find(|p| p.slot == SLOT)
        .map(|p| Pending {
            id: p.id,
            name: p.name,
            version: p.version,
            slot_name: p.slot.name().to_string(),
        })
}

/// Linked, checking, the last error and when it last asked.
pub fn status() -> Status {
    let st = sync_status();
    Status {
        linked: st.linked,
        checking: st.checking,
        error: st.last_error,
        checked: st.checked_at.map(|d| crate::cloud::ago(d.unix() as f64)),
    }
}

/// The developer folder in use, if any.
pub fn dev_dir() -> Option<PathBuf> {
    active().filter(|a| a.dev).map(|a| a.dir).or_else(|| {
        std::env::var_os(copper_core::addons::state::DEV_DIR_ENV)
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .filter(|_| glue().is_some_and(|g| g.local.borrow().dev.is_some()))
    })
}

/// "Use in Agent sidebar" for addon `id`.
pub fn set_enabled(_shell: &Shell, id: &str, on: bool) {
    let Some(sync) = glue().and_then(|g| g.sync.clone()) else {
        return;
    };
    let id = id.to_string();
    crate::cloud::run_then(async move { sync.set_enabled(&id, on) }, |_| {});
}

/// "Remove": addon `id` leaves this Copper until the cloud changes.
pub fn remove(_shell: &Shell, id: &str) {
    let Some(sync) = glue().and_then(|g| g.sync.clone()) else {
        return;
    };
    let id = id.to_string();
    crate::cloud::run_then(async move { sync.remove(&id) }, |_| {});
}

/// "Check for addon updates" (and every Copper update check).
pub fn check_now() {
    if selftest_run() {
        return;
    }
    if let Some(sync) = glue().and_then(|g| g.sync.clone()) {
        sync.kick();
    }
}

/// Install the staged update; `done` gets the "<name> <version>" of what
/// now runs.
pub fn install_pending(_shell: &Shell, done: impl FnOnce(Result<Vec<String>, String>) + 'static) {
    let Some(sync) = glue().and_then(|g| g.sync.clone()) else {
        done(Err("no Copper Cloud".into()));
        return;
    };
    let names: Vec<(String, String)> = sync_status()
        .pending
        .iter()
        .map(|p| (p.id.clone(), format!("{} {}", p.name, p.version)))
        .collect();
    crate::cloud::run_then(
        async move {
            sync.install_pending(Some(&[SLOT]))
                .map_err(|e| e.to_string())
        },
        move |result| {
            done(result.map(|changes| {
                changes
                    .iter()
                    .filter_map(|c| match c {
                        copper_core::addons::Change::Installed { id, .. } => {
                            names.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone())
                        }
                        _ => None,
                    })
                    .collect()
            }))
        },
    );
}

/// `window.__copperAddon.emit(…)` for an event, safe to evaluate.
pub fn emit_js(name: &str, payload: &Value) -> String {
    copper_core::addons::emit_js(name, payload)
}

/// The bridge shim, injected before any addon script.
pub fn bridge_js() -> &'static str {
    BRIDGE_JS
}

/// The addon asked for capability `cap` (`"page"`, …).
pub fn granted(active: &Active, cap: &str) -> bool {
    cap.parse::<Capability>()
        .is_ok_and(|c| active.core.manifest.permissions.grants(c))
}

/// Answer `uri` for the slot's addon, off the main loop; `done` runs on the
/// main thread.
pub fn serve(active: &Active, uri: String, done: impl FnOnce(Served) + 'static) {
    let core = active.core.clone();
    glib::spawn_future_local(async move {
        let served = gio::spawn_blocking(move || {
            let r = copper_core::addons::scheme::serve(&core, &uri);
            Served {
                status: r.status,
                mime: r.mime,
                body: r.body,
                headers: r.headers,
            }
        })
        .await;
        done(served.unwrap_or(Served {
            status: 500,
            mime: "text/plain; charset=utf-8",
            body: b"Internal error".to_vec(),
            headers: Vec::new(),
        }));
    });
}

/// One message from the addon page: the core dispatches it on the cloud
/// runtime with `host`; `done` gets the reply's JS on the main thread
/// (nothing for a message that can't be answered).
pub fn dispatch(active: &Active, text: String, host: Host, done: impl FnOnce(String) + 'static) {
    let Some(g) = glue() else { return };
    let mut ctx = Context::new(
        &active.core,
        Platform::Linux,
        env!("CARGO_PKG_VERSION"),
        g.store.clone(),
    );
    ctx.theme_dark = libadwaita::StyleManager::default().is_dark();
    ctx.fetcher = Some(g.fetcher.clone());
    let host = CoreHost(host);
    crate::cloud::run_then(
        async move { handle_message(&ctx, &host, &text).await.map(|r| r.to_js()) },
        move |js| {
            if let Some(js) = js {
                done(js);
            }
        },
    );
}

/// The core's `BridgeHost`, over this lane's [`Host`].
struct CoreHost(Host);

impl BridgeHost for CoreHost {
    fn identity(&self) -> impl std::future::Future<Output = Option<Identity>> {
        let id = crate::cloud::get().and_then(|c| copper_cloud::addons::identity(&c.client));
        async move { id }
    }

    async fn open_url(&self, url: Url) -> Result<(), BridgeError> {
        if self.0.open_url(url.to_string()).await {
            Ok(())
        } else {
            Err(BridgeError::internal("the window has closed"))
        }
    }

    async fn page_current(&self) -> Option<PageInfo> {
        let v = self.0.page_current().await;
        Some(PageInfo {
            url: v.get("url")?.as_str()?.to_string(),
            title: v
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    async fn close_slot(&self) {
        self.0.close_slot().await;
    }

    async fn oauth_authorize(
        &self,
        url: Url,
        redirect_prefix: String,
        timeout: Duration,
    ) -> Result<String, BridgeError> {
        match self
            .0
            .oauth_authorize(url.to_string(), redirect_prefix, timeout)
            .await
        {
            Outcome::Redirect(u) => Ok(u),
            Outcome::Cancelled => Err(BridgeError::cancelled("the sign-in window was closed")),
            Outcome::Timeout => Err(BridgeError::timeout("the sign-in took too long")),
            Outcome::Failed(e) => Err(BridgeError::internal(e)),
        }
    }

    async fn agent_state(&self) -> Value {
        agent_unavailable_state()
    }

    async fn agent_call(&self, _call: AgentCall) -> Result<Value, BridgeError> {
        Err(BridgeError::unavailable(
            "Copper for Linux has no native agent yet",
        ))
    }
}

/// The self-test's stand-in for the cloud: pack the folder at `dir` in the
/// served format, then verify and stage it exactly as a cloud answer would
/// be (so Settings › Updates shows it and Install installs it).
#[cfg(debug_assertions)]
pub fn selftest_stage(dir: &Path) -> Result<(), String> {
    use copper_core::addons::testkit;
    let sync = glue()
        .and_then(|g| g.sync.clone())
        .ok_or("no addon sync (no cloud client)")?;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map_err(|e| e.to_string())?.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            files.push((
                name,
                std::fs::read(entry.path()).map_err(|e| e.to_string())?,
            ));
        }
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let bytes = testkit::served_bundle(&manifest, &refs);
    let resolved = testkit::resolved_for(&manifest, &bytes);
    let set = copper_core::addons::ResolvedSet::parse(&testkit::resolved_body(
        "r_selftest",
        std::slice::from_ref(&resolved),
    ))
    .map_err(|e| e.to_string())?;
    let outcome = crate::cloud::runtime().block_on(sync.apply_with(set, move |addon| {
        let bytes = bytes.clone();
        async move {
            copper_core::addons::verify_bundle(&bytes, &addon.sha256, &addon)
                .map(|b| (bytes, b))
                .map_err(|e| copper_cloud::addons::bundle_failure(&e))
        }
    }));
    match outcome {
        copper_cloud::addons::CheckOutcome::Applied { staged: 1 } => Ok(()),
        other => Err(format!("{other:?}")),
    }
}
