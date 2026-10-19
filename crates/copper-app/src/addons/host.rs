//! What the bridge asks of Copper itself (Mirrors the Mac's
//! `Fork/Addons/AddonBridge.swift`, its `BridgeHost` side).
//!
//! The core's `dispatch` runs on the cloud's tokio runtime and does the
//! platform-neutral methods itself (`info`, `settings`, `storage.*`,
//! `fetch`, capability checks). For the rest it calls back into a host; this
//! one is `Send + Sync`, and every call that touches the window hops to the
//! GTK main thread and waits for the answer there, so nothing ever blocks
//! the main loop.
//!
//! | Bridge | Here |
//! |---|---|
//! | `identity()` | the Copper Cloud account (`null` when not signed in; `copper_cloud::addons::identity`) |
//! | `openUrl(url)` | a new foreground tab |
//! | `page.current()` | the active tab's `{url, title}` (`null` for private tabs) |
//! | `ui.close()` | hides the slot |
//! | `oauth.authorize(…)` | [`super::oauth`] |
//! | `agent.*` | unavailable: Linux has no native agent yet |

use super::core_glue::Active;
use super::oauth::Outcome;
use crate::shell::Shell;
use serde_json::Value;
use std::time::Duration;

/// The host for one addon's calls.
#[derive(Debug, Clone)]
pub struct Host {
    pub addon_id: String,
    pub addon_name: String,
}

impl Host {
    pub fn new(active: &Active) -> Host {
        Host {
            addon_id: active.id.clone(),
            addon_name: active.name.clone(),
        }
    }

    /// `copper.openUrl(url)`: `false` if the window is gone.
    pub async fn open_url(&self, url: String) -> bool {
        on_main(move |s| super::view::open_tab(s, &url))
            .await
            .is_some()
    }

    /// `copper.page.current()`.
    pub async fn page_current(&self) -> Value {
        on_main(super::view::current_page)
            .await
            .unwrap_or(Value::Null)
    }

    /// `copper.ui.close()`.
    pub async fn close_slot(&self) {
        on_main(super::hide).await;
    }

    /// `copper.oauth.authorize(…)`. The core has already checked the
    /// capability, the host of `url` and the form of `redirect_prefix`.
    pub async fn oauth_authorize(
        &self,
        url: String,
        redirect_prefix: String,
        timeout: Duration,
    ) -> Outcome {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let name = self.addon_name.clone();
        glib::MainContext::default().invoke(move || {
            let Some(s) = super::shell() else {
                let _ = tx.send(Outcome::Failed("the window has closed".into()));
                return;
            };
            super::oauth::authorize(&s, &name, &url, &redirect_prefix, timeout, move |o| {
                let _ = tx.send(o);
            });
        });
        rx.await.unwrap_or(Outcome::Cancelled)
    }
}

/// Run `f` with the shell on the GTK main thread; `None` if the window is
/// gone. Callable from any thread.
pub async fn on_main<T: Send + 'static>(f: impl FnOnce(&Shell) -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    glib::MainContext::default().invoke(move || {
        if let Some(s) = super::shell() {
            let _ = tx.send(f(&s));
        }
    });
    rx.await.ok()
}
