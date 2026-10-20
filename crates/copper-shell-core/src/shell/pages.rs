//! Pages: one live engine page per awake tab, its frame on the stage (made
//! by the view, [`crate::view::PageFrame`]), and what its events do to the
//! shell.
//!
//! A sleeping tab has no page at all (no WebView, no web process); it gets
//! one when it is selected or explicitly woken ([`Shell::ensure_page`]).

use super::{OpenTab, Shell, host_of};
use crate::control::ShellEvent;
use crate::main_loop::{MainLoop, Task};
use crate::view::{PageFrame, PageStatus, Question, Refresh, ShellView};
use crate::{PageOf, Platform};
use copper_core::TabId;
use copper_engine::{
    CrashReason, LoadState, NewWindowRequest, PageEvent, PageOptions, PageView, PermissionKind,
    WindowDisposition,
};
use copper_engine::{Engine, Profile};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// A tab's live page and the frame it sits in on the stage.
pub struct PageSlot<P: Platform> {
    pub id: TabId,
    pub page: PageOf<P>,
    /// The stage's child for this tab: the page, with the error / crash
    /// state laid over it when there is one.
    pub frame: P::Frame,
    pub private: bool,
    pub loading: Cell<bool>,
    pub progress: Cell<f64>,
    pub audio: Cell<bool>,
    /// This document asked for the camera, the microphone or the screen:
    /// it may be on a call, so it never sleeps (Mac `Sleep.swift` "on a
    /// call"; the engines say what was asked, not whether it is in use).
    pub capture: Cell<bool>,
    /// Downloads this page started that are still going.
    pub downloads: RefCell<std::collections::HashSet<copper_engine::DownloadId>>,
    pub crashed: Cell<bool>,
    /// The address that failed to load, while its error shows.
    pub failed_url: RefCell<Option<String>>,
    /// The link under the pointer.
    pub hover: RefCell<Option<String>>,
    pump: RefCell<Option<Task>>,
}

impl<P: Platform> PageSlot<P> {
    fn new(shell: &Shell<P>, id: &TabId, page: PageOf<P>, private: bool) -> Rc<PageSlot<P>> {
        let frame = shell.view().new_frame(shell, id, &page);
        Rc::new(PageSlot {
            id: id.clone(),
            page,
            frame,
            private,
            loading: Cell::new(false),
            progress: Cell::new(0.0),
            audio: Cell::new(false),
            capture: Cell::new(false),
            downloads: RefCell::default(),
            crashed: Cell::new(false),
            failed_url: RefCell::new(None),
            hover: RefCell::new(None),
            pump: RefCell::new(None),
        })
    }

    /// Hide the error / crash state.
    pub fn clear_status(&self) {
        self.crashed.set(false);
        self.failed_url.replace(None);
        self.frame.hide_status();
    }

    /// The web process went away (§7: "This page stopped", Reload).
    pub fn show_crash(&self, reason: CrashReason) {
        self.crashed.set(true);
        self.loading.set(false);
        self.frame.show_status(&crash_status(reason));
    }

    /// A load failed: the reason and Retry, in the page (`online`: whether
    /// the network is reachable, [`ShellView::is_online`]).
    pub fn show_failure(&self, url: &str, error: &str, online: bool) {
        self.failed_url.replace(Some(url.to_string()));
        self.loading.set(false);
        self.frame.show_status(&failure_status(url, error, online));
    }

    fn stop_pump(&self) {
        if let Some(h) = self.pump.borrow_mut().take() {
            h.abort();
        }
    }
}

/// What a crashed page says (§7).
fn crash_status(reason: CrashReason) -> PageStatus {
    PageStatus {
        title: "This page stopped".into(),
        description: match reason {
            CrashReason::ExceededMemoryLimit => "It used more memory than Copper allows a page.",
            CrashReason::TerminatedByApi => "Its process was ended.",
            CrashReason::Crashed => "Its process quit unexpectedly. Copper is fine.",
        }
        .into(),
        button: "Reload".into(),
    }
}

