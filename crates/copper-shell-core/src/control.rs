//! The internal command API: everything an automation layer (the MCP
//! server, the self-test) needs to drive the shell, callable from any
//! thread.
//!
//! The shell lives on the UI main thread and is `!Send`; a [`Control`] is
//! a `Send + Sync + Clone` handle that runs closures *there* (through the
//! platform's [`MainLoop::post`]) and hands the result back:
//!
//! ```ignore
//! let control = copper_app::control::handle().expect("app running");
//! // From a Tokio task, a thread, or the main loop itself:
//! let tabs = control.tabs().await?;
//! let id = control.open("example.com", OpenRequest::foreground()).await?;
//! control.wait_loaded(&id, Duration::from_secs(10)).await?;
//! let title = control
//!     .with_page_async(&id, |page| async move {
//!         page.eval("document.title", &World::Page).await
//!     })
//!     .await??;
//! let mut events = control.subscribe(); // tokio broadcast of ShellEvent
//! ```
//!
//! * Every call queues onto the main loop (never runs re-entrantly inside
//!   a caller already on the main thread) and resolves once it has run.
//! * Values crossing threads are plain data ([`TabInfo`], [`SpaceInfo`],
//!   [`ShellEvent`]); the page itself (the engine's page, `!Send`) is only
//!   ever touched inside [`Control::with_page`] / [`Control::with_page_async`].
//! * [`ControlError::Gone`] means the window has closed.
//!
//! One shell per process: [`install`] makes it the one every [`handle`]
//! drives. The handle and the shell are kept type-erased (the statics
//! cannot be generic) and handed back for the platform they were
//! installed for.

use crate::main_loop::MainLoop;
use crate::shell::{OpenTab, Shell, WeakShell};
use crate::view::ShellView;
use crate::{PageOf, Platform};
use copper_core::TabId;
use copper_engine::PageView;
use serde::Serialize;
use std::any::Any;
use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, oneshot};

/// Something the shell did, for watchers (`Send`, cheap to clone).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum ShellEvent {
    TabOpened(#[serde(serialize_with = "ser_id")] TabId),
    TabClosed(#[serde(serialize_with = "ser_id")] TabId),
    /// Title, address, sleep, pin… changed.
    TabChanged(#[serde(serialize_with = "ser_id")] TabId),
    Selected(#[serde(serialize_with = "ser_opt_id")] Option<TabId>),
    SpaceSwitched(String),
    SpacesChanged,
    /// A page finished loading.
    Loaded(#[serde(serialize_with = "ser_id")] TabId),
    /// A page's web process went away.
    Crashed(#[serde(serialize_with = "ser_id")] TabId),
    /// Tabs moved between the column and the row.
    LayoutChanged,
}

fn ser_id<S: serde::Serializer>(id: &TabId, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_str(&id.0)
}

fn ser_opt_id<S: serde::Serializer>(
    id: &Option<TabId>,
    s: S,
) -> std::result::Result<S::Ok, S::Error> {
    match id {
        Some(id) => s.serialize_some(&id.0),
        None => s.serialize_none(),
    }
}

/// The broadcast hub the shell sends [`ShellEvent`]s through.
pub struct Hub {
    tx: broadcast::Sender<ShellEvent>,
}

impl Default for Hub {
    fn default() -> Self {
        Hub {
            tx: broadcast::channel(512).0,
        }
    }
}

impl Hub {
    pub fn send(&self, e: ShellEvent) {
        // No receivers is fine.
        let _ = self.tx.send(e);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ShellEvent> {
        self.tx.subscribe()
    }

    pub(crate) fn sender(&self) -> broadcast::Sender<ShellEvent> {
        self.tx.clone()
    }
}

/// Why a control call did not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    #[error("the browser window has closed")]
    Gone,
    #[error("no tab {0}")]
    NoTab(String),
    #[error("no space {0}")]
    NoSpace(String),
    #[error("nothing to open for {0:?}")]
    BadAddress(String),
    #[error("timed out after {0:?}")]
    Timeout(Duration),
}

pub type Result<T> = std::result::Result<T, ControlError>;

/// One tab, as automation sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabInfo {
    #[serde(serialize_with = "ser_id")]
    pub id: TabId,
    /// Position in the current space's visible list (pins first), as
    /// `Ctrl+1…9` count it; `None` for tabs in other spaces.
    pub index: Option<usize>,
    pub title: String,
    pub url: String,
    pub space: String,
    pub pinned: bool,
    /// A pin's letter.
    pub pin: Option<String>,
    /// Kept (Saved) rather than today's.
    pub saved: bool,
    pub sleeping: bool,
    pub private: bool,
    pub active: bool,
    /// Has a live page (a WebView).
    pub live: bool,
    pub loading: bool,
    pub crashed: bool,
}

/// One space.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpaceInfo {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// 0…1, `None` for grey.
    pub hue: Option<f64>,
    /// The palette name nearest its colour (`Copper`, `Teal`, …).
    pub colour: String,
    /// Its own cookie jar, if any.
    pub profile: Option<String>,
    pub current: bool,
    /// Loose tabs in it (pins are in every space).
    pub tabs: usize,
}

/// How to open a tab from automation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenRequest {
    pub foreground: bool,
    pub private: bool,
    /// A space id (default: the current one).
    pub space: Option<String>,
}

impl OpenRequest {
    pub fn foreground() -> Self {
        OpenRequest {
            foreground: true,
            ..Default::default()
        }
    }

    pub fn background() -> Self {
        OpenRequest::default()
    }
}

thread_local! {
    /// The installed shell's `WeakShell<P>` (main thread).
    static CURRENT: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

/// The installed shell's `Control<P>`.
static HANDLE: OnceLock<Box<dyn Any + Send + Sync>> = OnceLock::new();

/// The running app's control handle (set once the window is up); `None`
/// before that, or when asked for another platform than the one installed.
pub fn handle<P: Platform>() -> Option<Control<P>> {
    HANDLE.get()?.downcast_ref::<Control<P>>().cloned()
}

/// Make `shell` the one [`Control`] drives. Main thread, once.
pub fn install<P: Platform>(shell: &Shell<P>) -> Control<P> {
    CURRENT.with(|c| *c.borrow_mut() = Some(Box::new(shell.downgrade())));
    let control = Control {
        events: shell.hub().sender(),
        platform: PhantomData,
    };
    let _ = HANDLE.set(Box::new(control.clone()));
    control
}

/// The shell, when called on the main thread while it is alive.
pub fn current<P: Platform>() -> Option<Shell<P>> {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()?
            .downcast_ref::<WeakShell<P>>()?
            .upgrade()
    })
}

/// A handle that drives the shell from any thread.
pub struct Control<P: Platform> {
    events: broadcast::Sender<ShellEvent>,
    /// `fn() -> P`: `Send + Sync` whatever `P` is (no `P` is held).
    platform: PhantomData<fn() -> P>,
}

impl<P: Platform> Clone for Control<P> {
    fn clone(&self) -> Self {
        Control {
            events: self.events.clone(),
            platform: PhantomData,
        }
    }
}

impl<P: Platform> std::fmt::Debug for Control<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Control")
    }
}

