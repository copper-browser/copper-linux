//! Pages: one live [`WebKitPage`] per awake tab, its frame on the stage, and
//! what its events do to the shell.
//!
//! A sleeping tab has no page at all (no WebView, no web process); it gets
//! one when it is selected or explicitly woken ([`Shell::ensure_page`]).

use super::{OpenTab, Shell, host_of};
use crate::control::ShellEvent;
use crate::window::Refresh;
use copper_core::TabId;
use copper_engine::{
    CrashReason, LoadState, NewWindowRequest, PageEvent, PageOptions, PageView, PermissionKind,
    WindowDisposition,
};
use copper_engine::{Engine, Profile};
use copper_engine_webkitgtk::WebKitPage;
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

/// A tab's live page and the frame it sits in on the stage.
pub struct PageSlot {
    pub id: TabId,
    pub page: WebKitPage,
    /// The stage's child for this tab: the page, with the error / crash
    /// state laid over it when there is one.
    pub frame: gtk::Overlay,
    status: gtk::Box,
    status_page: adw::StatusPage,
    status_button: gtk::Button,
    pub private: bool,
    pub loading: Cell<bool>,
    pub progress: Cell<f64>,
    pub audio: Cell<bool>,
    pub crashed: Cell<bool>,
    /// The address that failed to load, while its error shows.
    pub failed_url: RefCell<Option<String>>,
    /// The link under the pointer.
    pub hover: RefCell<Option<String>>,
    pump: RefCell<Option<glib::JoinHandle<()>>>,
}

