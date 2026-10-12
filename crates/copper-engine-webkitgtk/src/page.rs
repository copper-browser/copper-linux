//! Pages: one `WebView` + `UserContentManager` each.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use copper_engine::{
    ConsoleLevel, ConsoleMessage, CrashReason, DownloadId, EngineError, EventSink, EventStream,
    FindOptions, Frames, InjectionTime, InputEvent, JsonMap, JsonValue, LinkHover, LoadState,
    LocalFuture, NewWindowHandler, NewWindowRequest, PageEvent, PageOptions, PageView,
    PermissionId, PermissionKind, Profile, Result, ScriptId, Snapshot, SnapshotRegion, StyleLevel,
    StyleSheetId, UserScript, UserStyleSheet, WebsiteDataKinds, WindowDisposition, World,
};
use glib::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, NavigationPolicyDecision, NavigationType, NetworkError, PolicyDecisionType,
    PolicyError, ResponsePolicyDecision, SnapshotOptions, UserContentInjectedFrames,
    UserContentManager, UserScriptInjectionTime, UserStyleLevel, WebProcessTerminationReason,
    WebView,
};

use crate::profile::WebKitProfile;
use crate::{WebKitContentFilter, convert};

/// Script that posts `console.*` calls to [`CONSOLE_HANDLER`].
const CONSOLE_JS: &str = include_str!("console.js");
/// Async function body behind `PageView::input` (JS-dispatched events).
const INPUT_JS: &str = include_str!("input.js");
/// Message handler (page world) used by `PageOptions::capture_console`.
const CONSOLE_HANDLER: &str = "__copperConsole";
/// Isolated world the input script runs in.
const INPUT_WORLD: &str = "copper-input";
/// Message handler registered in every isolated world a page uses, so the
/// world (and its globals) outlives a single evaluation.
const WORLD_PIN: &str = "__copperWorld";

/// A web page backed by a `WebKitWebView`. Cheap to clone (shared handle).
///
/// The page lives as long as a handle or its widget does: event streams keep
/// receiving while the widget is alive, even after every handle is dropped.
#[derive(Clone)]
pub struct WebKitPage {
    inner: Rc<PageInner>,
}

struct PageInner {
    view: WebView,
    ucm: UserContentManager,
    profile: WebKitProfile,
    shared: Rc<Shared>,
    scripts: RefCell<HashMap<ScriptId, webkit6::UserScript>>,
    sheets: RefCell<HashMap<StyleSheetId, webkit6::UserStyleSheet>>,
    /// (name, world) pairs registered through `add_message_handler`.
    handlers: RefCell<Vec<(String, Option<String>)>>,
    /// One `script-message-received::<name>` connection per handler name.
    handler_signals: RefCell<HashMap<String, glib::SignalHandlerId>>,
    /// Isolated worlds pinned for the page's lifetime (see `pin_world`).
    worlds: RefCell<std::collections::HashSet<String>>,
}

/// State shared with signal handlers. It never owns the view, so the view's
/// handlers can hold it strongly without a reference cycle.
struct Shared {
    sink: Rc<EventSink<PageEvent>>,
    permissions: RefCell<HashMap<PermissionId, webkit6::PermissionRequest>>,
    new_window: RefCell<Option<NewWindowHandler<WebKitPage>>>,
    hover: RefCell<Option<String>>,
}

