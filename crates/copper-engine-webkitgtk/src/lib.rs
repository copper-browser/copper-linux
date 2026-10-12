//! WebKitGTK 6.0 implementation of copper-engine; owns every webkit6 call.
//!
//! [`WebKitEngine`] implements [`copper_engine::Engine`]; its profiles are
//! [`WebKitProfile`] (one `NetworkSession` each), its pages [`WebKitPage`]
//! (one `WebView` + `UserContentManager` each) and its compiled filters
//! [`WebKitContentFilter`]. The only GTK type in the public API is
//! [`WebKitPage::widget`], which the shell needs to put the page on screen.
//!
//! Everything here is main-thread only: create the engine after
//! `gtk4::init()` on the thread that runs the GTK main loop, and poll the
//! returned futures on that loop.

mod convert;
mod page;
mod profile;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use copper_engine::{
    ContentFilter, Engine, EngineError, EnginePaths, LocalFuture, PageOptions, ProfileId, Result,
};
use webkit6::{UserContentFilter, UserContentFilterError, UserContentFilterStore};

pub use page::WebKitPage;
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
}

impl WebKitEngine {
    /// An engine storing its state under `paths`. Call after `gtk4::init()`.
    pub fn new(paths: EnginePaths) -> Self {
        WebKitEngine {
            inner: Rc::new(EngineInner {
                paths,
                profiles: RefCell::default(),
                filters: RefCell::default(),
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
        WebKitPage::create(profile, &opts, None)
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
