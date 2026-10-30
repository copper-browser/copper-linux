//! Copper Cloud in the app (Mac: `Fork/Cloud/`): one [`CloudClient`], one
//! [`SyncEngine`] driven through [`adapter::AppBrowser`] (the shell's own
//! state) and [`CloudIntelligence`]. Settings › Cloud and the commands are
//! the app's (`copper-app`'s `cloud::register`, which calls [`start`]).
//!
//! * Everything network-shaped runs on this module's tokio runtime
//!   ([`runtime`], [`spawn`]); the UI main thread never blocks on it — it
//!   awaits [`tokio::task::JoinHandle`]s from main-loop futures.
//! * The engine reads and writes the browser through the adapter, which runs
//!   each call on the main loop and waits for the answer.
//! * Local edits reach the engine as [`SyncEngine::changed`]: a one-second
//!   watch compares the browser's, history's and bookmarks' generations and
//!   the cloud settings values with what was last seen (remote applies mark
//!   themselves seen, so they are never pushed back as local edits). The
//!   same watch hands the engine every change made here to kept tabs and
//!   folders ([`SyncEngine::local_edit`]): its intents, the only changes
//!   this device may publish over the base. A remote apply first hands over
//!   what was edited before it, then marks itself seen.
//! * A spaces publish or apply the engine holds (many tabs unfiled or
//!   removed at once) is asked about with [`crate::view::ShellView::ask`],
//!   drawn the way each shell draws a question.
//! * At launch the client, the engine and the intelligence fetch start when
//!   `cloud.json` holds a link; otherwise when a link is first made. Before
//!   that, before any restored page loads, the engine keys the kept tabs
//!   ([`SyncEngine::remember_kept_tabs`]; the key is kept on the tab, see
//!   [`adapter::SYNC_KEY`]).
//!
//! Other lanes: [`history_cleared`] and [`history_forgot`] tell the cloud
//! that history was cleared / a page forgotten here.
//!
//! Settings › Cloud › Delete history on cloud is [`eraser`]: its words and
//! decisions, toolkit-free; each shell draws the card.
//!
//! Single sign-on ("Continue with Google") and `copper://cloud/join` links
//! are [`sso`] and [`join`]: toolkit-free, each shell only draws them.

pub mod adapter;
pub mod eraser;
pub mod join;
pub mod mcp;
pub mod sso;

use crate::Platform;
use crate::main_loop::MainLoop;
use crate::shell::Shell;
use crate::view::ShellView;
use copper_cloud::{CanvasApi, CloudClient, CloudIntelligence, Domain, SyncEngine};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static CLOUD: OnceLock<Cloud> = OnceLock::new();

thread_local! {
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

/// Run `f` on the cloud runtime. The handle may be awaited from a
/// main-loop future on the main thread.
pub fn spawn<F>(f: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(f)
}

/// Run `f` on the cloud runtime and hand its output to `then` on `L`'s
/// main thread (never blocking it).
pub fn run_then<L, F, T>(f: F, then: impl FnOnce(F::Output) + 'static)
where
    L: MainLoop,
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = spawn(f);
    L::spawn_local(async move {
        if let Ok(out) = handle.await {
            then(out);
        }
    });
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

/// The app's cloud, once [`start`] ran.
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

/// Open the cloud for `shell` (once per process): the client from
/// `cloud.json`, the engine over [`adapter::AppBrowser`], the intelligence
/// fetch and the canvas API; started when a link is made already. Then the
/// one-second watch for local edits. `None`, logged, when `cloud.json`
/// cannot be read: Copper Cloud is off this run, and the app leaves its
/// Settings page and commands out.
pub fn start<P: Platform>(shell: &Shell<P>) -> Option<&'static Cloud> {
    if CLOUD.get().is_none() {
        let client = match CloudClient::open(shell.store().clone()) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "cloud.json unreadable; Copper Cloud is off this run");
                return None;
            }
        };
        let browser: Arc<dyn copper_cloud::BrowserState> =
            Arc::new(adapter::AppBrowser::<P>::new());
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
        // Before any restored page loads (the active tab wakes after the
        // lanes register): every kept tab gets its key in the last spaces
        // document synced, by the address it was restored with, kept on the
        // tab — so a redirect when it wakes never makes it a new entry.
        cloud.sync.remember_kept_tabs();
        let _ = CLOUD.set(cloud);
    }
    let cloud = get()?;
    if cloud.client.is_linked() {
        cloud.start();
    }
    mark_seen(shell);
    watch_local(shell);
    Some(cloud)
}