impl WebKitPage {
    pub(crate) fn create(
        profile: &WebKitProfile,
        opts: &PageOptions,
        related: Option<&WebView>,
    ) -> WebKitPage {
        let ucm = UserContentManager::new();
        let settings = webkit6::Settings::new();
        if let Some(ua) = &opts.user_agent {
            settings.set_user_agent(Some(ua));
        }
        settings.set_enable_developer_extras(opts.developer_extras);
        settings.set_javascript_can_open_windows_automatically(opts.allow_script_popups);

        let builder = WebView::builder()
            .user_content_manager(&ucm)
            .settings(&settings)
            .zoom_level(opts.zoom)
            .is_muted(opts.muted)
            .hexpand(true)
            .vexpand(true);
        let view = match related {
            // A related view shares its parent's web process and network session.
            Some(parent) => builder.related_view(parent).build(),
            None => builder.network_session(profile.session()).build(),
        };

        let shared = Rc::new(Shared {
            sink: Rc::new(EventSink::new()),
            permissions: RefCell::default(),
            new_window: RefCell::default(),
            hover: RefCell::default(),
        });
        profile.register_page(&view, Rc::downgrade(&shared.sink));
        let page = WebKitPage {
            inner: Rc::new(PageInner {
                view,
                ucm,
                profile: profile.clone(),
                shared,
                scripts: RefCell::default(),
                sheets: RefCell::default(),
                handlers: RefCell::default(),
                handler_signals: RefCell::default(),
                worlds: RefCell::default(),
            }),
        };
        page.connect_signals();
        if opts.capture_console {
            page.register_handler(CONSOLE_HANDLER, None);
            page.add_user_script(
                &UserScript::new(CONSOLE_JS)
                    .at(InjectionTime::DocumentStart)
                    .frames(Frames::All),
            );
        }
        page
    }

    /// The GTK widget to put on screen (a `WebKitWebView`).
    pub fn widget(&self) -> gtk4::Widget {
        self.inner.view.clone().upcast()
    }

    /// The profile this page belongs to.
    pub fn profile(&self) -> &WebKitProfile {
        &self.inner.profile
    }

    fn sink(&self) -> &EventSink<PageEvent> {
        &self.inner.shared.sink
    }

