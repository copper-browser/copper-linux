//! Page/profile events and the multi-subscriber stream that carries them.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use serde::{Deserialize, Serialize};

use crate::{DownloadId, Image, JsonValue, PermissionId};

/// Something that happened in a page. Every variant is plain data (`Send`),
/// so a stream can be handed to a worker thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageEvent {
    /// Document title changed (`None` while there is no title). WebKit may
    /// deliver it a few milliseconds after `Load(Finished)`.
    Title(Option<String>),
    /// Active URL changed (includes same-document navigations).
    Url(Option<String>),
    /// Favicon changed; `None` when the page has none.
    Favicon(Option<Image>),
    /// Estimated load progress, 0.0 to 1.0.
    Progress(f64),
    /// Main-frame load state machine.
    Load(LoadState),
    /// The page asked for a new window (`window.open`, `target=_blank`,
    /// or a middle/Ctrl-click on a link) and no new-window handler took it.
    NewWindow(NewWindowRequest),
    /// A download started by this page changed state.
    Download(DownloadEvent),
    /// `console.*` call or uncaught error (only with `PageOptions::capture_console`).
    Console(ConsoleMessage),
    /// A script posted to a registered message handler
    /// (`window.webkit.messageHandlers.<name>.postMessage(body)`).
    ScriptMessage { name: String, body: JsonValue },
    /// The web content process went away; the page is blank until reloaded.
    Crashed(CrashReason),
    /// The page wants a permission. Answer with `PageView::answer_permission`;
    /// unanswered requests stay pending until the page navigates.
    PermissionRequest {
        id: PermissionId,
        kind: PermissionKind,
        /// Page URL when the request was made.
        url: Option<String>,
    },
    /// Script called `window.close()` on a page that may be closed.
    CloseRequested,
    /// The pointer moved onto (`Some`) or off (`None`) a link.
    LinkHover(Option<LinkHover>),
    /// The page started or stopped playing audio.
    AudioPlaying(bool),
}

/// Main-frame load progress.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadState {
    Started {
        url: Option<String>,
    },
    Redirected {
        url: Option<String>,
    },
    /// First bytes of the new document arrived; the URL is now final.
    Committed {
        url: Option<String>,
    },
    /// Load finished (also emitted after `Failed`).
    Finished {
        url: Option<String>,
    },
    Failed {
        url: String,
        error: String,
        /// The load was stopped or replaced, not broken.
        cancelled: bool,
    },
}

/// Why a new window was requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowDisposition {
    /// `window.open` / `target=_blank`: the page may keep a handle (opener).
    Popup,
    /// Middle-click or Ctrl/Meta-click on a link: open in a new tab, no opener.
    NewTab,
}

/// Details of a new-window request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewWindowRequest {
    /// Target URL; `None` or `about:blank` for script-filled popups.
    pub url: Option<String>,
    pub user_gesture: bool,
    /// `window.open` target name, if any.
    pub frame_name: Option<String>,
    pub disposition: WindowDisposition,
}

/// Link under the pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkHover {
    pub url: String,
    pub title: Option<String>,
    pub label: Option<String>,
}

/// Why the web process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashReason {
    Crashed,
    ExceededMemoryLimit,
    /// `PageView::terminate_web_process` was called.
    TerminatedByApi,
}

/// What a permission request is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionKind {
    Geolocation,
    Notifications,
    Camera,
    Microphone,
    CameraAndMicrophone,
    DisplayCapture,
    Clipboard,
    DeviceInfo,
    PointerLock,
    MediaKeySystem,
    /// Storage access for a third-party frame (`document.requestStorageAccess`).
    WebsiteDataAccess {
        requesting_domain: Option<String>,
        current_domain: Option<String>,
    },
    Xr,
    /// A request type this crate does not know; the engine's type name.
    Other(String),
}

/// A console line from the page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleMessage {
    pub level: ConsoleLevel,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

/// A download state change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DownloadEvent {
    pub id: DownloadId,
    pub state: DownloadState,
}