/// What a page that failed to load says.
fn failure_status(url: &str, error: &str, online: bool) -> PageStatus {
    let (title, description) = if online {
        (format!("Couldn’t open {}", host_of(url)), sentence(error))
    } else {
        (
            "You’re offline".to_string(),
            "Copper will load this page once you are connected.".to_string(),
        )
    };
    PageStatus {
        title,
        description,
        button: "Try Again".into(),
    }
}

/// WebKit's error text as one sentence.
fn sentence(error: &str) -> String {
    let e = error.trim().trim_end_matches('.');
    if e.is_empty() {
        return "The page could not be loaded.".into();
    }
    let mut c = e.chars();
    let first = c
        .next()
        .map(|f| f.to_uppercase().collect::<String>())
        .unwrap_or_default();
    format!("{first}{}.", c.as_str())
}

/// A profile id the engine accepts, from a Mac profile name (`Work`,
/// `Client A`): `[A-Za-z0-9._-]`, never leading `.`.
pub fn profile_id(name: Option<&str>) -> copper_engine::ProfileId {
    let raw = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("default");
    let clean: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim_start_matches('.').to_string();
    let clean = if clean.is_empty() {
        "default".to_string()
    } else {
        clean
    };
    let clean: String = clean.chars().take(128).collect();
    copper_engine::ProfileId::new(clean)
        .unwrap_or_else(|_| copper_engine::ProfileId::new("default").expect("valid id"))
}