    fn connect_signals(&self) {
        let view = &self.inner.view;
        let sh = &self.inner.shared;

        let s = sh.sink.clone();
        view.connect_title_notify(move |v| {
            let title = v.title().map(String::from).filter(|t| !t.is_empty());
            s.emit(PageEvent::Title(title));
        });
        let s = sh.sink.clone();
        view.connect_uri_notify(move |v| s.emit(PageEvent::Url(v.uri().map(String::from))));
        let s = sh.sink.clone();
        view.connect_favicon_notify(move |v| {
            s.emit(PageEvent::Favicon(
                v.favicon().map(|t| convert::texture_to_image(&t)),
            ));
        });
        let s = sh.sink.clone();
        view.connect_estimated_load_progress_notify(move |v| {
            s.emit(PageEvent::Progress(v.estimated_load_progress()));
        });
        let s = sh.sink.clone();
        view.connect_is_playing_audio_notify(move |v| {
            s.emit(PageEvent::AudioPlaying(v.is_playing_audio()));
        });
        let s = sh.sink.clone();
        view.connect_close(move |_| s.emit(PageEvent::CloseRequested));

        let sh2 = sh.clone();
        view.connect_load_changed(move |v, ev| {
            let sh = &sh2;
            let url = v.uri().map(String::from);
            let state = match ev {
                LoadEvent::Started => LoadState::Started { url },
                LoadEvent::Redirected => LoadState::Redirected { url },
                LoadEvent::Committed => {
                    // A new document: pending permission prompts are moot
                    // (dropping a request denies it).
                    sh.permissions.borrow_mut().clear();
                    LoadState::Committed { url }
                }
                LoadEvent::Finished => LoadState::Finished { url },
                _ => return,
            };
            sh.sink.emit(PageEvent::Load(state));
        });
        let s = sh.sink.clone();
        view.connect_load_failed(move |_, _, uri, err| {
            let cancelled = err.kind::<NetworkError>() == Some(NetworkError::Cancelled)
                || err.kind::<PolicyError>()
                    == Some(PolicyError::FrameLoadInterruptedByPolicyChange);
            s.emit(PageEvent::Load(LoadState::Failed {
                url: uri.to_owned(),
                error: err.message().to_owned(),
                cancelled,
            }));
            false
        });

        let s = sh.sink.clone();
        view.connect_web_process_terminated(move |_, reason| {
            let reason = match reason {
                WebProcessTerminationReason::ExceededMemoryLimit => {
                    CrashReason::ExceededMemoryLimit
                }
                WebProcessTerminationReason::TerminatedByApi => CrashReason::TerminatedByApi,
                _ => CrashReason::Crashed,
            };
            s.emit(PageEvent::Crashed(reason));
        });

        let sh2 = sh.clone();
        view.connect_mouse_target_changed(move |_, hit, _modifiers| {
            let sh = &sh2;
            let link = hit
                .context_is_link()
                .then(|| hit.link_uri().map(String::from))
                .flatten();
            if *sh.hover.borrow() == link {
                return;
            }
            *sh.hover.borrow_mut() = link.clone();
            sh.sink.emit(PageEvent::LinkHover(link.map(|url| LinkHover {
                url,
                title: hit.link_title().map(String::from),
                label: hit.link_label().map(String::from),
            })));
        });

        let sh2 = sh.clone();
        view.connect_permission_request(move |v, req| {
            let sh = &sh2;
            if !sh.sink.has_subscribers() {
                req.deny();
                return true;
            }
            let id = PermissionId(crate::next_id());
            sh.permissions.borrow_mut().insert(id, req.clone());
            sh.sink.emit(PageEvent::PermissionRequest {
                id,
                kind: permission_kind(req),
                url: v.uri().map(String::from),
            });
            true
        });

        let sh2 = sh.clone();
        view.connect_create(move |_, action| {
            let sh = &sh2;
            let req = NewWindowRequest {
                url: action.request().and_then(|r| r.uri()).map(String::from),
                user_gesture: action.is_user_gesture(),
                frame_name: action.frame_name().map(String::from),
                disposition: WindowDisposition::Popup,
            };
            // Take the handler out while it runs so it may replace itself.
            let handler = sh.new_window.borrow_mut().take();
            let page = handler.as_ref().and_then(|h| h(&req));
            if handler.is_some() && sh.new_window.borrow().is_none() {
                *sh.new_window.borrow_mut() = handler;
            }
            match page {
                Some(p) => Some(p.widget()),
                None => {
                    sh.sink.emit(PageEvent::NewWindow(req));
                    None
                }
            }
        });

        let s = sh.sink.clone();
        view.connect_decide_policy(move |_, decision, kind| match kind {
            PolicyDecisionType::NavigationAction => {
                let Some(nav) = decision.downcast_ref::<NavigationPolicyDecision>() else {
                    return false;
                };
                let Some(action) = nav.navigation_action() else {
                    return false;
                };
                let mods = gdk4::ModifierType::from_bits_truncate(action.modifiers());
                let new_tab = action.navigation_type() == NavigationType::LinkClicked
                    && (action.mouse_button() == 2
                        || mods.intersects(
                            gdk4::ModifierType::CONTROL_MASK | gdk4::ModifierType::META_MASK,
                        ));
                if !new_tab {
                    return false;
                }
                s.emit(PageEvent::NewWindow(NewWindowRequest {
                    url: action.request().and_then(|r| r.uri()).map(String::from),
                    user_gesture: true,
                    frame_name: None,
                    disposition: WindowDisposition::NewTab,
                }));
                decision.ignore();
                true
            }
            PolicyDecisionType::Response => {
                let Some(resp) = decision.downcast_ref::<ResponsePolicyDecision>() else {
                    return false;
                };
                // Attachments already download by default; also download
                // main documents WebKit cannot show instead of dropping them.
                if resp.is_main_frame_main_resource() && !resp.is_mime_type_supported() {
                    decision.download();
                    return true;
                }
                false
            }
            _ => false,
        });
    }

    /// WebKit drops a named world once nothing references it, so state set
    /// by one `eval` in `Isolated(name)` would be gone by the next. A message
    /// handler registered in the world keeps it alive while the page lives.
    fn pin_world(&self, world: &World) {
        let Some(name) = world.name() else { return };
        if self.inner.worlds.borrow().contains(name) {
            return;
        }
        self.inner
            .ucm
            .register_script_message_handler(WORLD_PIN, Some(name));
        self.inner.worlds.borrow_mut().insert(name.to_owned());
    }

