//! WebKitGTK 6.0 implementation of copper-engine; owns every webkit6 call.
//!
//! [`WebKitEngine`] implements [`copper_engine::Engine`]; its profiles are
//! [`WebKitProfile`] (one `NetworkSession` each), its pages [`WebKitPage`]
//! (one `WebView` + `UserContentManager` each) and its compiled filters
//! [`WebKitContentFilter`], its backstage (pages an agent works in while
//! nobody looks at them) [`WebKitBackstage`]. The GTK types in the public API
//! are [`WebKitPage::widget`], which the shell needs to put the page on
//! screen, and the `gtk4::Window` the backstage hosts pages in.
//!
//! `PageView::input` delivers pointer moves, left clicks and typing through
//! WebKit's own input handling (`isTrusted == true`) and the rest from
//! script; see `hands.rs` and [`WebKitPage::trusted_input`].
//!
//! Everything here is main-thread only: create the engine after
//! `gtk4::init()` on the thread that runs the GTK main loop, and poll the
//! returned futures on that loop.

mod backstage;
mod caps;
mod convert;
mod ghost;
mod hands;
mod page;
mod profile;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use copper_engine::{
    Capabilities, ContentFilter, Engine, EngineError, EnginePaths, LocalFuture, PageOptions,
    ProfileId, Result, SchemeHandler, SchemeRequest, ScriptId, UserScript,
};
use webkit6::{UserContentFilter, UserContentFilterError, UserContentFilterStore};

pub use backstage::WebKitBackstage;
/// What `PageView::input` delivers trusted (moved to `copper-engine`).
pub use copper_engine::TrustedInput;
pub use page::{WeakWebKitPage, WebKitPage};
pub use profile::WebKitProfile;

/// Re-export so callers can name the engine-neutral API through this crate.
pub use copper_engine;

/// The WebKitGTK engine. Cheap to clone (shared handle).
#[derive(Clone)]
pub struct WebKitEngine {
    inner: Rc<EngineInner>,
}

struct EngineInner {
    paths: EnginePaths,
    profiles: RefCell<HashMap<(ProfileId, bool), Weak<profile::ProfileInner>>>,
    filters: RefCell<Option<UserContentFilterStore>>,
    backstage: WebKitBackstage,
    page_scripts: Rc<page::PageScripts>,
    /// Handlers for `Engine::register_scheme`, by lower-case scheme.
    schemes: RefCell<HashMap<String, Rc<SchemeHandler>>>,
}

impl WebKitEngine {
    /// An engine storing its state under `paths`. Call after `gtk4::init()`.
    pub fn new(paths: EnginePaths) -> Self {
        hands::self_check();
        let missing = caps::capabilities().unavailable();
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|c| c.name()).collect();
            tracing::info!("this WebKitGTK build lacks: {}", names.join(", "));
        }
        WebKitEngine {
            inner: Rc::new(EngineInner {
                paths,
                profiles: RefCell::default(),
                filters: RefCell::default(),
                backstage: WebKitBackstage::new(),
                page_scripts: Rc::default(),
                schemes: RefCell::default(),
            }),
        }
    }

    /// An engine using the XDG base directories (`~/.local/share/copper`…).
    pub fn with_xdg() -> Result<Self> {
        let paths = EnginePaths::from_xdg()
            .ok_or_else(|| EngineError::Io("cannot resolve the home directory".into()))?;
        Ok(Self::new(paths))
    }

    fn filter_store(&self) -> Result<UserContentFilterStore> {
        if let Some(s) = self.inner.filters.borrow().as_ref() {
            return Ok(s.clone());
        }
        let dir = self.inner.paths.content_filter_dir();
        std::fs::create_dir_all(&dir)?;
        let store = UserContentFilterStore::new(convert::path_str(&dir)?);
        *self.inner.filters.borrow_mut() = Some(store.clone());
        Ok(store)
    }
}

impl Engine for WebKitEngine {
    type Profile = WebKitProfile;
    type Filter = WebKitContentFilter;
    type Page = WebKitPage;
    type Backstage = WebKitBackstage;

    fn paths(&self) -> &EnginePaths {
        &self.inner.paths
    }

    fn profile(&self, id: &ProfileId, ephemeral: bool) -> Result<WebKitProfile> {
        let key = (id.clone(), ephemeral);
        let mut cache = self.inner.profiles.borrow_mut();
        if let Some(inner) = cache.get(&key).and_then(Weak::upgrade) {
            return Ok(WebKitProfile::from_inner(inner));
        }
        cache.retain(|_, w| w.strong_count() > 0);
        let profile = WebKitProfile::create(&self.inner.paths, id, ephemeral)?;
        cache.insert(key, profile.downgrade());
        Ok(profile)
    }

    fn new_page(&self, profile: &WebKitProfile, opts: PageOptions) -> WebKitPage {
        WebKitPage::create(profile, &opts, None, &self.inner.page_scripts)
    }

    fn compile_content_filter(
        &self,
        id: &str,
        rules_json: &str,
    ) -> LocalFuture<Result<WebKitContentFilter>> {
        let store = self.filter_store();
        let id = id.to_owned();
        let bytes = glib::Bytes::from_owned(rules_json.as_bytes().to_vec());
        Box::pin(async move {
            let filter = store?
                .save_future(&id, &bytes)
                .await
                .map_err(|e| EngineError::ContentFilter(format!("compile {id:?}: {e}")))?;
            Ok(WebKitContentFilter::new(id, filter))
        })
    }

