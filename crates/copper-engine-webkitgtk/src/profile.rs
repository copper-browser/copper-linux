//! Profiles: one `NetworkSession` each, plus the profile's downloads.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use copper_engine::{
    Cookie, DownloadEvent, DownloadId, DownloadState, EngineError, EnginePaths, EventSink,
    EventStream, LocalFuture, PageEvent, Profile, ProfileId, Result, WebsiteDataKinds,
};
use glib::prelude::*;
use webkit6::{CookiePersistentStorage, Download, DownloadError, NetworkSession, WebView};

use crate::convert;

/// A cookie/storage jar backed by a `NetworkSession`. Cheap to clone.
#[derive(Clone)]
pub struct WebKitProfile {
    inner: Rc<ProfileInner>,
}

pub(crate) struct ProfileInner {
    id: ProfileId,
    ephemeral: bool,
    data_dir: Option<PathBuf>,
    session: NetworkSession,
    downloads: Rc<Downloads>,
}

impl WebKitProfile {
    pub(crate) fn create(paths: &EnginePaths, id: &ProfileId, ephemeral: bool) -> Result<Self> {
        let (session, data_dir) = if ephemeral {
            (NetworkSession::new_ephemeral(), None)
        } else {
            let data = paths.profile_data_dir(id);
            let cache = paths.profile_cache_dir(id);
            create_private_dir(&data)?;
            std::fs::create_dir_all(&cache)?;
            let session = NetworkSession::new(
                Some(convert::path_str(&data)?),
                Some(convert::path_str(&cache)?),
            );
            // Cookies stay in memory unless a persistent store is set.
            if let Some(cm) = session.cookie_manager() {
                let db = data.join("cookies.sqlite");
                cm.set_persistent_storage(convert::path_str(&db)?, CookiePersistentStorage::Sqlite);
            }
            (session, Some(data))
        };
        if let Some(wdm) = session.website_data_manager() {
            wdm.set_favicons_enabled(true);
        }
        let downloads = Rc::new(Downloads {
            sink: EventSink::new(),
            active: RefCell::default(),
            reserved: RefCell::default(),
            pages: RefCell::default(),
            dir: RefCell::new(paths.downloads_dir.clone()),
        });
        let weak = Rc::downgrade(&downloads);
        session.connect_download_started(move |_, dl| {
            if let Some(d) = weak.upgrade() {
                Downloads::track(&d, dl);
            }
        });
        Ok(WebKitProfile {
            inner: Rc::new(ProfileInner {
                id: id.clone(),
                ephemeral,
                data_dir,
                session,
                downloads,
            }),
        })
    }

    pub(crate) fn from_inner(inner: Rc<ProfileInner>) -> Self {
        WebKitProfile { inner }
    }

    pub(crate) fn downgrade(&self) -> Weak<ProfileInner> {
        Rc::downgrade(&self.inner)
    }

    pub(crate) fn session(&self) -> &NetworkSession {
        &self.inner.session
    }

    /// Route downloads started by `view` to `sink` as `PageEvent::Download`.
    pub(crate) fn register_page(&self, view: &WebView, sink: Weak<EventSink<PageEvent>>) {
        let mut pages = self.inner.downloads.pages.borrow_mut();
        pages.retain(|(v, s)| v.upgrade().is_some() && s.strong_count() > 0);
        pages.push((view.downgrade(), sink));
    }

    /// Id of a download object, tracking it if the session has not yet.
    pub(crate) fn track_download(&self, dl: &Download) -> DownloadId {
        Downloads::track(&self.inner.downloads, dl)
    }
}

impl Profile for WebKitProfile {
    fn id(&self) -> &ProfileId {
        &self.inner.id
    }

    fn is_ephemeral(&self) -> bool {
        self.inner.ephemeral
    }

    fn data_dir(&self) -> Option<&Path> {
        self.inner.data_dir.as_deref()
    }

    fn cookies(&self, url: &str) -> LocalFuture<Result<Vec<Cookie>>> {
        let cm = self.inner.session.cookie_manager();
        let url = url.to_owned();
        Box::pin(async move {
            let cm = cm.ok_or_else(|| EngineError::Cookies("no cookie manager".into()))?;
            let list = cm
                .cookies_future(&url)
                .await
                .map_err(|e| EngineError::Cookies(e.to_string()))?;
            Ok(list.into_iter().map(convert::cookie_from_soup).collect())
        })
    }