    /// Register a message handler and make sure its name is routed to events.
    fn register_handler(&self, name: &str, world: Option<&str>) -> bool {
        let key = (name.to_owned(), world.map(str::to_owned));
        if self.inner.handlers.borrow().contains(&key) {
            return false;
        }
        if !self.inner.ucm.register_script_message_handler(name, world) {
            return false;
        }
        self.inner.handlers.borrow_mut().push(key);
        let mut signals = self.inner.handler_signals.borrow_mut();
        if !signals.contains_key(name) {
            let s = self.inner.shared.sink.clone();
            let n = name.to_owned();
            let id = self
                .inner
                .ucm
                .connect_script_message_received(Some(name), move |_, value| {
                    let body = convert::js_to_json(value).unwrap_or_else(|e| {
                        tracing::debug!("script message {n}: {e}");
                        JsonValue::Null
                    });
                    if n == CONSOLE_HANDLER {
                        if let Some(msg) = console_message(&body) {
                            s.emit(PageEvent::Console(msg));
                        }
                    } else {
                        s.emit(PageEvent::ScriptMessage {
                            name: n.clone(),
                            body,
                        });
                    }
                });
            signals.insert(name.to_owned(), id);
        }
        true
    }
}

impl PageView for WebKitPage {
    type Filter = WebKitContentFilter;

    fn load(&self, url: &str) {
        self.inner.view.load_uri(url);
    }

    fn load_html(&self, html: &str, base_url: Option<&str>) {
        self.inner.view.load_html(html, base_url);
    }

    fn back(&self) {
        self.inner.view.go_back();
    }

    fn forward(&self) {
        self.inner.view.go_forward();
    }

    fn can_go_back(&self) -> bool {
        self.inner.view.can_go_back()
    }

    fn can_go_forward(&self) -> bool {
        self.inner.view.can_go_forward()
    }

    fn reload(&self, bypass_cache: bool) {
        if bypass_cache {
            self.inner.view.reload_bypass_cache();
        } else {
            self.inner.view.reload();
        }
    }

    fn hard_reload(&self) -> LocalFuture<Result<Vec<String>>> {
        let host = self.url().as_deref().and_then(convert::url_host);
        let clear = host.map(|h| {
            self.inner
                .profile
                .clear_website_data(Some(&h), WebsiteDataKinds::CACHES)
        });
        let view = self.inner.view.downgrade();
        Box::pin(async move {
            let names = match clear {
                Some(f) => f.await?,
                None => Vec::new(),
            };
            if let Some(v) = view.upgrade() {
                v.reload_bypass_cache();
            }
            Ok(names)
        })
    }

    fn stop(&self) {
        self.inner.view.stop_loading();
    }

    fn url(&self) -> Option<String> {
        self.inner.view.uri().map(String::from)
    }

    fn title(&self) -> Option<String> {
        self.inner
            .view
            .title()
            .map(String::from)
            .filter(|t| !t.is_empty())
    }

    fn is_loading(&self) -> bool {
        self.inner.view.is_loading()
    }

    fn progress(&self) -> f64 {
        self.inner.view.estimated_load_progress()
    }

    fn is_audio_playing(&self) -> bool {
        self.inner.view.is_playing_audio()
    }

    fn set_muted(&self, muted: bool) {
        self.inner.view.set_is_muted(muted);
    }

    fn zoom(&self) -> f64 {
        self.inner.view.zoom_level()
    }

    fn set_zoom(&self, level: f64) {
        self.inner.view.set_zoom_level(level);
    }

    fn eval(&self, js: &str, world: &World) -> LocalFuture<Result<JsonValue>> {
        self.pin_world(world);
        let fut = self
            .inner
            .view
            .evaluate_javascript_future(js, world.name(), None);
        Box::pin(async move {
            let v = fut.await.map_err(convert::js_error)?;
            convert::js_to_json(&v)
        })
    }