impl PageSlot {
    fn new(shell: &Shell, id: &TabId, page: WebKitPage, private: bool) -> Rc<PageSlot> {
        let frame = gtk::Overlay::new();
        frame.set_hexpand(true);
        frame.set_vexpand(true);
        let view = page.widget();
        view.set_hexpand(true);
        view.set_vexpand(true);
        frame.set_child(Some(&view));

        let status_page = adw::StatusPage::new();
        status_page.add_css_class("compact");
        let status_button = gtk::Button::with_label("Reload");
        status_button.add_css_class("pill");
        status_button.set_halign(gtk::Align::Center);
        status_page.set_child(Some(&status_button));
        let status = gtk::Box::new(gtk::Orientation::Vertical, 0);
        status.add_css_class("copper-status");
        status.append(&status_page);
        status_page.set_vexpand(true);
        status.set_visible(false);
        frame.add_overlay(&status);

        let weak = shell.downgrade();
        let tab = id.clone();
        status_button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.reload(&tab);
            }
        });

        Rc::new(PageSlot {
            id: id.clone(),
            page,
            frame,
            status,
            status_page,
            status_button,
            private,
            loading: Cell::new(false),
            progress: Cell::new(0.0),
            audio: Cell::new(false),
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
        self.status.set_visible(false);
    }

    /// The web process went away (§7: "This page stopped", Reload).
    pub fn show_crash(&self, reason: CrashReason) {
        self.crashed.set(true);
        self.loading.set(false);
        self.status_page.set_title("This page stopped");
        self.status_page.set_description(Some(match reason {
            CrashReason::ExceededMemoryLimit => "It used more memory than Copper allows a page.",
            CrashReason::TerminatedByApi => "Its process was ended.",
            CrashReason::Crashed => "Its process quit unexpectedly. Copper is fine.",
        }));
        self.status_button.set_label("Reload");
        self.status.set_visible(true);
    }

    /// A load failed: the reason and Retry, in the page.
    pub fn show_failure(&self, url: &str, error: &str) {
        let offline = !gio::NetworkMonitor::default().is_network_available();
        self.failed_url.replace(Some(url.to_string()));
        self.loading.set(false);
        if offline {
            self.status_page.set_title("You’re offline");
            self.status_page
                .set_description(Some("Copper will load this page once you are connected."));
        } else {
            self.status_page
                .set_title(&format!("Couldn’t open {}", host_of(url)));
            self.status_page.set_description(Some(&sentence(error)));
        }
        self.status_button.set_label("Try Again");
        self.status.set_visible(true);
    }

    fn stop_pump(&self) {
        if let Some(h) = self.pump.borrow_mut().take() {
            h.abort();
        }
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

impl Shell {
    /// The tab's page, built (and loading its address) if it has none.
    pub fn ensure_page(&self, id: &TabId) -> Option<WebKitPage> {
        if let Some(p) = self.page(id) {
            return Some(p);
        }
        let (url, private, profile) = {
            let b = self.browser();
            let t = b.tab(id)?;
            (t.url.clone(), t.private, b.profile_of(id))
        };
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
    /// and load `url` if given.
    pub(crate) fn adopt_page(
        &self,
        id: &TabId,
        page: WebKitPage,
        url: Option<&str>,
        private: bool,
    ) {
        let slot = PageSlot::new(self, id, page.clone(), private);
        self.ui().stage.add_page(&slot);
        self.set_popup_handler(id, &page);

        let mut events = page.events();
        let weak = self.downgrade();
        let tab = id.clone();
        let slot_weak = Rc::downgrade(&slot);
        let handle = glib::spawn_future_local(async move {
            while let Some(ev) = events.next().await {
                let (Some(shell), Some(slot)) = (weak.upgrade(), slot_weak.upgrade()) else {
                    break;
                };
                shell.on_page_event(&tab, &slot, ev);
            }
        });
        slot.pump.replace(Some(handle));
        self.pages_map().borrow_mut().insert(id.clone(), slot);
        crate::features::page_created(self, id, &page);
        if let Some(url) = url {
            page.load(url);
        }
    }

    /// Drop a tab's page (it went to sleep, closed, or changes profile).
    pub(crate) fn drop_page(&self, id: &TabId) {
        let slot = self.pages_map().borrow_mut().remove(id);
        if let Some(slot) = slot {
            slot.stop_pump();
            if let Some(ui) = self.ui_opt() {
                ui.stage.remove_page(&slot);
            }
        }
    }

    /// Rebuild the pages of the tabs `pick` chooses (a profile changed):
    /// they sleep, and the selected one wakes in its new jar.
    pub(crate) fn recycle_pages(&self, pick: impl Fn(&copper_core::Browser, &TabId) -> bool) {
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

    fn ui_opt(&self) -> Option<&crate::window::Ui> {
        self.0.ui.get()
    }

    fn host_icons(&self) -> &RefCell<std::collections::HashMap<String, Option<gdk::Texture>>> {
        &self.0.host_icons
    }

    fn set_popup_handler(&self, opener: &TabId, page: &WebKitPage) {
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
    fn adopt_popup(&self, opener: &TabId, page: WebKitPage, req: &NewWindowRequest) {
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
        glib::idle_add_local_once(move || {
            if let Some(s) = weak.upgrade() {
                s.flush();
            }
        });
    }

    pub(crate) fn on_page_event(&self, id: &TabId, slot: &Rc<PageSlot>, ev: PageEvent) {
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
                    self.ui().refresh(self, &Refresh::chrome());
                }
            }
            PageEvent::Url(_) => {}
            PageEvent::Favicon(image) => {
                let texture = image.and_then(|img| {
                    gdk::Texture::from_bytes(&glib::Bytes::from(&img.png))
                        .ok()
                        .map(|t| (t, img.png))
                });
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
                self.ui().refresh(self, &Refresh::tab(id));
            }
            PageEvent::Progress(p) => {
                slot.progress.set(p);
                if active {
                    self.ui().stage.set_progress(slot.loading.get(), p);
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
                    self.ui().stage.set_progress(false, 0.0);
                }
                self.ui().refresh(self, &Refresh::tab(id));
                self.0.hub.send(ShellEvent::Crashed(id.clone()));
            }
            PageEvent::CloseRequested => {
                // `window.close()`: back to the opener if this was on screen.
                // A Saved row is only unloaded; it stays saved.
                self.close_keeping_saved(id);
            }
            PageEvent::PermissionRequest { id: pid, kind, url } => {
                self.ask_permission(slot, pid, kind, url.as_deref());
            }
            PageEvent::LinkHover(h) => {
                slot.hover.replace(h.map(|h| h.url));
                if active {
                    self.ui().stage.set_hover(slot.hover.borrow().as_deref());
                }
            }
            PageEvent::AudioPlaying(on) => {
                slot.audio.set(on);
                self.ui().refresh(self, &Refresh::tab(id));
            }
            PageEvent::Download(_) | PageEvent::Console(_) | PageEvent::ScriptMessage { .. } => {}
        }
    }

    fn on_load(&self, id: &TabId, slot: &Rc<PageSlot>, state: LoadState, active: bool) {
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
                slot.loading.set(false);
                slot.progress.set(1.0);
                self.0.hub.send(ShellEvent::Loaded(id.clone()));
            }
            LoadState::Failed {
                url,
                error,
                cancelled,
            } => {
                slot.loading.set(false);
                if !cancelled {
                    slot.show_failure(&url, &error);
                }
            }
        }
        if active {
            self.ui()
                .stage
                .set_progress(slot.loading.get(), slot.progress.get());
            self.ui().refresh(self, &Refresh::chrome());
        }
        self.ui().refresh(self, &Refresh::tab(id));
    }

    /// A page wants the camera, the location, …: ask in one sentence.
    fn ask_permission(
        &self,
        slot: &Rc<PageSlot>,
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
        let dialog = adw::AlertDialog::new(Some(&format!("Allow {host} to {what}?")), None);
        dialog.add_response("deny", "Don’t Allow");
        dialog.add_response("allow", "Allow");
        dialog.set_default_response(Some("deny"));
        dialog.set_close_response("deny");
        let page = slot.page.clone();
        dialog.connect_response(None, move |_, response| {
            page.answer_permission(pid, response == "allow");
        });
        dialog.present(Some(self.window()));
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
    pub(crate) fn open_icon(&self, url: &str) -> Option<gdk::Texture> {
        let id = self
            .browser()
            .all_tabs()
            .find(|t| t.url == url)
            .map(|t| t.id.clone())?;
        self.favicons().borrow().get(&id).cloned()
    }

    /// The icon kept for a page: its own part of the site, else the site's
    /// front page; never another part's (the Maps pin on a Search tab).
    pub(crate) fn cached_icon(&self, url: &str) -> Option<gdk::Texture> {
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

    fn kept_icon(&self, rel: &std::path::Path) -> Option<gdk::Texture> {
        let key = rel.to_string_lossy().to_string();
        if let Some(hit) = self.host_icons().borrow().get(&key) {
            return hit.clone();
        }
        let path = self.icon_path(rel);
        let tex = path
            .exists()
            .then(|| gdk::Texture::from_filename(&path).ok())
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
pub(crate) fn icon_rel(url: &str) -> Option<PathBuf> {
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
        let rel = |u: &str| icon_rel(u).map(|p| p.to_string_lossy().to_string());
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