    fn load_content_filter(&self, id: &str) -> LocalFuture<Result<Option<WebKitContentFilter>>> {
        let store = self.filter_store();
        let id = id.to_owned();
        Box::pin(async move {
            match store?.load_future(&id).await {
                Ok(filter) => Ok(Some(WebKitContentFilter::new(id, filter))),
                Err(e)
                    if e.kind::<UserContentFilterError>()
                        == Some(UserContentFilterError::NotFound) =>
                {
                    Ok(None)
                }
                Err(e) => Err(EngineError::ContentFilter(format!("load {id:?}: {e}"))),
            }
        })
    }

    fn remove_content_filter(&self, id: &str) -> LocalFuture<Result<()>> {
        let store = self.filter_store();
        let id = id.to_owned();
        Box::pin(async move {
            match store?.remove_future(&id).await {
                Ok(()) => Ok(()),
                Err(e)
                    if e.kind::<UserContentFilterError>()
                        == Some(UserContentFilterError::NotFound) =>
                {
                    Ok(())
                }
                Err(e) => Err(EngineError::ContentFilter(format!("remove {id:?}: {e}"))),
            }
        })
    }

    fn backstage(&self) -> &WebKitBackstage {
        &self.inner.backstage
    }

    fn capabilities(&self) -> Capabilities {
        caps::capabilities()
    }

    fn add_page_script(&self, script: &UserScript) -> ScriptId {
        self.inner.page_scripts.add(script)
    }

    fn remove_page_script(&self, id: ScriptId) -> bool {
        self.inner.page_scripts.remove(id)
    }

    fn set_preferred_languages(&self, languages: &[String]) {
        set_preferred_languages(languages);
    }

    /// On WebKit's default web context (every page's), marked secure. WebKit
    /// keeps the first handler a context was given for a scheme, so a second
    /// registration swaps the handler behind the one WebKit calls.
    fn register_scheme(&self, scheme: &str, handler: SchemeHandler) -> Result<()> {
        let scheme = scheme.to_ascii_lowercase();
        if scheme.is_empty()
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return Err(EngineError::InvalidArgument(format!(
                "not a URL scheme: {scheme:?}"
            )));
        }
        let fresh = self
            .inner
            .schemes
            .borrow_mut()
            .insert(scheme.clone(), Rc::new(handler))
            .is_none();
        if !fresh {
            return Ok(());
        }
        let context = webkit6::WebContext::default()
            .ok_or_else(|| EngineError::Other("no default WebKit web context".into()))?;
        if let Some(security) = context.security_manager() {
            security.register_uri_scheme_as_secure(&scheme);
        }
        let engine = Rc::downgrade(&self.inner);
        let name = scheme.clone();
        context.register_uri_scheme(&scheme, move |request| {
            let answer = match engine.upgrade() {
                Some(inner) => inner.answer_scheme(&name, request),
                None => Err("the engine has shut down".to_owned()),
            };
            finish_scheme_request(request, answer);
        });
        tracing::info!("{scheme}: scheme registered");
        Ok(())
    }
}

impl EngineInner {
    /// Ask the handler registered for `scheme` about `request`.
    fn answer_scheme(
        &self,
        scheme: &str,
        request: &webkit6::URISchemeRequest,
    ) -> copper_engine::SchemeResponse {
        let handler = self
            .schemes
            .borrow()
            .get(scheme)
            .cloned()
            .ok_or_else(|| format!("nothing serves {scheme}:"))?;
        let ask = SchemeRequest {
            uri: request.uri().map(|u| u.to_string()).unwrap_or_default(),
            page: request
                .web_view()
                .and_then(|view| self.page_scripts.page_of_view(&view)),
        };
        handler(&ask)
    }
}

/// Answer a custom-scheme request exactly once: the body, or a not-found
/// error with the handler's text.
fn finish_scheme_request(
    request: &webkit6::URISchemeRequest,
    answer: copper_engine::SchemeResponse,
) {
    match answer {
        Ok(body) => {
            let len = body.data.len() as i64;
            let bytes = glib::Bytes::from_owned(body.data);
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            request.finish(&stream, len, Some(&body.mime));
        }
        Err(text) => {
            tracing::debug!(%text, "custom scheme request refused");
            let mut error = glib::Error::new(gio::IOErrorEnum::NotFound, &text);
            request.finish_error(&mut error);
        }
    }
}

/// A compiled WebKit content-blocker list.
#[derive(Clone)]
pub struct WebKitContentFilter {
    id: Rc<str>,
    filter: UserContentFilter,
}

impl WebKitContentFilter {
    fn new(id: String, filter: UserContentFilter) -> Self {
        WebKitContentFilter {
            id: id.into(),
            filter,
        }
    }

    pub(crate) fn raw(&self) -> &UserContentFilter {
        &self.filter
    }
}

impl ContentFilter for WebKitContentFilter {
    fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for WebKitContentFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebKitContentFilter({:?})", self.id)
    }
}

/// Process-unique ids for downloads, permissions, scripts and style sheets.
pub(crate) fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The languages pages are told the user reads (`Accept-Language`,
/// `navigator.languages`, the default `Intl` locale), best first, as BCP 47
/// tags (`en-US`). Call before the first page: WebKit hands the list to each
/// web process as it starts. An empty list keeps WebKit's own choice.
pub fn set_preferred_languages(languages: &[String]) {
    if languages.is_empty() {
        return;
    }
    let Some(context) = webkit6::WebContext::default() else {
        return;
    };
    let tags: Vec<&str> = languages.iter().map(String::as_str).collect();
    context.set_preferred_languages(&tags);
}