    fn call_async(
        &self,
        body: &str,
        args: &JsonMap,
        world: &World,
    ) -> LocalFuture<Result<JsonValue>> {
        // Arguments travel as JSON strings (data, never spliced into code)
        // and are parsed back in a prologue, so any JSON value works.
        let mut prologue = String::new();
        let dict = glib::VariantDict::new(None);
        for (name, value) in args {
            if !convert::is_js_identifier(name) {
                let err = EngineError::InvalidArgument(format!(
                    "argument name {name:?} is not a JS identifier"
                ));
                return Box::pin(std::future::ready(Err(err)));
            }
            dict.insert_value(name, &value.to_string().to_variant());
            prologue.push_str(&format!("{name} = JSON.parse({name});\n"));
        }
        let arguments = (!args.is_empty()).then(|| dict.end());
        self.pin_world(world);
        let full = format!("{prologue}{body}");
        let fut = self.inner.view.call_async_javascript_function_future(
            &full,
            arguments.as_ref(),
            world.name(),
            None,
        );
        Box::pin(async move {
            let v = fut.await.map_err(convert::js_error)?;
            convert::js_to_json(&v)
        })
    }

    fn add_message_handler(&self, name: &str, world: &World) -> bool {
        if name == CONSOLE_HANDLER || name == WORLD_PIN || name.is_empty() {
            return false;
        }
        self.register_handler(name, world.name())
    }

    fn remove_message_handler(&self, name: &str, world: &World) {
        let key = (name.to_owned(), world.name().map(str::to_owned));
        let mut handlers = self.inner.handlers.borrow_mut();
        let before = handlers.len();
        handlers.retain(|k| *k != key);
        if handlers.len() == before {
            return;
        }
        self.inner
            .ucm
            .unregister_script_message_handler(name, world.name());
        if !handlers.iter().any(|(n, _)| n == name)
            && let Some(id) = self.inner.handler_signals.borrow_mut().remove(name)
        {
            self.inner.ucm.disconnect(id);
        }
    }

    fn add_user_script(&self, script: &UserScript) -> ScriptId {
        let frames = injected_frames(script.frames);
        let time = match script.injection_time {
            InjectionTime::DocumentStart => UserScriptInjectionTime::Start,
            InjectionTime::DocumentEnd => UserScriptInjectionTime::End,
        };
        let allow: Vec<&str> = script.allow_list.iter().map(String::as_str).collect();
        let block: Vec<&str> = script.block_list.iter().map(String::as_str).collect();
        self.pin_world(&script.world);
        let ws = match script.world.name() {
            Some(w) => {
                webkit6::UserScript::for_world(&script.source, frames, time, w, &allow, &block)
            }
            None => webkit6::UserScript::new(&script.source, frames, time, &allow, &block),
        };
        self.inner.ucm.add_script(&ws);
        let id = ScriptId(crate::next_id());
        self.inner.scripts.borrow_mut().insert(id, ws);
        id
    }

    fn remove_user_script(&self, id: ScriptId) -> bool {
        match self.inner.scripts.borrow_mut().remove(&id) {
            Some(ws) => {
                self.inner.ucm.remove_script(&ws);
                true
            }
            None => false,
        }
    }

    fn add_user_style_sheet(&self, sheet: &UserStyleSheet) -> StyleSheetId {
        let frames = injected_frames(sheet.frames);
        let level = match sheet.level {
            StyleLevel::User => UserStyleLevel::User,
            StyleLevel::Author => UserStyleLevel::Author,
        };
        let allow: Vec<&str> = sheet.allow_list.iter().map(String::as_str).collect();
        let block: Vec<&str> = sheet.block_list.iter().map(String::as_str).collect();
        let ws = match sheet.world.name() {
            Some(w) => {
                webkit6::UserStyleSheet::for_world(&sheet.source, frames, level, w, &allow, &block)
            }
            None => webkit6::UserStyleSheet::new(&sheet.source, frames, level, &allow, &block),
        };
        self.inner.ucm.add_style_sheet(&ws);
        let id = StyleSheetId(crate::next_id());
        self.inner.sheets.borrow_mut().insert(id, ws);
        id
    }

    fn remove_user_style_sheet(&self, id: StyleSheetId) -> bool {
        match self.inner.sheets.borrow_mut().remove(&id) {
            Some(ws) => {
                self.inner.ucm.remove_style_sheet(&ws);
                true
            }
            None => false,
        }
    }

