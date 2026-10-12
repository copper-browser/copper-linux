//! Engine and PageView traits, events and value types.
//!
//! This crate is the boundary between Copper and its web engine (D-003).
//! It names no GTK or WebKit type; `copper-engine-webkitgtk` is the only
//! implementation. A shell or MCP server written against these traits never
//! sees `webkit6`.
//!
//! # Threading
//! Engine objects (`Engine`, `Profile`, `PageView`, `ContentFilter`) are
//! main-thread only: they are `!Send` and every call must happen on the
//! thread running the UI main loop. Async results are [`LocalFuture`]s —
//! poll them on that same loop (`glib::MainContext::spawn_local`). Values
//! that come *out* of the engine (events, JSON, images, cookies) are plain
//! `Send` data, so an [`EventStream`] may be moved to a worker thread.
//!
//! # Events
//! `PageView::events()` and `Profile::downloads()` return an independent
//! [`EventStream`] per call; each sees every event emitted after it was
//! created. Drop a stream to unsubscribe.

mod error;
mod events;
mod input;
mod paths;
mod types;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub use error::{EngineError, Result};
pub use events::{
    ConsoleLevel, ConsoleMessage, CrashReason, DownloadEvent, DownloadState, EventSink,
    EventStream, LinkHover, LoadState, NewWindowRequest, PageEvent, PermissionKind,
    WindowDisposition,
};
pub use input::{InputEvent, Modifiers, MouseButton};
pub use paths::EnginePaths;
pub use types::{
    Cookie, DownloadId, FindOptions, Frames, Image, InjectionTime, PageOptions, PermissionId,
    ProfileId, SameSite, ScriptId, SnapshotRegion, StyleLevel, StyleSheetId, UserScript,
    UserStyleSheet, WebsiteDataKinds, World,
};

/// JSON value returned by script evaluation and carried by script messages.
pub type JsonValue = serde_json::Value;
/// JSON object (named arguments for `PageView::call_async`).
pub type JsonMap = serde_json::Map<String, JsonValue>;

/// A main-thread future (not `Send`) resolving to `T`.
pub type LocalFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

/// A snapshot is a PNG with its size.
pub type Snapshot = Image;

/// Decides what to do with a popup request synchronously. Return a page made
/// with [`PageView::new_related`] on the requesting page to give the popup a
/// live `window.opener`; return `None` to refuse it (a `PageEvent::NewWindow`
/// is still emitted so the shell can open the URL in a plain tab). The page
/// owns its handler, so capture the page itself weakly to avoid a cycle.
pub type NewWindowHandler<P> = Box<dyn Fn(&NewWindowRequest) -> Option<P>>;

/// The engine: makes profiles, pages and compiled content filters.
pub trait Engine {
    type Profile: Profile;
    type Filter: ContentFilter;
    type Page: PageView<Filter = Self::Filter>;

    /// On-disk layout this engine uses.
    fn paths(&self) -> &EnginePaths;

    /// The profile `id`. Persistent profiles keep data under
    /// `paths().profile_data_dir(id)`; ephemeral ones keep nothing on disk.
    /// The same `(id, ephemeral)` returns the same profile while any handle
    /// to it is alive.
    fn profile(&self, id: &ProfileId, ephemeral: bool) -> Result<Self::Profile>;

    /// A new page (tab) in `profile`.
    fn new_page(&self, profile: &Self::Profile, opts: PageOptions) -> Self::Page;

    /// Compile a WebKit content-blocker list (the JSON Safari and
    /// `Shield.swift` use) and store it under `id` in
    /// `paths().content_filter_dir()`, replacing any older version.
    fn compile_content_filter(
        &self,
        id: &str,
        rules_json: &str,
    ) -> LocalFuture<Result<Self::Filter>>;

    /// A previously compiled filter, or `None` if `id` was never stored.
    fn load_content_filter(&self, id: &str) -> LocalFuture<Result<Option<Self::Filter>>>;

    /// Delete a stored filter (pages already using it keep it until reset).
    fn remove_content_filter(&self, id: &str) -> LocalFuture<Result<()>>;
}

/// A compiled content-blocker list, ready to attach to pages.
pub trait ContentFilter: Clone {
    fn id(&self) -> &str;
}

/// A cookie/storage jar plus its downloads. Cheap to clone (shared handle).
pub trait Profile: Clone {
    fn id(&self) -> &ProfileId;
    fn is_ephemeral(&self) -> bool;
    /// Data directory of a persistent profile.
    fn data_dir(&self) -> Option<&Path>;

    /// Cookies that would be sent to `url`.
    fn cookies(&self, url: &str) -> LocalFuture<Result<Vec<Cookie>>>;
    /// Every cookie in the jar.
    fn all_cookies(&self) -> LocalFuture<Result<Vec<Cookie>>>;
    /// Add or replace a cookie.
    fn set_cookie(&self, cookie: &Cookie) -> LocalFuture<Result<()>>;
    /// Delete the cookie matching name, domain and path.
    fn delete_cookie(&self, cookie: &Cookie) -> LocalFuture<Result<()>>;

    /// Remove `kinds` of website data for `domain` and its subdomains (or for
    /// every site when `domain` is `None`). Resolves to the site names removed.
    fn clear_website_data(
        &self,
        domain: Option<&str>,
        kinds: WebsiteDataKinds,
    ) -> LocalFuture<Result<Vec<String>>>;