    fn all_cookies(&self) -> LocalFuture<Result<Vec<Cookie>>> {
        let cm = self.inner.session.cookie_manager();
        Box::pin(async move {
            let cm = cm.ok_or_else(|| EngineError::Cookies("no cookie manager".into()))?;
            let list = cm
                .all_cookies_future()
                .await
                .map_err(|e| EngineError::Cookies(e.to_string()))?;
            Ok(list.into_iter().map(convert::cookie_from_soup).collect())
        })
    }

    fn set_cookie(&self, cookie: &Cookie) -> LocalFuture<Result<()>> {
        let cm = self.inner.session.cookie_manager();
        let sc = convert::cookie_to_soup(cookie);
        Box::pin(async move {
            let cm = cm.ok_or_else(|| EngineError::Cookies("no cookie manager".into()))?;
            cm.add_cookie_future(&sc?)
                .await
                .map_err(|e| EngineError::Cookies(e.to_string()))
        })
    }

    fn delete_cookie(&self, cookie: &Cookie) -> LocalFuture<Result<()>> {
        let cm = self.inner.session.cookie_manager();
        let sc = convert::cookie_to_soup(cookie);
        Box::pin(async move {
            let cm = cm.ok_or_else(|| EngineError::Cookies("no cookie manager".into()))?;
            cm.delete_cookie_future(&sc?)
                .await
                .map_err(|e| EngineError::Cookies(e.to_string()))
        })
    }

    fn clear_website_data(
        &self,
        domain: Option<&str>,
        kinds: WebsiteDataKinds,
    ) -> LocalFuture<Result<Vec<String>>> {
        let wdm = self.inner.session.website_data_manager();
        let domain = domain.map(str::to_owned);
        let types = convert::data_types(kinds);
        Box::pin(async move {
            let wdm = wdm.ok_or_else(|| EngineError::WebsiteData("no data manager".into()))?;
            if types.is_empty() {
                return Ok(Vec::new());
            }
            let records = wdm
                .fetch_future(types)
                .await
                .map_err(|e| EngineError::WebsiteData(format!("fetch: {e}")))?;
            let matching: Vec<_> = records
                .iter()
                .filter(|r| match (&domain, r.name()) {
                    (None, _) => true,
                    (Some(d), Some(name)) => convert::site_matches(&name, d),
                    (Some(_), None) => false,
                })
                .collect();
            let names: Vec<String> = matching
                .iter()
                .filter_map(|r| r.name().map(String::from))
                .collect();
            let (tx, rx) = futures_channel::oneshot::channel();
            if domain.is_none() {
                wdm.clear(
                    types,
                    glib::TimeSpan(0),
                    None::<&gio::Cancellable>,
                    move |r| {
                        let _ = tx.send(r);
                    },
                );
            } else if matching.is_empty() {
                return Ok(names);
            } else {
                wdm.remove(types, &matching, None::<&gio::Cancellable>, move |r| {
                    let _ = tx.send(r);
                });
            }
            rx.await
                .map_err(|_| EngineError::WebsiteData("remove was dropped".into()))?
                .map_err(|e| EngineError::WebsiteData(format!("remove: {e}")))?;
            Ok(names)
        })
    }

    fn download(&self, url: &str) -> Result<DownloadId> {
        let dl = self
            .inner
            .session
            .download_uri(url)
            .ok_or_else(|| EngineError::Download(format!("cannot download {url}")))?;
        Ok(self.track_download(&dl))
    }

    fn cancel_download(&self, id: DownloadId) -> bool {
        let dl = self.inner.downloads.active.borrow().get(&id).cloned();
        match dl {
            Some(dl) => {
                dl.cancel();
                true
            }
            None => false,
        }
    }

    fn download_dir(&self) -> PathBuf {
        self.inner.downloads.dir.borrow().clone()
    }

    fn set_download_dir(&self, dir: PathBuf) {
        *self.inner.downloads.dir.borrow_mut() = dir;
    }

    fn downloads(&self) -> EventStream<DownloadEvent> {
        self.inner.downloads.sink.subscribe()
    }
}

impl std::fmt::Debug for WebKitProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebKitProfile")
            .field("id", &self.inner.id)
            .field("ephemeral", &self.inner.ephemeral)
            .finish()
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    Ok(())
}

/// Download bookkeeping for one profile. Shared (weakly) with signal handlers.
struct Downloads {
    sink: EventSink<DownloadEvent>,
    active: RefCell<HashMap<DownloadId, Download>>,
    /// Destinations chosen but not finished, so two downloads never pick the same name.
    reserved: RefCell<HashSet<PathBuf>>,
    pages: RefCell<Vec<PageRoute>>,
    dir: RefCell<PathBuf>,
}