    fn set_content_filter(&self, filter: Option<&WebKitContentFilter>) {
        self.inner.ucm.remove_all_filters();
        if let Some(f) = filter {
            self.inner.ucm.add_filter(f.raw());
        }
    }

    fn add_content_filter(&self, filter: &WebKitContentFilter) {
        self.inner.ucm.add_filter(filter.raw());
    }

    fn remove_content_filter(&self, id: &str) {
        self.inner.ucm.remove_filter_by_id(id);
    }

    fn snapshot(&self, region: SnapshotRegion) -> LocalFuture<Result<Snapshot>> {
        let region = match region {
            SnapshotRegion::Visible => webkit6::SnapshotRegion::Visible,
            SnapshotRegion::FullDocument => webkit6::SnapshotRegion::FullDocument,
        };
        let fut = self
            .inner
            .view
            .snapshot_future(region, SnapshotOptions::NONE);
        Box::pin(async move {
            let tex = fut
                .await
                .map_err(|e| EngineError::Snapshot(e.message().to_owned()))?;
            Ok(convert::texture_to_image(&tex))
        })
    }

    fn find(&self, query: &str, opts: FindOptions) -> LocalFuture<Result<u32>> {
        let Some(fc) = self.inner.view.find_controller() else {
            return Box::pin(std::future::ready(Err(EngineError::Other(
                "no find controller".into(),
            ))));
        };
        if query.is_empty() {
            fc.search_finish();
            return Box::pin(std::future::ready(Ok(0)));
        }
        let mut flags = webkit6::FindOptions::NONE;
        if !opts.case_sensitive {
            flags |= webkit6::FindOptions::CASE_INSENSITIVE;
        }
        if opts.backwards {
            flags |= webkit6::FindOptions::BACKWARDS;
        }
        if opts.wrap_around {
            flags |= webkit6::FindOptions::WRAP_AROUND;
        }
        if opts.at_word_starts {
            flags |= webkit6::FindOptions::AT_WORD_STARTS;
        }
        let max = opts.max_matches.max(1);
        let (tx, rx) = futures_channel::oneshot::channel::<u32>();
        let tx = Rc::new(RefCell::new(Some(tx)));
        let t = tx.clone();
        let found = fc.connect_found_text(move |_, n| {
            if let Some(tx) = t.borrow_mut().take() {
                let _ = tx.send(n);
            }
        });
        let failed = fc.connect_failed_to_find_text(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(0);
            }
        });
        let guard = SignalGuard {
            obj: fc.clone().upcast(),
            ids: vec![found, failed],
        };
        fc.search(query, flags.bits(), max);
        Box::pin(async move {
            let n = rx.await.unwrap_or(0);
            drop(guard);
            Ok(n)
        })
    }

    fn find_next(&self) {
        if let Some(fc) = self.inner.view.find_controller() {
            fc.search_next();
        }
    }

    fn find_previous(&self) {
        if let Some(fc) = self.inner.view.find_controller() {
            fc.search_previous();
        }
    }

    fn find_finish(&self) {
        if let Some(fc) = self.inner.view.find_controller() {
            fc.search_finish();
        }
    }

    fn events(&self) -> EventStream<PageEvent> {
        self.sink().subscribe()
    }

    fn answer_permission(&self, id: PermissionId, allow: bool) -> bool {
        let req = self.inner.shared.permissions.borrow_mut().remove(&id);
        match req {
            Some(req) => {
                if allow {
                    req.allow();
                } else {
                    req.deny();
                }
                true
            }
            None => false,
        }
    }

    fn new_related(&self, opts: PageOptions) -> WebKitPage {
        WebKitPage::create(&self.inner.profile, &opts, Some(&self.inner.view))
    }

    fn set_new_window_handler(&self, handler: Option<NewWindowHandler<WebKitPage>>) {
        *self.inner.shared.new_window.borrow_mut() = handler;
    }

    fn download(&self, url: &str) -> Result<DownloadId> {
        let dl = self
            .inner
            .view
            .download_uri(url)
            .ok_or_else(|| EngineError::Download(format!("cannot download {url}")))?;
        Ok(self.inner.profile.track_download(&dl))
    }

    fn input(&self, event: InputEvent) -> LocalFuture<Result<()>> {
        let ev = match serde_json::to_value(&event) {
            Ok(v) => v,
            Err(e) => {
                return Box::pin(std::future::ready(Err(EngineError::InvalidArgument(
                    e.to_string(),
                ))));
            }
        };
        let mut args = JsonMap::new();
        args.insert("ev".into(), ev);
        let fut = self.call_async(INPUT_JS, &args, &World::isolated(INPUT_WORLD));
        Box::pin(async move {
            let out = fut.await?;
            if out.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                Ok(())
            } else {
                let why = out
                    .get("error")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("input was not delivered");
                Err(EngineError::NotFound(why.to_owned()))
            }
        })
    }

    fn show_inspector(&self) {
        if let Some(i) = self.inner.view.inspector() {
            i.show();
        }
    }

    fn terminate_web_process(&self) {
        self.inner.view.terminate_web_process();
    }
}