/// Download life cycle: `Started` → `Destination` → `Progress`* → `Finished`
/// or `Failed` (a download that fails before the response arrives skips
/// `Destination`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Started {
        url: Option<String>,
    },
    /// The file name was chosen; bytes go to `path`.
    Destination {
        path: PathBuf,
        suggested_filename: Option<String>,
        total: Option<u64>,
    },
    /// Throttled to about ten per second.
    Progress {
        received: u64,
        total: Option<u64>,
        /// 0.0 to 1.0 (engine estimate).
        fraction: f64,
    },
    Finished {
        path: PathBuf,
        bytes: u64,
    },
    Failed {
        error: String,
        /// `Profile::cancel_download` (or the user) stopped it.
        cancelled: bool,
    },
}

/// Receiving half of an event subscription. Ends (`next` returns `None`)
/// when the page or profile that feeds it is gone. Unbounded: a subscriber
/// that never reads keeps every event, so drop streams you are done with.
pub struct EventStream<T> {
    rx: UnboundedReceiver<T>,
}

impl<T> EventStream<T> {
    /// Next event, waiting if none is queued; `None` once the source is gone.
    pub async fn next(&mut self) -> Option<T> {
        std::future::poll_fn(|cx| self.poll_recv(cx)).await
    }

    /// Next queued event without waiting.
    pub fn try_next(&mut self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    /// Poll for the next event.
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        futures_core::Stream::poll_next(Pin::new(&mut self.rx), cx)
    }
}

impl<T> futures_core::Stream for EventStream<T> {
    type Item = T;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().poll_recv(cx)
    }
}

impl<T> std::fmt::Debug for EventStream<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventStream")
    }
}

/// Fan-out sender for engine implementations: every `subscribe` gets its own
/// stream and sees every event emitted after it subscribed.
pub struct EventSink<T: Clone> {
    subs: Mutex<Vec<UnboundedSender<T>>>,
}

impl<T: Clone> Default for EventSink<T> {
    fn default() -> Self {
        EventSink {
            subs: Mutex::new(Vec::new()),
        }
    }
}

impl<T: Clone> EventSink<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new stream receiving every later event.
    pub fn subscribe(&self) -> EventStream<T> {
        let (tx, rx) = unbounded();
        self.lock().push(tx);
        EventStream { rx }
    }

    /// Send `event` to every live subscriber, dropping closed ones.
    pub fn emit(&self, event: T) {
        let mut subs = self.lock();
        subs.retain(|tx| !tx.is_closed());
        if let Some((last, rest)) = subs.split_last() {
            for tx in rest {
                let _ = tx.unbounded_send(event.clone());
            }
            let _ = last.unbounded_send(event);
        }
    }

    /// Whether anyone is still listening.
    pub fn has_subscribers(&self) -> bool {
        let mut subs = self.lock();
        subs.retain(|tx| !tx.is_closed());
        !subs.is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<UnboundedSender<T>>> {
        self.subs.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fan_out_and_close() {
        let sink = EventSink::<u32>::new();
        assert!(!sink.has_subscribers());
        let mut a = sink.subscribe();
        let mut b = sink.subscribe();
        sink.emit(1);
        assert_eq!(a.try_next(), Some(1));
        assert_eq!(b.try_next(), Some(1));
        drop(b);
        assert!(sink.has_subscribers());
        sink.emit(2);
        assert_eq!(a.try_next(), Some(2));
        assert_eq!(a.try_next(), None);
        drop(sink);
        // Source gone: stream ends.
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert_eq!(a.poll_recv(&mut cx), Poll::Ready(None));
    }

    #[test]
    fn events_serialize() {
        let ev = PageEvent::Load(LoadState::Finished {
            url: Some("http://x/".into()),
        });
        let s = serde_json::to_string(&ev).unwrap();
        assert_eq!(s, r#"{"load":{"finished":{"url":"http://x/"}}}"#);
        let back: PageEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ev);
    }
}