/// Sync on after a sign-in that came without a choice of switches — a
/// pairing code, single sign-on: every switch, or the ones this device had
/// before if it was signed in once. Through the window's own engine (the
/// running app owns the profile).
pub async fn sync_after_sign_in(cloud: &Cloud) {
    let before = cloud.sync.enabled();
    let domains: Vec<Domain> = if before.is_empty() {
        Domain::ALL.to_vec()
    } else {
        before.into_iter().collect()
    };
    cloud.sync.turn_on(&domains).await;
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
    /// The kept tabs and the folder ids, for the intents.
    kept: Vec<copper_cloud::sync::LiveKept>,
    folders: Vec<String>,
}

fn snapshot<P: Platform>(shell: &Shell<P>) -> Seen {
    Seen {
        browser: shell.browser().generation(),
        history: shell.history().generation(),
        bookmarks: shell.bookmarks().generation(),
        settings: adapter::cloud_settings(shell),
        kept: adapter::kept(shell),
        folders: crate::agents::groups::all(shell)
            .into_iter()
            .map(|g| g.id)
            .collect(),
    }
}

/// Everything as it is now counts as seen: a remote apply is not a local edit.
pub fn mark_seen<P: Platform>(shell: &Shell<P>) {
    let now = snapshot(shell);
    SEEN.with(|s| *s.borrow_mut() = Some(now));
}

/// Before a remote apply: whatever was changed here since the last look
/// goes to the engine as intents first, so the apply's own marking-seen
/// never swallows an edit made here.
pub(crate) fn take_local_edits<P: Platform>(shell: &Shell<P>) {
    let now = snapshot(shell);
    let before = SEEN.with(|s| s.borrow_mut().replace(now.clone()));
    if let (Some(before), Some(cloud)) = (before, get()) {
        intents(cloud, &before, &now);
    }
}

fn intents(cloud: &Cloud, before: &Seen, now: &Seen) {
    if before.kept != now.kept || before.folders != now.folders {
        cloud
            .sync
            .local_edit(&before.kept, &now.kept, &before.folders, &now.folders);
    }
}

/// The person just confirmed a bulk change to kept tabs (deleting a space
/// with its tabs): its publish goes through without being held.
pub fn confirmed() {
    if let Some(c) = get() {
        c.sync.confirmed();
    }
}

fn watch_local<P: Platform>(shell: &Shell<P>) {
    let weak = shell.downgrade();
    P::Loop::spawn_local(async move {
        loop {
            P::Loop::sleep(Duration::from_millis(1000)).await;
            let Some(shell) = weak.upgrade() else { break };
            poll_local(&shell);
        }
    });
}

fn poll_local<P: Platform>(shell: &Shell<P>) {
    let now = snapshot(shell);
    let before = SEEN.with(|s| s.borrow_mut().replace(now.clone()));
    let Some(before) = before else { return };
    let Some(cloud) = get() else { return };
    // Recorded signed in or not: an edit made offline still goes up later.
    intents(cloud, &before, &now);
    ask_held(shell, cloud);
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

// MARK: held

thread_local! {
    /// The hold last asked about, and whether its question is still up.
    static ASKED: RefCell<(Option<copper_cloud::sync::Held>, bool)> = const { RefCell::new((None, false)) };
}

/// A held spaces publish or apply: ask once (a new question only once the
/// last is answered and the hold changed), and hand the answer back.
fn ask_held<P: Platform>(shell: &Shell<P>, cloud: &Cloud) {
    let held = cloud.sync.held();
    let ask = ASKED.with(|a| {
        let mut a = a.borrow_mut();
        if a.1 || a.0 == held {
            return false;
        }
        a.0 = held;
        a.1 = held.is_some();
        a.1
    });
    let Some(held) = held.filter(|_| ask) else {
        return;
    };
    let (allow, deny) = held.buttons();
    let question = crate::view::Question {
        title: held.title(),
        detail: Some(held.detail().to_string()),
        deny: deny.to_string(),
        allow: allow.to_string(),
    };
    shell.view().ask(
        shell,
        question,
        Box::new(|go| {
            ASKED.with(|a| a.borrow_mut().1 = false);
            if let Some(c) = get() {
                let sync = c.sync.clone();
                spawn(async move { sync.resolve_hold(go).await });
            }
        }),
    );
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