impl std::fmt::Debug for WebKitPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebKitPage")
            .field("url", &self.url())
            .field("profile", &self.inner.profile)
            .finish()
    }
}

/// Disconnects signal handlers when dropped (futures that are abandoned
/// must not leave handlers behind).
struct SignalGuard {
    obj: glib::Object,
    ids: Vec<glib::SignalHandlerId>,
}

impl Drop for SignalGuard {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            self.obj.disconnect(id);
        }
    }
}

fn injected_frames(frames: Frames) -> UserContentInjectedFrames {
    match frames {
        Frames::All => UserContentInjectedFrames::AllFrames,
        Frames::TopOnly => UserContentInjectedFrames::TopFrame,
    }
}

fn console_message(body: &JsonValue) -> Option<ConsoleMessage> {
    let level = match body.get("level")?.as_str()? {
        "info" => ConsoleLevel::Info,
        "warn" => ConsoleLevel::Warn,
        "error" => ConsoleLevel::Error,
        "debug" => ConsoleLevel::Debug,
        _ => ConsoleLevel::Log,
    };
    let message = body.get("message")?.as_str()?.to_owned();
    Some(ConsoleMessage { level, message })
}

fn permission_kind(req: &webkit6::PermissionRequest) -> PermissionKind {
    use webkit6::{
        ClipboardPermissionRequest, DeviceInfoPermissionRequest, GeolocationPermissionRequest,
        MediaKeySystemPermissionRequest, NotificationPermissionRequest,
        PointerLockPermissionRequest, UserMediaPermissionRequest,
        WebsiteDataAccessPermissionRequest, XRPermissionRequest,
    };
    if req.is::<GeolocationPermissionRequest>() {
        PermissionKind::Geolocation
    } else if req.is::<NotificationPermissionRequest>() {
        PermissionKind::Notifications
    } else if let Some(m) = req.downcast_ref::<UserMediaPermissionRequest>() {
        if webkit6::functions::user_media_permission_is_for_display_device(m) {
            PermissionKind::DisplayCapture
        } else {
            match (m.is_for_video_device(), m.is_for_audio_device()) {
                (true, true) => PermissionKind::CameraAndMicrophone,
                (true, false) => PermissionKind::Camera,
                _ => PermissionKind::Microphone,
            }
        }
    } else if req.is::<ClipboardPermissionRequest>() {
        PermissionKind::Clipboard
    } else if req.is::<DeviceInfoPermissionRequest>() {
        PermissionKind::DeviceInfo
    } else if req.is::<PointerLockPermissionRequest>() {
        PermissionKind::PointerLock
    } else if req.is::<MediaKeySystemPermissionRequest>() {
        PermissionKind::MediaKeySystem
    } else if let Some(w) = req.downcast_ref::<WebsiteDataAccessPermissionRequest>() {
        PermissionKind::WebsiteDataAccess {
            requesting_domain: w.requesting_domain().map(String::from),
            current_domain: w.current_domain().map(String::from),
        }
    } else if req.is::<XRPermissionRequest>() {
        PermissionKind::Xr
    } else {
        PermissionKind::Other(req.type_().name().to_owned())
    }
}