    /// Download `url` into the download folder (no page involved).
    fn download(&self, url: &str) -> Result<DownloadId>;
    /// Cancel a running download; `false` if it is unknown or already over.
    fn cancel_download(&self, id: DownloadId) -> bool;
    /// Folder new downloads are saved in (created on demand).
    fn download_dir(&self) -> PathBuf;
    fn set_download_dir(&self, dir: PathBuf);
    /// Every download in this profile, including page-started ones.
    fn downloads(&self) -> EventStream<DownloadEvent>;
}

/// One web page (a tab's content).
pub trait PageView {
    type Filter: ContentFilter;

    // -- navigation --------------------------------------------------------
    fn load(&self, url: &str);
    /// Show `html` as if it came from `base_url` (internal pages, crash page).
    fn load_html(&self, html: &str, base_url: Option<&str>);
    fn back(&self);
    fn forward(&self);
    fn can_go_back(&self) -> bool;
    fn can_go_forward(&self) -> bool;
    /// Reload; `bypass_cache` revalidates everything with the origin.
    fn reload(&self, bypass_cache: bool);
    /// Clear the memory + disk HTTP cache for this page's site, then reload
    /// from origin (macOS `Tab.hardReload`). Resolves to the site names cleared.
    fn hard_reload(&self) -> LocalFuture<Result<Vec<String>>>;
    fn stop(&self);

    // -- state -------------------------------------------------------------
    fn url(&self) -> Option<String>;
    fn title(&self) -> Option<String>;
    fn is_loading(&self) -> bool;
    /// Estimated load progress, 0.0 to 1.0.
    fn progress(&self) -> f64;
    fn is_audio_playing(&self) -> bool;
    fn set_muted(&self, muted: bool);
    fn zoom(&self) -> f64;
    fn set_zoom(&self, level: f64);

    // -- script ------------------------------------------------------------
    /// Evaluate an expression; resolves to its JSON form (`undefined` → `null`).
    /// Promises are not awaited — use `call_async` for that.
    fn eval(&self, js: &str, world: &World) -> LocalFuture<Result<JsonValue>>;
    /// Run `body` as an async function body (`await` allowed, `return` gives
    /// the result). Each key of `args` is a parameter holding its JSON value.
    /// Keys must be plain JS identifiers.
    fn call_async(
        &self,
        body: &str,
        args: &JsonMap,
        world: &World,
    ) -> LocalFuture<Result<JsonValue>>;
    /// Register `window.webkit.messageHandlers.<name>` in `world`; messages
    /// arrive as `PageEvent::ScriptMessage`. `false` if already registered.
    fn add_message_handler(&self, name: &str, world: &World) -> bool;
    fn remove_message_handler(&self, name: &str, world: &World);
    /// Inject a script into every matching document from now on.
    fn add_user_script(&self, script: &UserScript) -> ScriptId;
    fn remove_user_script(&self, id: ScriptId) -> bool;
    fn add_user_style_sheet(&self, sheet: &UserStyleSheet) -> StyleSheetId;
    fn remove_user_style_sheet(&self, id: StyleSheetId) -> bool;

    // -- content blocking ----------------------------------------------------
    /// Replace every attached filter with `filter` (or none). Applies from the
    /// next load.
    fn set_content_filter(&self, filter: Option<&Self::Filter>);
    /// Attach one more filter.
    fn add_content_filter(&self, filter: &Self::Filter);
    fn remove_content_filter(&self, id: &str);

    // -- capture, find ---------------------------------------------------------
    /// Paint the page into a PNG. `Visible` needs the page to be on screen
    /// (mapped); see T-002 for off-screen agent tabs.
    fn snapshot(&self, region: SnapshotRegion) -> LocalFuture<Result<Snapshot>>;
    /// Highlight matches of `query`, select the first; resolves to the number
    /// of matches (0 = not found, capped at `opts.max_matches`).
    fn find(&self, query: &str, opts: FindOptions) -> LocalFuture<Result<u32>>;
    fn find_next(&self);
    fn find_previous(&self);
    /// Clear find highlights.
    fn find_finish(&self);

    // -- events, permissions, popups, downloads -------------------------------
    fn events(&self) -> EventStream<PageEvent>;
    /// Answer a `PageEvent::PermissionRequest`; `false` if already answered.
    fn answer_permission(&self, id: PermissionId, allow: bool) -> bool;
    /// A page in the same profile and web process, for popups that need
    /// `window.opener` (return it from the new-window handler).
    fn new_related(&self, opts: PageOptions) -> Self
    where
        Self: Sized;
    fn set_new_window_handler(&self, handler: Option<NewWindowHandler<Self>>)
    where
        Self: Sized;
    /// Download `url` in this page's profile; events go to the page too.
    fn download(&self, url: &str) -> Result<DownloadId>;

    // -- input -----------------------------------------------------------------
    /// Deliver an input action. Current implementations dispatch DOM events
    /// from script (`isTrusted == false`): pointer/mouse events at the element
    /// under the point (through same-origin iframes and open shadow roots),
    /// keyboard events at the focused element, typing through the native
    /// value setter + `beforeinput`/`input`. Resolves once dispatched.
    fn input(&self, event: InputEvent) -> LocalFuture<Result<()>>;

    // -- tools -------------------------------------------------------------------
    /// Open the web inspector (needs `PageOptions::developer_extras`).
    fn show_inspector(&self);
    /// Kill the web process (emits `PageEvent::Crashed(TerminatedByApi)`).
    fn terminate_web_process(&self);
}