impl<P: Platform> Control<P> {
    /// Every [`ShellEvent`] from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<ShellEvent> {
        self.events.subscribe()
    }

    /// Run `f` on the main thread with the shell; its result comes back.
    pub async fn run<R, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&Shell<P>) -> R + Send + 'static,
    {
        self.run_async(move |shell| async move { f(&shell) }).await
    }

    /// Run an async block on the main thread (it may await engine futures —
    /// `evaluate`, `snapshot` — and timers); its output comes back.
    pub async fn run_async<R, Fut, F>(&self, f: F) -> Result<R>
    where
        R: Send + 'static,
        Fut: Future<Output = R> + 'static,
        F: FnOnce(Shell<P>) -> Fut + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        P::Loop::post(move || {
            // Always a fresh task: never inside a caller's borrow.
            P::Loop::spawn_local(async move {
                match current::<P>() {
                    Some(shell) => {
                        let out = f(shell).await;
                        let _ = tx.send(Ok(out));
                    }
                    None => {
                        let _ = tx.send(Err(ControlError::Gone));
                    }
                }
            });
        });
        rx.await.unwrap_or(Err(ControlError::Gone))
    }

    // MARK: reading

    /// Every tab: the current space's visible list first (in order), then
    /// the other spaces' rows.
    pub async fn tabs(&self) -> Result<Vec<TabInfo>> {
        self.run(tab_infos).await
    }

    /// One tab.
    pub async fn tab(&self, id: &TabId) -> Result<TabInfo> {
        let id = id.clone();
        self.run(move |s| {
            tab_infos(s)
                .into_iter()
                .find(|t| t.id == id)
                .ok_or_else(|| ControlError::NoTab(id.to_string()))
        })
        .await?
    }

    /// The selected tab.
    pub async fn active(&self) -> Result<Option<TabId>> {
        self.run(|s| s.active_id()).await
    }

    /// Every space, in order.
    pub async fn spaces(&self) -> Result<Vec<SpaceInfo>> {
        self.run(space_infos).await
    }

    // MARK: tabs

    /// Open `address` (an address or words to search for) in a new tab.
    pub async fn open(&self, address: &str, how: OpenRequest) -> Result<TabId> {
        let address = address.to_string();
        self.run(move |s| {
            let url = if address.is_empty() || address == "about:blank" {
                "about:blank".to_string()
            } else {
                s.destination(&address)
                    .ok_or_else(|| ControlError::BadAddress(address.clone()))?
            };
            if let Some(space) = &how.space
                && s.browser().space(space).is_none()
            {
                return Err(ControlError::NoSpace(space.clone()));
            }
            Ok(s.open(
                &url,
                OpenTab {
                    foreground: how.foreground,
                    private: how.private,
                    space: how.space.clone(),
                    ..Default::default()
                },
            ))
        })
        .await?
    }

    pub async fn select(&self, id: &TabId) -> Result<()> {
        let id = id.clone();
        self.run(move |s| {
            if s.select(&id) {
                Ok(())
            } else {
                Err(ControlError::NoTab(id.to_string()))
            }
        })
        .await?
    }

    /// Close a tab (a pin for good); a Saved row is only unloaded, and
    /// stays saved (Arc).
    pub async fn close(&self, id: &TabId) -> Result<()> {
        let id = id.clone();
        self.run(move |s| {
            if s.browser().tab(&id).is_none() {
                return Err(ControlError::NoTab(id.to_string()));
            }
            s.close_keeping_saved(&id);
            Ok(())
        })
        .await?
    }

    /// Load `address` in a tab (waking it).
    pub async fn navigate(&self, id: &TabId, address: &str) -> Result<()> {
        let (id, address) = (id.clone(), address.to_string());
        self.run(move |s| {
            if s.browser().tab(&id).is_none() {
                return Err(ControlError::NoTab(id.to_string()));
            }
            let url = s
                .destination(&address)
                .ok_or_else(|| ControlError::BadAddress(address.clone()))?;
            s.navigate(&id, &url);
            Ok(())
        })
        .await?
    }

    pub async fn reload(&self, id: &TabId) -> Result<()> {
        let id = id.clone();
        self.run(move |s| s.reload(&id)).await
    }

    pub async fn back(&self, id: &TabId) -> Result<()> {
        self.with_page(id, |p| p.back()).await
    }

    pub async fn forward(&self, id: &TabId) -> Result<()> {
        self.with_page(id, |p| p.forward()).await
    }

    pub async fn pin(&self, id: &TabId, pinned: bool) -> Result<()> {
        let id = id.clone();
        self.run(move |s| {
            if pinned {
                s.pin(&id);
            } else {
                s.unpin(&id);
            }
        })
        .await
    }

    /// Move a tab within its list (pins among pins, row tabs in the row).
    pub async fn move_tab(&self, id: &TabId, index: usize) -> Result<()> {
        let id = id.clone();
        self.run(move |s| {
            s.move_tab(&id, index);
        })
        .await
    }

    /// Bring back the most recently closed tab.
    pub async fn reopen(&self) -> Result<Option<TabId>> {
        self.run(|s| s.reopen()).await
    }

    /// Wait until a tab's page has finished loading (waking it if needed).
    pub async fn wait_loaded(&self, id: &TabId, timeout: Duration) -> Result<()> {
        let id = id.clone();
        self.run_async(move |s| async move {
            let start = Instant::now();
            s.ensure_page(&id);
            loop {
                let Some(slot) = s.slot(&id) else {
                    return Err(ControlError::NoTab(id.to_string()));
                };
                let started = !slot.page.url().unwrap_or_default().is_empty();
                if started && !slot.loading.get() && !slot.page.is_loading() {
                    return Ok(());
                }
                if start.elapsed() >= timeout {
                    return Err(ControlError::Timeout(timeout));
                }
                P::Loop::sleep(Duration::from_millis(40)).await;
            }
        })
        .await?
    }

    // MARK: pages

    /// Run `f` with the tab's live page (built if the tab was asleep).
    pub async fn with_page<R, F>(&self, id: &TabId, f: F) -> Result<R>
    where
        R: Send + 'static,
        F: FnOnce(&PageOf<P>) -> R + Send + 'static,
    {
        let id = id.clone();
        self.run(move |s| {
            let page = s
                .ensure_page(&id)
                .ok_or_else(|| ControlError::NoTab(id.to_string()))?;
            Ok(f(&page))
        })
        .await?
    }

    /// Run an async block with the tab's live page (built if asleep) —
    /// `evaluate`, `snapshot`, `find`…
    pub async fn with_page_async<R, Fut, F>(&self, id: &TabId, f: F) -> Result<R>
    where
        R: Send + 'static,
        Fut: Future<Output = R> + 'static,
        F: FnOnce(PageOf<P>) -> Fut + Send + 'static,
    {
        let id = id.clone();
        self.run_async(move |s| async move {
            match s.ensure_page(&id) {
                Some(page) => Ok(f(page).await),
                None => Err(ControlError::NoTab(id.to_string())),
            }
        })
        .await?
    }

    // MARK: spaces

    /// A new space (made current); its id.
    pub async fn add_space(&self, name: &str) -> Result<String> {
        let name = name.to_string();
        self.run(move |s| s.add_space(&name)).await
    }

    pub async fn switch_space(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.run(move |s| {
            if s.switch_space(&id) {
                Ok(())
            } else {
                Err(ControlError::NoSpace(id.clone()))
            }
        })
        .await?
    }

    // MARK: the window

    /// Tabs in the column (`true`) or the row.
    pub async fn set_tabs_left(&self, left: bool) -> Result<()> {
        self.run(move |s| {
            s.set_tabs_position(if left {
                copper_core::settings::TabsPosition::Left
            } else {
                copper_core::settings::TabsPosition::Top
            })
        })
        .await
    }

    /// Light, dark or system.
    pub async fn set_look(&self, look: copper_core::settings::Look) -> Result<()> {
        self.run(move |s| s.set_look(look)).await
    }

    /// A PNG of the whole window.
    pub async fn screenshot(&self) -> Result<Option<Vec<u8>>> {
        self.run(|s| s.view().screenshot()).await
    }

    /// Write everything to disk now.
    pub async fn save(&self) -> Result<()> {
        self.run(|s| s.save_now()).await
    }
}