impl<P: Platform> Shell<P> {
    /// The tab's page, built (and loading its address) if it has none.
    pub fn ensure_page(&self, id: &TabId) -> Option<PageOf<P>> {
        if let Some(p) = self.page(id) {
            return Some(p);
        }
        let (url, private, profile, sleeping) = {
            let b = self.browser();
            let t = b.tab(id)?;
            (t.url.clone(), t.private, b.profile_of(id), t.sleeping)
        };
        // A tab that gets a page is awake, and was just used: left marked
        // asleep, its next change would take the page away again (and a
        // caller waiting on it would rebuild it, again and again); left with
        // a stale `seen` (a tab restored from the session), the idle sleeper
        // would put it back to sleep within a minute.
        if sleeping {
            self.browser_mut().update_tab(id, |t| {
                t.sleeping = false;
                t.seen = copper_core::date::UnixTime::now();
            });
        }
        // Woken: its last picture stands in until the page has drawn.
        self.wake_cover(id);
        let profile = if private {
            self.engine().profile(&crate::private::profile_id(id), true)
        } else {
            self.engine()
                .profile(&profile_id(profile.as_deref()), false)
        };
        let profile = match profile {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "profile unavailable; using the default");
                self.engine().profile(&profile_id(None), false).ok()?
            }
        };
        self.0.downloads.attach(self, &profile);
        let page = self.engine().new_page(&profile, page_options());
        let load = (!url.is_empty()).then_some(url);
        self.adopt_page(id, page.clone(), load.as_deref(), private);
        Some(page)
    }

    /// Give `page` to tab `id`: frame it, put it on the stage, listen to it,
    /// run the [`Shell::on_page_created`] hooks, and load `url` if given.
    pub fn adopt_page(&self, id: &TabId, page: PageOf<P>, url: Option<&str>, private: bool) {
        let slot = PageSlot::new(self, id, page.clone(), private);
        self.view().page_added(self, &slot);
        self.set_popup_handler(id, &page);

        let mut events = page.events();
        let weak = self.downgrade();
        let tab = id.clone();
        let slot_weak = Rc::downgrade(&slot);
        let handle = crate::main_loop::spawn::<P::Loop>(async move {
            while let Some(ev) = events.next().await {
                let (Some(shell), Some(slot)) = (weak.upgrade(), slot_weak.upgrade()) else {
                    break;
                };
                shell.on_page_event(&tab, &slot, ev);
            }
        });
        slot.pump.replace(Some(handle));
        self.pages_map().borrow_mut().insert(id.clone(), slot);
        let hooks = self.0.page_hooks.borrow().clone();
        for hook in &hooks {
            hook(self, id, &page);
        }
        if let Some(url) = url {
            page.load(url);
        }
    }

    /// Drop a tab's page (it went to sleep, closed, or changes profile).
    pub fn drop_page(&self, id: &TabId) {
        let slot = self.pages_map().borrow_mut().remove(id);
        if let Some(slot) = slot {
            slot.stop_pump();
            self.view().page_removed(self, &slot);
        }
    }

    /// Rebuild the pages of the tabs `pick` chooses (a profile changed):
    /// they sleep, and the selected one wakes in its new jar.
    pub fn recycle_pages(&self, pick: impl Fn(&copper_core::Browser, &TabId) -> bool) {
        let ids: Vec<TabId> = {
            let b = self.browser();
            self.pages_map()
                .borrow()
                .keys()
                .filter(|id| pick(&b, id))
                .cloned()
                .collect()
        };
        if ids.is_empty() {
            return;
        }
        let active = self.active_id();
        for id in &ids {
            self.drop_page(id);
            if Some(id) != active.as_ref() {
                self.browser_mut().set_sleeping(id, true);
            }
        }
        self.flush();
        self.show_active();
    }

    fn host_icons(&self) -> &RefCell<std::collections::HashMap<String, Option<P::Icon>>> {
        &self.0.host_icons
    }

    fn set_popup_handler(&self, opener: &TabId, page: &PageOf<P>) {
        let weak = self.downgrade();
        let opener = opener.clone();
        page.set_new_window_handler(Some(Box::new(move |req: &NewWindowRequest| {
            if req.disposition != WindowDisposition::Popup {
                return None;
            }
            let shell = weak.upgrade()?;
            let parent = shell.page(&opener)?;
            let related = parent.new_related(page_options());
            shell.adopt_popup(&opener, related.clone(), req);
            Some(related)
        })));
    }

    /// A popup that keeps its `window.opener` (sign-in windows): a tab
    /// right after the opener, in front.
    fn adopt_popup(&self, opener: &TabId, page: PageOf<P>, req: &NewWindowRequest) {
        let url = req
            .url
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "about:blank".into());
        let private = self.browser().tab(opener).is_some_and(|t| t.private);
        let id = {
            let mut b = self.browser_mut();
            let index = b.tab(opener).and_then(|t| {
                if t.is_pinned() {
                    Some(0)
                } else {
                    b.row(&t.space)
                        .iter()
                        .position(|r| r.id == *opener)
                        .map(|p| p + 1)
                }
            });
            b.open_tab(
                &url,
                copper_core::OpenOptions {
                    foreground: true,
                    private,
                    index,
                    ..Default::default()
                },
            )
        };
        self.set_opener(&id, opener);
        self.adopt_page(&id, page, None, private);
        // Not from inside WebKit's `create` signal: the redraw can wait a turn.
        let weak = self.downgrade();
        P::Loop::idle(move || {
            if let Some(s) = weak.upgrade() {
                s.flush();
            }
        });
    }

    /// What one event of tab `id`'s page does to the shell.
    pub fn on_page_event(&self, id: &TabId, slot: &Rc<PageSlot<P>>, ev: PageEvent) {
        if let PageEvent::Load(state) = &ev {
            tracing::debug!(tab = %id.short(), ?state, "load");
        }
        let active = self.active_id().as_ref() == Some(id);
        match ev {
            PageEvent::Title(title) => {
                let title = title.unwrap_or_default();
                // A title cleared outside a load (the process went away)
                // keeps the one the tab had.
                if title.is_empty() && (slot.crashed.get() || !slot.loading.get()) {
                    return;
                }
                let url = self.browser().tab(id).map(|t| t.url.clone());
                self.browser_mut().set_title(id, &title);
                if !slot.private
                    && let Some(url) = url
                {
                    self.history_mut().retitle(&url, &title);
                    self.history_changed();
                }
                self.flush();
            }
            PageEvent::Url(Some(url)) if !url.is_empty() && url != "about:blank" => {
                self.browser_mut().set_url(id, &url);
                self.flush();
                if active {
                    self.view().refresh(self, &Refresh::chrome());
                }
            }
            PageEvent::Url(_) => {}
            PageEvent::Favicon(image) => {
                let texture =
                    image.and_then(|img| self.view().decode_icon(&img.png).map(|t| (t, img.png)));
                match texture {
                    Some((t, png)) => {
                        self.favicons().borrow_mut().insert(id.clone(), t);
                        if !slot.private {
                            let url = self.browser().tab(id).map(|t| t.url.clone());
                            if let Some(url) = url {
                                self.remember_icon(&url, &png);
                            }
                        }
                    }
                    None => {
                        self.favicons().borrow_mut().remove(id);
                    }
                }
                self.view().refresh(self, &Refresh::tab(id));
            }
            PageEvent::Progress(p) => {
                slot.progress.set(p);
                if active {
                    self.view().set_progress(slot.loading.get(), p);
                }
            }
            PageEvent::Load(state) => self.on_load(id, slot, state, active),
            PageEvent::NewWindow(req) => {
                let Some(url) = req.url.filter(|u| !u.is_empty() && u != "about:blank") else {
                    return;
                };
                let private = slot.private;
                let opened = self.open(
                    &url,
                    OpenTab {
                        foreground: req.disposition == WindowDisposition::Popup,
                        private,
                        after: Some(id.clone()),
                        ..Default::default()
                    },
                );
                self.set_opener(&opened, id);
            }
            PageEvent::Crashed(reason) => {
                self.uncover(id, std::time::Duration::ZERO);
                let url = self
                    .browser()
                    .tab(id)
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                slot.show_crash(reason);
                if reason != CrashReason::TerminatedByApi || !slot.private {
                    crate::crash::record(self, id, &url, reason);
                }
                if active {
                    self.view().set_progress(false, 0.0);
                }
                self.view().refresh(self, &Refresh::tab(id));
                self.0.hub.send(ShellEvent::Crashed(id.clone()));
            }
            PageEvent::CloseRequested => {
                // `window.close()`: back to the opener if this was on screen.
                // A Saved row is only unloaded; it stays saved.
                self.close_keeping_saved(id);
            }
            PageEvent::PermissionRequest { id: pid, kind, url } => {
                if matches!(
                    kind,
                    PermissionKind::Camera
                        | PermissionKind::Microphone
                        | PermissionKind::CameraAndMicrophone
                        | PermissionKind::DisplayCapture
                ) {
                    slot.capture.set(true);
                }
                self.ask_permission(slot, pid, kind, url.as_deref());
            }
            PageEvent::LinkHover(h) => {
                slot.hover.replace(h.map(|h| h.url));
                if active {
                    self.view().set_hover(slot.hover.borrow().as_deref());
                }
            }
            PageEvent::AudioPlaying(on) => {
                slot.audio.set(on);
                self.view().refresh(self, &Refresh::tab(id));
            }
            PageEvent::Download(d) => {
                let going = !matches!(
                    d.state,
                    copper_engine::DownloadState::Finished { .. }
                        | copper_engine::DownloadState::Failed { .. }
                );
                if going {
                    slot.downloads.borrow_mut().insert(d.id);
                } else {
                    slot.downloads.borrow_mut().remove(&d.id);
                }
            }
            PageEvent::Console(_) | PageEvent::ScriptMessage { .. } => {}
        }
    }

    fn on_load(&self, id: &TabId, slot: &Rc<PageSlot<P>>, state: LoadState, active: bool) {
        match state {
            LoadState::Started { url } => {
                slot.loading.set(true);
                slot.progress.set(0.08);
                // WebKit loads its own error page for the failed address;
                // only somewhere else clears the message.
                let failed = slot.failed_url.borrow().clone();
                let elsewhere = failed.is_none() || url.is_some() && url != failed;
                if (failed.is_some() || slot.crashed.get()) && elsewhere {
                    slot.clear_status();
                }
            }
            LoadState::Redirected { .. } => {}
            LoadState::Committed { url } => {
                // A new document: whatever the old one asked for is over;
                // a wake cover comes off once it has had time to paint.
                slot.capture.set(false);
                self.uncover(id, crate::cover::AFTER_COMMIT);
                if let Some(url) = url
                    && !slot.private
                {
                    let title = self
                        .browser()
                        .tab(id)
                        .map(|t| t.title.clone())
                        .unwrap_or_default();
                    if self.history_mut().record(&url, &title) {
                        self.history_changed();
                    }
                }
            }
            LoadState::Finished { .. } => {
                self.uncover(id, std::time::Duration::ZERO);
                slot.loading.set(false);
                slot.progress.set(1.0);
                self.0.hub.send(ShellEvent::Loaded(id.clone()));
            }
            LoadState::Failed {
                url,
                error,
                cancelled,
            } => {
                self.uncover(id, std::time::Duration::ZERO);
                slot.loading.set(false);
                if !cancelled {
                    slot.show_failure(&url, &error, self.view().is_online());
                }
            }
        }
        if active {
            self.view()
                .set_progress(slot.loading.get(), slot.progress.get());
            self.view().refresh(self, &Refresh::chrome());
        }
        self.view().refresh(self, &Refresh::tab(id));
    }

    /// A page wants the camera, the location, …: ask in one sentence.
    fn ask_permission(
        &self,
        slot: &Rc<PageSlot<P>>,
        pid: copper_engine::PermissionId,
        kind: PermissionKind,
        url: Option<&str>,
    ) {
        let what = match &kind {
            PermissionKind::Geolocation => "know your location",
            PermissionKind::Notifications => "show notifications",
            PermissionKind::Camera => "use your camera",
            PermissionKind::Microphone => "use your microphone",
            PermissionKind::CameraAndMicrophone => "use your camera and microphone",
            PermissionKind::DisplayCapture => "see your screen",
            PermissionKind::Clipboard => "read your clipboard",
            PermissionKind::DeviceInfo => "list your cameras and microphones",
            PermissionKind::PointerLock => "hide and lock the pointer",
            PermissionKind::MediaKeySystem => "play protected media",
            PermissionKind::WebsiteDataAccess { .. } => "use its cookies on this page",
            PermissionKind::Xr => "use your VR or AR device",
            PermissionKind::Other(_) => "use a device",
        };
        let host = url.map(host_of).unwrap_or_else(|| "This page".into());
        let question = Question {
            title: format!("Allow {host} to {what}?"),
            detail: None,
            deny: "Don’t Allow".into(),
            allow: "Allow".into(),
        };
        let page = slot.page.clone();
        self.view().ask(
            self,
            question,
            Box::new(move |allowed| {
                page.answer_permission(pid, allowed);
            }),
        );
    }

    fn icon_path(&self, rel: &std::path::Path) -> PathBuf {
        self.cache_dir().join("favicons").join(rel)
    }

    /// Keep a page's icon for next time (sleeping tabs show it at launch).
    fn remember_icon(&self, url: &str, png: &[u8]) {
        let Some(rel) = icon_rel(url) else {
            return;
        };
        let path = self.icon_path(&rel);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, png) {
            tracing::debug!(error = %e, "could not keep the favicon");
        }
        self.host_icons()
            .borrow_mut()
            .remove(&rel.to_string_lossy().to_string());
    }

    /// The live icon of an open tab showing exactly `url`.
    pub fn open_icon(&self, url: &str) -> Option<P::Icon> {
        let id = self
            .browser()
            .all_tabs()
            .find(|t| t.url == url)
            .map(|t| t.id.clone())?;
        self.favicons().borrow().get(&id).cloned()
    }

    /// The icon kept for a page: its own part of the site, else the site's
    /// front page; never another part's (the Maps pin on a Search tab).
    pub fn cached_icon(&self, url: &str) -> Option<P::Icon> {
        let rel = icon_rel(url)?;
        let front = rel.with_file_name(format!("{FRONT}.png"));
        self.kept_icon(&rel).or_else(|| {
            if front == rel {
                None
            } else {
                self.kept_icon(&front)
            }
        })
    }

    fn kept_icon(&self, rel: &std::path::Path) -> Option<P::Icon> {
        let key = rel.to_string_lossy().to_string();
        if let Some(hit) = self.host_icons().borrow().get(&key) {
            return hit.clone();
        }
        let path = self.icon_path(rel);
        let tex = path
            .exists()
            .then(|| self.view().load_icon(&path))
            .flatten();
        self.host_icons().borrow_mut().insert(key, tex.clone());
        tex
    }
}