/// A page's view and the sink its download events go to.
type PageRoute = (glib::WeakRef<WebView>, Weak<EventSink<PageEvent>>);

/// Per-download state shared by its signal handlers.
struct Tracked {
    id: DownloadId,
    failed: Cell<bool>,
    path: RefCell<Option<PathBuf>>,
    last_progress: Cell<Option<Instant>>,
}

const PROGRESS_EVERY: Duration = Duration::from_millis(100);

impl Downloads {
    fn track(this: &Rc<Downloads>, dl: &Download) -> DownloadId {
        if let Some((id, _)) = this.active.borrow().iter().find(|(_, d)| *d == dl) {
            return *id;
        }
        let id = DownloadId(crate::next_id());
        this.active.borrow_mut().insert(id, dl.clone());
        let t = Rc::new(Tracked {
            id,
            failed: Cell::new(false),
            path: RefCell::new(None),
            last_progress: Cell::new(None),
        });
        let url = dl.request().and_then(|r| r.uri()).map(String::from);
        this.emit(dl, id, DownloadState::Started { url });

        let weak = Rc::downgrade(this);
        let tt = t.clone();
        dl.connect_decide_destination(move |dl, suggested| {
            let Some(this) = weak.upgrade() else {
                return false;
            };
            let dir = this.dir.borrow().clone();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!("download dir {}: {e}", dir.display());
            }
            let path = convert::unique_path(&dir, suggested, &this.reserved.borrow());
            this.reserved.borrow_mut().insert(path.clone());
            dl.set_destination(&path.to_string_lossy());
            *tt.path.borrow_mut() = Some(path.clone());
            let total = dl.response().map(|r| r.content_length()).filter(|n| *n > 0);
            this.emit(
                dl,
                tt.id,
                DownloadState::Destination {
                    path,
                    suggested_filename: Some(suggested.to_owned()).filter(|s| !s.is_empty()),
                    total,
                },
            );
            true
        });

        let weak = Rc::downgrade(this);
        let tt = t.clone();
        dl.connect_received_data(move |dl, _len| {
            let Some(this) = weak.upgrade() else { return };
            let now = Instant::now();
            if tt
                .last_progress
                .get()
                .is_some_and(|last| now.duration_since(last) < PROGRESS_EVERY)
            {
                return;
            }
            tt.last_progress.set(Some(now));
            let total = dl.response().map(|r| r.content_length()).filter(|n| *n > 0);
            this.emit(
                dl,
                tt.id,
                DownloadState::Progress {
                    received: dl.received_data_length(),
                    total,
                    fraction: dl.estimated_progress(),
                },
            );
        });

        let weak = Rc::downgrade(this);
        let tt = t.clone();
        dl.connect_failed(move |dl, err| {
            tt.failed.set(true);
            let Some(this) = weak.upgrade() else { return };
            let cancelled = err.kind::<DownloadError>() == Some(DownloadError::CancelledByUser);
            this.finish(tt.id, &tt);
            this.emit(
                dl,
                tt.id,
                DownloadState::Failed {
                    error: err.message().to_owned(),
                    cancelled,
                },
            );
        });

        let weak = Rc::downgrade(this);
        dl.connect_finished(move |dl| {
            // `finished` also follows `failed`.
            if t.failed.get() {
                return;
            }
            let Some(this) = weak.upgrade() else { return };
            this.finish(t.id, &t);
            let path = t
                .path
                .borrow()
                .clone()
                .or_else(|| dl.destination().map(|d| PathBuf::from(d.as_str())))
                .unwrap_or_default();
            this.emit(
                dl,
                t.id,
                DownloadState::Finished {
                    path,
                    bytes: dl.received_data_length(),
                },
            );
        });
        id
    }

    fn finish(&self, id: DownloadId, t: &Tracked) {
        self.active.borrow_mut().remove(&id);
        if let Some(p) = t.path.borrow().as_ref() {
            self.reserved.borrow_mut().remove(p);
        }
    }

    fn emit(&self, dl: &Download, id: DownloadId, state: DownloadState) {
        let ev = DownloadEvent { id, state };
        if let Some(sink) = dl.web_view().and_then(|v| self.page_sink(&v)) {
            sink.emit(PageEvent::Download(ev.clone()));
        }
        self.sink.emit(ev);
    }

    fn page_sink(&self, view: &WebView) -> Option<Rc<EventSink<PageEvent>>> {
        self.pages
            .borrow()
            .iter()
            .find(|(v, _)| v.upgrade().as_ref() == Some(view))
            .and_then(|(_, s)| s.upgrade())
    }
}