/// Tabs as [`Control::tabs`] lists them.
pub fn tab_infos<P: Platform>(s: &Shell<P>) -> Vec<TabInfo> {
    let visible = s.visible_ids();
    let b = s.browser();
    let active = b.active_id();
    let info = |t: &copper_core::Tab| {
        let slot = s.slot(&t.id);
        TabInfo {
            id: t.id.clone(),
            index: visible.iter().position(|v| *v == t.id),
            title: t.title.clone(),
            url: t.url.clone(),
            space: t.space.clone(),
            pinned: t.is_pinned(),
            pin: t.pin.clone(),
            saved: t.saved,
            sleeping: t.sleeping,
            private: t.private,
            active: active.as_ref() == Some(&t.id),
            live: slot.is_some(),
            loading: slot.as_ref().is_some_and(|x| x.loading.get()),
            crashed: slot.as_ref().is_some_and(|x| x.crashed.get()),
        }
    };
    let mut out: Vec<TabInfo> = visible
        .iter()
        .filter_map(|id| b.tab(id))
        .map(info)
        .collect();
    for space in b.spaces() {
        if space.id == b.current_space_id() {
            continue;
        }
        out.extend(b.row(&space.id).into_iter().map(info));
    }
    out
}

/// Spaces as [`Control::spaces`] lists them.
pub fn space_infos<P: Platform>(s: &Shell<P>) -> Vec<SpaceInfo> {
    let b = s.browser();
    b.spaces()
        .map(|sp| SpaceInfo {
            id: sp.id.clone(),
            name: sp.name.clone(),
            icon: sp.icon.clone(),
            hue: sp.hue,
            colour: crate::spaces::colour::SpaceColour::of(sp)
                .name()
                .to_string(),
            profile: sp.profile.clone(),
            current: sp.id == b.current_space_id(),
            tabs: b.row(&sp.id).len(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_for_automation() {
        let id = TabId::new();
        let json = serde_json::to_value(ShellEvent::Selected(Some(id.clone()))).unwrap();
        assert_eq!(json["event"], "selected");
        assert_eq!(json["data"], id.to_string());
        let json = serde_json::to_value(ShellEvent::SpacesChanged).unwrap();
        assert_eq!(json["event"], "spaces_changed");
    }

    #[test]
    fn hub_broadcasts() {
        let hub = Hub::default();
        hub.send(ShellEvent::LayoutChanged); // nobody listening: fine
        let mut rx = hub.subscribe();
        hub.send(ShellEvent::SpacesChanged);
        assert_eq!(rx.try_recv().unwrap(), ShellEvent::SpacesChanged);
    }

    /// Checked for every platform: `Control<P>` holds no `P`.
    #[allow(dead_code)]
    fn control_is_send_and_sync_for<P: Platform>() {
        fn check<T: Send + Sync + Clone>() {}
        check::<Control<P>>();
    }

    #[test]
    fn control_handle_is_send_and_sync() {
        fn check<T: Send + Sync + Clone>() {}
        check::<TabInfo>();
        check::<SpaceInfo>();
    }
}