/// The file name of a site's front page in the icon cache.
const FRONT: &str = "_";

/// Where a page's icon is kept, under the cache's `favicons/`: the site
/// (without `www.`, with its port) and the first part of the path, so
/// `google.com/maps` and `google.com/search` — one host, different
/// products, different icons — each keep their own. `None` for pages
/// without a host.
pub fn icon_rel(url: &str) -> Option<PathBuf> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let site = match u.port() {
        Some(p) => format!("{host}_{p}"),
        None => host.to_string(),
    };
    let site = file_safe(&site, 253);
    if site.is_empty() {
        return None;
    }
    let first = u
        .path_segments()
        .and_then(|mut s| s.next())
        .unwrap_or_default()
        .to_lowercase();
    let first = file_safe(&first, 64);
    let first = if first.is_empty() { FRONT } else { &first };
    Some(PathBuf::from(site).join(format!("{first}.png")))
}

/// `[a-z0-9.-]` kept, anything else `_`, never a leading dot, at most `max`.
fn file_safe(text: &str, max: usize) -> String {
    let clean: String = text
        .chars()
        .take(max)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    match clean.strip_prefix('.') {
        Some(rest) => format!("_{rest}"),
        None => clean,
    }
}

fn page_options() -> PageOptions {
    PageOptions {
        developer_extras: true,
        ..Default::default()
    }
}

