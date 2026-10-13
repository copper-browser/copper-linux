//! Copper Cloud in the app (Mac: `Fork/Cloud/`): one [`CloudClient`], one
//! [`SyncEngine`] driven through [`adapter::AppBrowser`] (the shell's own
//! state), [`CloudIntelligence`], and Settings › Cloud ([`page`]).
//!
//! * Everything network-shaped runs on this module's tokio runtime
//!   ([`runtime`], [`spawn`]); the GTK main thread never blocks on it — it
//!   awaits [`tokio::task::JoinHandle`]s from `glib` futures.
//! * The engine reads and writes the browser through the adapter, which runs
//!   each call on the main loop and waits for the answer.
//! * Local edits reach the engine as [`SyncEngine::changed`]: a one-second
//!   watch compares the browser's, history's and bookmarks' generations and
//!   the cloud settings values with what was last seen (remote applies mark
//!   themselves seen, so they are never pushed back as local edits).
//! * At launch the client, the engine and the intelligence fetch start when
//!   `cloud.json` holds a link; otherwise when a link is first made.
//!
//! Other lanes: [`history_cleared`] and [`history_forgot`] tell the cloud
//! that history was cleared / a page forgotten here.

pub mod adapter;
pub mod bench;
pub mod mcp;
pub mod page;
pub mod selftest;

use crate::actions::{Command, SettingsPage, register_command, register_settings_page};
use crate::shell::{Shell, WeakShell};
use copper_cloud::{CanvasApi, CloudClient, CloudIntelligence, Domain, SyncEngine};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static CLOUD: OnceLock<Cloud> = OnceLock::new();

thread_local! {
    static SHELL: RefCell<Option<WeakShell>> = const { RefCell::new(None) };
    static SEEN: RefCell<Option<Seen>> = const { RefCell::new(None) };
}

/// The cloud's tokio runtime (two workers; started on first use).
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("copper-cloud")
            .enable_all()
            .build()
            .expect("the cloud's tokio runtime")
    })
}

/// Run `f` on the cloud runtime. The handle may be awaited from a `glib`
/// future on the main thread.
pub fn spawn<F>(f: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(f)
}

/// Run `f` on the cloud runtime and hand its output to `then` on the main
/// thread (never blocking it).
pub fn run_then<F, T>(f: F, then: impl FnOnce(F::Output) + 'static)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = spawn(f);
    glib::spawn_future_local(async move {
        if let Ok(out) = handle.await {
            then(out);
        }
    });
}

/// The shell, on the main thread, while it lives.
pub(crate) fn shell() -> Option<Shell> {
    SHELL.with(|s| s.borrow().as_ref().and_then(WeakShell::upgrade))
}

/// The app's cloud: the client, the sync engine, the intelligence fetch and
/// the canvas REST calls, all on the same link and account.
pub struct Cloud {
    pub client: CloudClient,
    pub sync: SyncEngine,
    pub intelligence: CloudIntelligence,
    pub canvas: CanvasApi,
    started: AtomicBool,
}

impl Cloud {
    /// Start the event stream, the engine and the intelligence fetch (once).
    pub fn start(&self) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let _rt = runtime().enter();
        self.client.start();
        self.sync.start();
        self.intelligence.start();
        tracing::info!(target: "copper_app::cloud", "cloud started");
    }

    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Something of `domain` changed here.
    pub fn changed(&self, domain: Domain) {
        let _rt = runtime().enter();
        self.sync.changed(domain);
    }
}

/// The app's cloud, once [`register`] ran.
pub fn get() -> Option<&'static Cloud> {
    CLOUD.get()
}

/// History was cleared here: everything on the cloud too, while history syncs.
pub fn history_cleared() {
    if let Some(c) = get() {
        let _rt = runtime().enter();
        c.sync.history_cleared();
    }
}

/// A page (its history key) was forgotten here: its rows on the cloud too.
pub fn history_forgot(key: &str) {
    if let Some(c) = get() {
        let _rt = runtime().enter();
        c.sync.history_forgot(key);
    }
}