/// What a profile's downloads stream needs to know its profile by.
pub fn profile_key<P: Profile>(p: &P) -> String {
    format!(
        "{}{}",
        p.id().as_str(),
        if p.is_ephemeral() { "#private" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ids_from_mac_names() {
        assert_eq!(profile_id(None).as_str(), "default");
        assert_eq!(profile_id(Some("Work")).as_str(), "Work");
        assert_eq!(profile_id(Some("Client A/B")).as_str(), "Client_A_B");
        assert_eq!(profile_id(Some("..hidden")).as_str(), "hidden");
        assert_eq!(profile_id(Some("  ")).as_str(), "default");
    }

    #[test]
    fn icons_are_kept_per_part_of_a_site() {
        // Compared with `/` on every system (Windows joins with `\\`).
        let rel = |u: &str| icon_rel(u).map(|p| p.to_string_lossy().replace('\\', "/"));
        assert_eq!(
            rel("https://www.google.com/maps/@1,2,3z").as_deref(),
            Some("google.com/maps.png")
        );
        assert_eq!(
            rel("https://www.google.com/search?q=webkitgtk").as_deref(),
            Some("google.com/search.png")
        );
        assert_eq!(
            rel("https://google.com/").as_deref(),
            Some("google.com/_.png")
        );
        assert_eq!(
            rel("https://docs.google.com/Document/d/1").as_deref(),
            Some("docs.google.com/document.png")
        );
        assert_eq!(
            rel("http://127.0.0.1:8765/forms.html").as_deref(),
            Some("127.0.0.1_8765/forms.html.png")
        );
        assert_eq!(
            rel("https://example.com/../x").as_deref(),
            Some("example.com/x.png")
        );
        assert_eq!(
            rel("https://example.com/.hidden/x").as_deref(),
            Some("example.com/_hidden.png")
        );
        assert_eq!(
            rel("https://example.com/a%2F..%2Fb").as_deref(),
            Some("example.com/a_2f.._2fb.png")
        );
        assert_eq!(rel("about:blank"), None);
        assert_eq!(rel("file:///home/x.html"), None);
        assert_eq!(rel("copper://canvas/1").as_deref(), Some("canvas/1.png"));
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            sentence("could not resolve host"),
            "Could not resolve host."
        );
        assert_eq!(sentence(""), "The page could not be loaded.");
    }
}