pub fn register(shell: &Shell) {
    SHELL.with(|s| *s.borrow_mut() = Some(shell.downgrade()));
    if CLOUD.get().is_none() {
        let client = match CloudClient::open(shell.store().clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "cloud.json unreadable; Copper Cloud is off this run");
                return;
            }
        };
        let browser: Arc<dyn copper_cloud::BrowserState> = Arc::new(adapter::AppBrowser);
        let cloud = {
            let _rt = runtime().enter();
            Cloud {
                sync: SyncEngine::new(client.clone(), browser),
                intelligence: CloudIntelligence::new(client.clone()),
                canvas: CanvasApi::new(client.clone()),
                client,
                started: AtomicBool::new(false),
            }
        };
        let _ = CLOUD.set(cloud);
    }
    let Some(cloud) = get() else { return };
    if cloud.client.is_linked() {
        cloud.start();
    }
    mark_seen(shell);
    watch_local(shell);

    register_settings_page(
        shell,
        SettingsPage {
            id: "cloud".into(),
            title: "Cloud".into(),
            icon: "network-server-symbolic".into(),
            build: Rc::new(page::build),
        },
    );
    register_command(
        shell,
        Command::new(
            "copper-cloud",
            "Copper Cloud…",
            "network-server-symbolic",
            |s| {
                open_settings(s);
            },
        ),
    );
    register_command(
        shell,
        Command::new(
            "pair-device",
            "Pair another device with Copper Cloud",
            "phone-symbolic",
            |s| {
                if let Some(p) = open_settings(s) {
                    p.make_code();
                }
            },
        )
        .when(|_| get().is_some_and(|c| c.client.is_signed_in())),
    );
    register_command(
        shell,
        Command::new(
            "cloud-sync-now",
            "Sync Now",
            "emblem-synchronizing-symbolic",
            |s| {
                sync_now();
                s.toast("Syncing with Copper Cloud");
            },
        )
        .when(|_| get().is_some_and(|c| c.client.is_signed_in() && c.sync.is_on())),
    );
    bench::start_if_asked(shell);
    selftest::maybe_start(shell);
}

/// Settings, on the Cloud page. The page, once built.
pub fn open_settings(shell: &Shell) -> Option<Rc<page::CloudPage>> {
    use libadwaita::prelude::*;
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name("cloud");
    page::current()
}

/// "Sync now".
pub fn sync_now() {
    if let Some(c) = get() {
        let sync = c.sync.clone();
        spawn(async move { sync.sync_now().await });
    }
}

// MARK: local changes

/// What the watch last saw.
#[derive(Clone)]
struct Seen {
    browser: u64,
    history: u64,
    bookmarks: u64,
    settings: BTreeMap<String, String>,
}

fn snapshot(shell: &Shell) -> Seen {
    Seen {
        browser: shell.browser().generation(),
        history: shell.history().generation(),
        bookmarks: shell.bookmarks().generation(),
        settings: adapter::cloud_settings(shell),
    }
}

/// Everything as it is now counts as seen: a remote apply is not a local edit.
pub(crate) fn mark_seen(shell: &Shell) {
    let now = snapshot(shell);
    SEEN.with(|s| *s.borrow_mut() = Some(now));
}

fn watch_local(shell: &Shell) {
    let weak = shell.downgrade();
    glib::timeout_add_local(Duration::from_millis(1000), move || {
        let Some(shell) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        poll_local(&shell);
        glib::ControlFlow::Continue
    });
}

fn poll_local(shell: &Shell) {
    let now = snapshot(shell);
    let before = SEEN.with(|s| s.borrow_mut().replace(now.clone()));
    let Some(before) = before else { return };
    let Some(cloud) = get() else { return };
    if !cloud.client.is_signed_in() || !cloud.sync.is_on() {
        return;
    }
    if now.browser != before.browser {
        cloud.changed(Domain::Spaces);
        cloud.changed(Domain::Tabs);
    }
    if now.history != before.history {
        cloud.changed(Domain::History);
    }
    if now.bookmarks != before.bookmarks {
        cloud.changed(Domain::Bookmarks);
    }
    if now.settings != before.settings {
        cloud.changed(Domain::Settings);
    }
}

// MARK: words

/// "just now", "3 minutes ago", "2 hours ago", "4 days ago".
pub fn ago(unix: f64) -> String {
    let secs = (copper_core::date::now_unix() - unix).max(0.0);
    let (n, unit) = if secs < 45.0 {
        return "just now".into();
    } else if secs < 3600.0 {
        ((secs / 60.0).round().max(1.0), "minute")
    } else if secs < 86_400.0 {
        ((secs / 3600.0).round(), "hour")
    } else {
        ((secs / 86_400.0).round(), "day")
    };
    let n = n as i64;
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// "1 tab", "3 tabs".
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        let now = copper_core::date::now_unix();
        assert_eq!(ago(now - 5.0), "just now");
        assert_eq!(ago(now - 60.0), "1 minute ago");
        assert_eq!(ago(now - 600.0), "10 minutes ago");
        assert_eq!(ago(now - 7200.0), "2 hours ago");
        assert_eq!(ago(now - 86_400.0 * 3.0), "3 days ago");
        assert_eq!(count(1, "tab", "tabs"), "1 tab");
        assert_eq!(count(2, "tab", "tabs"), "2 tabs");
    }
}
