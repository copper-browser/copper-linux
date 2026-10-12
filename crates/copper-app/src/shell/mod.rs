//! The shell: copper-core's state plus every operation the window, the
//! actions and the [`crate::control`] API perform.
//!
//! One [`Shell`] per process (Copper has one window). It owns the
//! [`Browser`] (spaces, tabs, pins, selection), [`History`], [`Bookmarks`],
//! [`Settings`], the debounced [`Saver`], the WebKit engine and one
//! [`pages::PageSlot`] per tab that has a live page. Every mutation goes
//! through a method here and ends in [`Shell::flush`], which reads the
//! browser's change events, redraws what changed, wakes or drops pages,
//! writes the session (debounced) and tells [`crate::control`] watchers.
//!
//! Main thread only (`Rc`); other threads reach it through
//! [`crate::control::Control`].

pub mod pages;

use crate::actions::Registry;
use crate::control::{Hub, ShellEvent};
use crate::downloads::Downloads;
use crate::options::Options;
use crate::session;
use crate::spaces::colour::SpaceColour;
use crate::window::{Refresh, Ui};
use copper_core::browser::{BrowserEvent, CloseOutcome};
use copper_core::history::Suggestion;
use copper_core::settings::{Look, TabsPosition, keys};
use copper_core::store::Store;
use copper_core::{Bookmarks, Browser, History, OpenOptions, Saver, Settings, TabId, UnixTime};
use copper_engine::PageView;
use copper_engine_webkitgtk::{WebKitEngine, WebKitPage};
use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use pages::PageSlot;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

/// The shell (cheap to clone; main thread only).
#[derive(Clone)]
pub struct Shell(Rc<Inner>);

/// A weak handle, for closures that widgets and pages keep.
#[derive(Clone)]
pub struct WeakShell(Weak<Inner>);

impl WeakShell {
    pub fn upgrade(&self) -> Option<Shell> {
        self.0.upgrade().map(Shell)
    }
}

pub struct Inner {
    pub app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub engine: WebKitEngine,
    pub store: Store,
    pub saver: Saver,
    pub options: Options,
    browser: RefCell<Browser>,
    history: RefCell<History>,
    bookmarks: RefCell<Bookmarks>,
    settings: RefCell<Settings>,
    pages: RefCell<HashMap<TabId, Rc<PageSlot>>>,
    favicons: RefCell<HashMap<TabId, gdk::Texture>>,
    host_icons: RefCell<HashMap<String, Option<gdk::Texture>>>,
    ui: OnceCell<Ui>,
    pub registry: RefCell<Registry>,
    pub hub: Hub,
    pub downloads: Downloads,
    cache_dir: PathBuf,
    restored: Cell<bool>,
    saved_generation: Cell<u64>,
    flushing: Cell<bool>,
    again: Cell<bool>,
    history_dirty: Cell<bool>,
    settings_dirty: Cell<bool>,
    timers: RefCell<Vec<glib::SourceId>>,
    shut: Cell<bool>,
}

/// How to open a tab.
#[derive(Debug, Clone, Default)]
pub struct OpenTab {
    pub title: String,
    pub foreground: bool,
    pub private: bool,
    /// The space (default: current).
    pub space: Option<String>,
    pub at_end: bool,
    /// Right after this tab (popups, Ctrl-click).
    pub after: Option<TabId>,
}

/// What the tab lists draw for one tab.
#[derive(Clone, Debug)]
pub struct TabView {
    pub id: TabId,
    pub label: String,
    pub url: String,
    pub host: String,
    pub pin: Option<String>,
    pub letter: String,
    pub sleeping: bool,
    pub private: bool,
    pub saved: bool,
    pub live: bool,
    pub loading: bool,
    pub audio: bool,
    pub crashed: bool,
    pub favicon: Option<gdk::Texture>,
}

/// The current space's tabs, as the column lays them out.
#[derive(Clone, Debug, Default)]
pub struct Visible {
    pub pins: Vec<TabView>,
    /// Loose tabs you keep (Saved), in row order.
    pub saved: Vec<TabView>,
    /// Loose tabs opened today, in row order.
    pub today: Vec<TabView>,
}

impl Visible {
    /// Pins, then the row (saved then today) — the order ⌘1–9 counts.
    pub fn all(&self) -> impl Iterator<Item = &TabView> {
        self.pins.iter().chain(&self.saved).chain(&self.today)
    }
}

impl Shell {
    /// Build the shell and its window, restore the session, show the window.
    pub fn new(app: &adw::Application, options: Options) -> Shell {
        let store = Store::from_env();
        let saver = Saver::new(store.clone());
        let loaded = session::load(&store);
        let cache_dir = session::cache_dir(&store);
        let engine = WebKitEngine::new(session::engine_paths(&store, &loaded.settings));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Copper")
            .default_width(if options.scripted() { 1440 } else { 1280 })
            .default_height(if options.scripted() { 900 } else { 820 })
            .build();
        window.add_css_class("copper");

        let shell = Shell(Rc::new(Inner {
            app: app.clone(),
            window,
            engine,
            store,
            saver,
            options,
            browser: RefCell::new(loaded.browser),
            history: RefCell::new(loaded.history),
            bookmarks: RefCell::new(loaded.bookmarks),
            settings: RefCell::new(loaded.settings),
            pages: RefCell::default(),
            favicons: RefCell::default(),
            host_icons: RefCell::default(),
            ui: OnceCell::new(),
            registry: RefCell::new(Registry::default()),
            hub: Hub::default(),
            downloads: Downloads::default(),
            cache_dir,
            restored: Cell::new(false),
            saved_generation: Cell::new(0),
            flushing: Cell::new(false),
            again: Cell::new(false),
            history_dirty: Cell::new(false),
            settings_dirty: Cell::new(false),
            timers: RefCell::default(),
            shut: Cell::new(false),
        }));

        let ui = Ui::build(&shell);
        let _ = shell.0.ui.set(ui);
        crate::actions::register_core(&shell);
        crate::actions::register_lanes(&shell);
        shell.apply_look();
        shell.ui().set_layout(&shell, shell.tabs_position());

        let weak = shell.downgrade();
        shell.0.window.connect_close_request(move |_| {
            if let Some(s) = weak.upgrade() {
                s.shutdown();
            }
            glib::Propagation::Proceed
        });
        let weak = shell.downgrade();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(s) = weak.upgrade() {
                s.ui().refresh(&s, &Refresh::spaces());
            }
        });

        shell.0.restored.set(true);
        shell.0.saved_generation.set(shell.browser().generation());
        shell.start_timers();
        shell.ui().refresh(&shell, &Refresh::all());
        shell.show_active();
        if let Some(note) = loaded.notice {
            shell.toast(&note);
        }
        shell.0.window.present();
        shell
    }

    pub fn downgrade(&self) -> WeakShell {
        WeakShell(Rc::downgrade(&self.0))
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.0.window
    }

    pub fn app(&self) -> &adw::Application {
        &self.0.app
    }

    pub fn engine(&self) -> &WebKitEngine {
        &self.0.engine
    }

    pub fn registry(&self) -> &RefCell<Registry> {
        &self.0.registry
    }

    pub fn hub(&self) -> &Hub {
        &self.0.hub
    }

    pub fn downloads(&self) -> &Downloads {
        &self.0.downloads
    }

    pub fn saver(&self) -> &Saver {
        &self.0.saver
    }

    pub fn options(&self) -> &Options {
        &self.0.options
    }

    pub fn store(&self) -> &Store {
        &self.0.store
    }

    pub fn ui(&self) -> &Ui {
        self.0.ui.get().expect("ui is built in Shell::new")
    }

    /// Read the browser state. Never hold the guard across a call back into
    /// the shell.
    pub fn browser(&self) -> std::cell::Ref<'_, Browser> {
        self.0.browser.borrow()
    }

    pub fn history(&self) -> std::cell::Ref<'_, History> {
        self.0.history.borrow()
    }

    pub fn bookmarks(&self) -> std::cell::Ref<'_, Bookmarks> {
        self.0.bookmarks.borrow()
    }

    pub fn settings(&self) -> std::cell::Ref<'_, Settings> {
        self.0.settings.borrow()
    }

    /// Change the browser state, then redraw/save ([`Shell::flush`]).
    pub fn with_browser<R>(&self, f: impl FnOnce(&mut Browser) -> R) -> R {
        let r = f(&mut self.0.browser.borrow_mut());
        self.flush();
        r
    }

    /// Change a setting; written to `settings.json` shortly after.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.0.settings.borrow_mut());
        self.0.settings_dirty.set(true);
        self.save_soon();
    }

    /// Tabs down the left or across the top. Linux defaults to the column
    /// (the Mac's look) when neither `sidebar` nor `manner` was ever set.
    pub fn tabs_position(&self) -> TabsPosition {
        let s = self.settings();
        if s.get(keys::SIDEBAR).is_none() && s.get(keys::MANNER).is_none() {
            TabsPosition::Left
        } else {
            s.tabs_position()
        }
    }

    pub fn set_tabs_position(&self, p: TabsPosition) {
        self.update_settings(|s| s.set_tabs_position(p));
        self.ui().set_layout(self, p);
        self.ui().refresh(self, &Refresh::all());
        self.show_active();
        self.0.hub.send(ShellEvent::LayoutChanged);
    }

    pub fn look(&self) -> Look {
        self.settings().look()
    }

    pub fn set_look(&self, look: Look) {
        self.update_settings(|s| s.set_look(look));
        self.apply_look();
    }

    fn apply_look(&self) {
        let scheme = match self.look() {
            Look::Light => adw::ColorScheme::ForceLight,
            Look::Dark => adw::ColorScheme::ForceDark,
            Look::System => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
        if let Some(ui) = self.0.ui.get() {
            ui.refresh(self, &Refresh::spaces());
        }
    }

    /// The window is dark right now.
    pub fn is_dark(&self) -> bool {
        adw::StyleManager::default().is_dark()
    }

    // MARK: reading

    /// The selected tab.
    pub fn active_id(&self) -> Option<TabId> {
        self.browser().active_id()
    }

    /// The selected tab is private.
    pub fn active_is_private(&self) -> bool {
        self.browser().active().is_some_and(|t| t.private)
    }

    fn view_of(&self, b: &Browser, id: &TabId, active: Option<&TabId>) -> Option<TabView> {
        let t = b.tab(id)?;
        let slot = self.0.pages.borrow().get(id).cloned();
        let favicon = self
            .0
            .favicons
            .borrow()
            .get(id)
            .cloned()
            .or_else(|| self.cached_icon(&t.url));
        Some(TabView {
            id: t.id.clone(),
            label: if t.title.trim().is_empty() && t.url.is_empty() {
                "New Tab".into()
            } else {
                t.label()
            },
            url: t.url.clone(),
            host: host_of(&t.url),
            pin: t.pin.clone(),
            letter: t.pin.clone().unwrap_or_else(|| t.monogram()),
            sleeping: t.sleeping,
            private: t.private,
            saved: t.saved,
            live: active == Some(id),
            loading: slot.as_ref().is_some_and(|s| s.loading.get()),
            audio: slot.as_ref().is_some_and(|s| s.audio.get()),
            crashed: slot.as_ref().is_some_and(|s| s.crashed.get()),
            favicon,
        })
    }

    /// One tab, as drawn.
    pub fn tab_view(&self, id: &TabId) -> Option<TabView> {
        let b = self.browser();
        let active = b.active_id();
        self.view_of(&b, id, active.as_ref())
    }

    /// The current space's tabs, as drawn.
    pub fn visible(&self) -> Visible {
        let b = self.browser();
        let active = b.active_id();
        let mut v = Visible::default();
        for t in b.pins() {
            if let Some(view) = self.view_of(&b, &t.id, active.as_ref()) {
                v.pins.push(view);
            }
        }
        for t in b.row(b.current_space_id()) {
            if let Some(view) = self.view_of(&b, &t.id, active.as_ref()) {
                if view.saved {
                    v.saved.push(view);
                } else {
                    v.today.push(view);
                }
            }
        }
        v
    }

    /// Tab ids in the order ⌘1–9 and ⌃Tab walk.
    pub fn visible_ids(&self) -> Vec<TabId> {
        self.visible().all().map(|t| t.id.clone()).collect()
    }

    /// The page behind a tab, if it has a live one.
    pub fn page(&self, id: &TabId) -> Option<WebKitPage> {
        self.0.pages.borrow().get(id).map(|s| s.page.clone())
    }

    pub(crate) fn slot(&self, id: &TabId) -> Option<Rc<PageSlot>> {
        self.0.pages.borrow().get(id).cloned()
    }

    /// The selected tab's page.
    pub fn active_page(&self) -> Option<WebKitPage> {
        self.active_id().and_then(|id| self.page(&id))
    }

    /// History + open tabs + bookmarks rows for what was typed.
    pub fn suggestions(&self, typed: &str, limit: usize) -> (Vec<Suggestion>, Vec<Suggestion>) {
        let tabs = self.browser().open_tab_suggestions(typed, limit);
        let visited = self.history().suggestions(typed, limit);
        (tabs, visited)
    }

    // MARK: tabs

    /// Open `url` in a new tab. Returns its id.
    pub fn open(&self, url: &str, how: OpenTab) -> TabId {
        let id = {
            let mut b = self.0.browser.borrow_mut();
            let index = how.after.as_ref().and_then(|after| {
                let t = b.tab(after)?;
                if t.is_pinned() {
                    return Some(0);
                }
                let space = how.space.clone().unwrap_or_else(|| t.space.clone());
                b.row(&space).iter().position(|r| r.id == *after).map(|p| p + 1)
            });
            b.open_tab(
                url,
                OpenOptions {
                    title: how.title.clone(),
                    space: how.space.clone(),
                    foreground: how.foreground,
                    at_end: how.at_end,
                    private: how.private,
                    index,
                },
            )
        };
        self.flush();
        id
    }

    /// What the field turns typed text into, with the configured engine.
    pub fn destination(&self, typed: &str) -> Option<String> {
        let engine = self.settings().search_engine();
        copper_core::address::destination(typed, engine).map(|u| u.to_string())
    }

    /// Go somewhere from the field: in the selected tab (`new_tab == false`
    /// and there is one), else in a new tab in front.
    pub fn go(&self, typed: &str, new_tab: bool, private: bool) -> bool {
        let Some(url) = self.destination(typed) else {
            return false;
        };
        match self.active_id().filter(|_| !new_tab) {
            Some(id) => self.navigate(&id, &url),
            None => {
                self.open(
                    &url,
                    OpenTab {
                        foreground: true,
                        private,
                        ..Default::default()
                    },
                );
            }
        }
        true
    }

    /// Load `url` in a tab (waking it).
    pub fn navigate(&self, id: &TabId, url: &str) {
        let had_page = self.page(id);
        self.0.browser.borrow_mut().update_tab(id, |t| {
            t.url = url.to_string();
            t.sleeping = false;
        });
        match had_page {
            Some(page) => {
                if let Some(slot) = self.slot(id) {
                    slot.clear_status();
                }
                page.load(url);
            }
            None => {
                self.ensure_page(id);
            }
        }
        self.flush();
    }

    pub fn select(&self, id: &TabId) -> bool {
        self.with_browser(|b| b.select(id))
    }

    /// Select the `n`th visible tab (0-based); `None` selects the last.
    pub fn select_nth(&self, n: Option<usize>) {
        let ids = self.visible_ids();
        let pick = match n {
            Some(i) => ids.get(i).cloned(),
            None => ids.last().cloned(),
        };
        if let Some(id) = pick {
            self.select(&id);
        }
    }

    /// ⌃Tab / ⌃⇧Tab: the next or previous visible tab, wrapping.
    pub fn cycle_tab(&self, forward: bool) {
        let ids = self.visible_ids();
        if ids.is_empty() {
            return;
        }
        let here = self
            .active_id()
            .and_then(|a| ids.iter().position(|i| *i == a));
        let n = ids.len();
        let next = match here {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        let id = ids[next].clone();
        self.select(&id);
    }

    /// Close a tab (a pin is put down instead).
    pub fn close(&self, id: &TabId) -> CloseOutcome {
        let out = self.with_browser(|b| b.close_tab(id, false));
        if let CloseOutcome::PutDown { .. } = out {
            self.drop_page(id);
        }
        out
    }

    /// Close a tab for good, pin or not.
    pub fn close_force(&self, id: &TabId) -> CloseOutcome {
        self.with_browser(|b| b.close_tab(id, true))
    }

    pub fn close_active(&self) {
        if let Some(id) = self.active_id() {
            self.close(&id);
        }
    }

    /// ⌘⇧T.
    pub fn reopen(&self) -> Option<TabId> {
        let id = self.with_browser(|b| b.reopen_closed());
        if id.is_none() {
            self.toast("No closed tabs to bring back");
        }
        id
    }

    pub fn pin(&self, id: &TabId) -> bool {
        self.with_browser(|b| b.pin(id, None))
    }

    pub fn unpin(&self, id: &TabId) -> bool {
        self.with_browser(|b| b.unpin(id))
    }

    pub fn toggle_pin(&self, id: &TabId) {
        let pinned = self.browser().tab(id).is_some_and(|t| t.is_pinned());
        if pinned {
            self.unpin(id);
        } else {
            self.pin(id);
        }
    }

    /// Keep a tab (Saved) or let it be today's.
    pub fn set_saved(&self, id: &TabId, saved: bool) {
        self.with_browser(|b| b.update_tab(id, |t| t.saved = saved));
    }

    pub fn move_tab(&self, id: &TabId, index: usize) -> bool {
        self.with_browser(|b| b.move_tab(id, index))
    }

    pub fn move_to_space(&self, id: &TabId, space: &str) -> bool {
        let moved = self.with_browser(|b| b.move_to_space(id, space));
        if moved {
            // Another space may mean another cookie jar.
            self.recycle_pages(|b, t| t == id && b.tab(t).is_some());
        }
        moved
    }

    /// A drag let go over another tab: land before (or after) it, in its
    /// section, pinning or unpinning on the way.
    pub fn drop_tab(&self, dragged: &TabId, target: &TabId, after: bool) {
        if dragged == target {
            return;
        }
        let (d_pinned, t_pinned, t_saved) = {
            let b = self.browser();
            let (Some(d), Some(t)) = (b.tab(dragged), b.tab(target)) else {
                return;
            };
            (d.is_pinned(), t.is_pinned(), t.saved)
        };
        self.with_browser(|b| {
            if t_pinned {
                if !d_pinned && !b.pin(dragged, None) {
                    return;
                }
                let pins: Vec<TabId> = b
                    .pins()
                    .iter()
                    .map(|p| p.id.clone())
                    .filter(|p| p != dragged)
                    .collect();
                let at = pins.iter().position(|p| p == target).unwrap_or(pins.len());
                b.move_tab(dragged, at + usize::from(after));
            } else {
                if d_pinned {
                    b.unpin(dragged);
                }
                let space = b.current_space_id().to_string();
                b.update_tab(dragged, |t| t.saved = t_saved);
                let row: Vec<TabId> = b
                    .row(&space)
                    .iter()
                    .map(|t| t.id.clone())
                    .filter(|t| t != dragged)
                    .collect();
                if let Some(at) = row.iter().position(|t| t == target) {
                    b.move_tab(dragged, at + usize::from(after));
                }
            }
        });
    }

    /// A drag let go over the favourites: pin it at the end.
    pub fn drop_on_pins(&self, dragged: &TabId) {
        let pinned = self.browser().tab(dragged).is_some_and(|t| t.is_pinned());
        if !pinned {
            self.pin(dragged);
        }
    }

    /// A drag let go on a section's edge (the space header: keep it, at the
    /// top; New Tab: today's, at the top of today).
    pub fn drop_in_section(&self, dragged: &TabId, saved: bool) {
        self.with_browser(|b| {
            if b.tab(dragged).is_some_and(|t| t.is_pinned()) {
                b.unpin(dragged);
            }
            b.update_tab(dragged, |t| t.saved = saved);
            let space = b.current_space_id().to_string();
            let row: Vec<(TabId, bool)> = b
                .row(&space)
                .iter()
                .filter(|t| t.id != *dragged)
                .map(|t| (t.id.clone(), t.saved))
                .collect();
            let at = if saved {
                0
            } else {
                row.iter().position(|(_, s)| !*s).unwrap_or(row.len())
            };
            b.move_tab(dragged, at);
        });
    }

    /// A new private tab, in front, with the card asking where to.
    pub fn new_private_tab(&self) {
        self.ui().card.open(self, crate::palette::Mode::LaunchPrivate);
    }

    // MARK: the selected page

    pub fn back(&self) {
        if let Some(p) = self.active_page() {
            p.back();
        }
    }

    pub fn forward(&self) {
        if let Some(p) = self.active_page() {
            p.forward();
        }
    }

    /// Reload (a crashed or failed page included).
    pub fn reload(&self, id: &TabId) {
        match self.slot(id) {
            Some(slot) => {
                let failed = slot.failed_url.borrow().clone();
                slot.clear_status();
                match failed {
                    Some(url) => slot.page.load(&url),
                    None => slot.page.reload(false),
                }
            }
            None => {
                self.ensure_page(id);
            }
        }
    }

    pub fn reload_active(&self) {
        if let Some(id) = self.active_id() {
            self.reload(&id);
        }
    }

    /// ⌘⇧R: clear the site's cache, reload from the origin.
    pub fn hard_reload_active(&self) {
        let Some(id) = self.active_id() else { return };
        let Some(slot) = self.slot(&id) else { return };
        slot.clear_status();
        let fut = slot.page.hard_reload();
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            match fut.await {
                Ok(sites) => tracing::info!(?sites, "hard reload"),
                Err(e) => {
                    tracing::warn!(error = %e, "hard reload failed");
                    if let Some(s) = weak.upgrade() {
                        s.toast("Couldn't clear this site's cache");
                    }
                }
            }
        });
    }

    pub fn stop_active(&self) {
        if let Some(p) = self.active_page() {
            p.stop();
        }
    }

    /// Zoom the selected page one step (`Some(true)` in, `Some(false)`
    /// out, `None` back to 100 %).
    pub fn zoom(&self, step: Option<bool>) {
        const STEPS: [f64; 13] = [
            0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
        ];
        let Some(p) = self.active_page() else { return };
        let now = p.zoom();
        let next = match step {
            None => 1.0,
            Some(true) => STEPS.iter().copied().find(|z| *z > now + 0.001).unwrap_or(3.0),
            Some(false) => STEPS
                .iter()
                .rev()
                .copied()
                .find(|z| *z < now - 0.001)
                .unwrap_or(0.5),
        };
        p.set_zoom(next);
        if (next - 1.0).abs() > 0.001 {
            self.toast(&format!("{}%", (next * 100.0).round()));
        }
    }

    pub fn show_inspector(&self) {
        if let Some(p) = self.active_page() {
            p.show_inspector();
        }
    }

    // MARK: spaces

    /// A new space in a colour no other space wears; made current.
    pub fn add_space(&self, name: &str) -> String {
        let id = self.with_browser(|b| {
            let colour = SpaceColour::unused(b.spaces());
            let id = b.add_space(name);
            b.update_space(&id, |s| colour.apply(s));
            b.switch_space(&id);
            id
        });
        self.show_active();
        id
    }

    pub fn switch_space(&self, id: &str) -> bool {
        let ok = self.with_browser(|b| b.switch_space(id));
        if ok {
            self.show_active();
        }
        ok
    }

    /// The `n`th space (0-based).
    pub fn switch_space_nth(&self, n: usize) {
        let id = self.browser().spaces().nth(n).map(|s| s.id.clone());
        if let Some(id) = id {
            self.switch_space(&id);
        }
    }

    pub fn cycle_space(&self, forward: bool) {
        self.with_browser(|b| b.cycle_space(forward));
        self.show_active();
    }

    pub fn rename_space(&self, id: &str, name: &str) {
        if name.trim().is_empty() {
            return;
        }
        self.with_browser(|b| b.update_space(id, |s| s.name = name.trim().to_string()));
    }

    pub fn set_space_colour(&self, id: &str, colour: SpaceColour) {
        self.with_browser(|b| b.update_space(id, |s| colour.apply(s)));
    }

    /// An emoji, `sf:<symbol>`, or `None` for the name's letter.
    pub fn set_space_icon(&self, id: &str, icon: Option<&str>) {
        let icon = icon.map(str::trim).filter(|i| !i.is_empty()).map(str::to_string);
        self.with_browser(|b| b.update_space(id, |s| s.icon = icon));
    }

    /// The space's cookie jar: `None` shares the default one.
    pub fn set_space_profile(&self, id: &str, profile: Option<&str>) {
        let profile = profile
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_string);
        let same = self.browser().space(id).map(|s| s.profile.clone()) == Some(profile.clone());
        if same {
            return;
        }
        self.with_browser(|b| b.update_space(id, |s| s.profile = profile));
        let space = id.to_string();
        // Pages already open keep their old jar until rebuilt.
        self.recycle_pages(move |b, t| b.tab(t).is_some_and(|t| t.space == space && !t.private));
    }

    pub fn move_space(&self, id: &str, index: usize) {
        self.with_browser(|b| b.move_space(id, index));
    }

    /// Move a space one step left (`-1`) or right (`1`).
    pub fn nudge_space(&self, id: &str, by: isize) {
        let at = self.browser().spaces().position(|s| s.id == id);
        if let Some(at) = at {
            let to = (at as isize + by).max(0) as usize;
            self.move_space(id, to);
        }
    }

    /// The space a deleted one's tabs would go to: the neighbour to the
    /// left, or to the right for the first.
    pub fn neighbour_space(&self, id: &str) -> Option<String> {
        let b = self.browser();
        let all: Vec<&str> = b.spaces().map(|s| s.id.as_str()).collect();
        if all.len() < 2 {
            return None;
        }
        let i = all.iter().position(|s| *s == id)?;
        Some(all[if i == 0 { 1 } else { i - 1 }].to_string())
    }

    /// Delete a space; its tabs go to the neighbour when `keep_tabs`.
    pub fn remove_space(&self, id: &str, keep_tabs: bool) -> bool {
        let dest = keep_tabs.then(|| self.neighbour_space(id)).flatten();
        let ok = self.with_browser(|b| b.remove_space(id, dest.as_deref()));
        if ok {
            self.show_active();
        }
        ok
    }

    /// How many loose tabs a space holds.
    pub fn space_tab_count(&self, id: &str) -> usize {
        self.browser().row(id).len()
    }

    // MARK: flush

    /// Read the browser's changes and bring everything in line: pages,
    /// widgets, the session file, watchers. Re-entrant calls are folded
    /// into the running one.
    pub fn flush(&self) {
        if self.0.flushing.get() {
            self.0.again.set(true);
            return;
        }
        self.0.flushing.set(true);
        loop {
            self.0.again.set(false);
            let events = self.0.browser.borrow_mut().take_events();
            if !events.is_empty() {
                self.apply_events(&events);
            }
            if !self.0.again.get() {
                break;
            }
        }
        self.0.flushing.set(false);
        self.save_soon();
    }

    fn apply_events(&self, events: &[BrowserEvent]) {
        let mut refresh = Refresh::default();
        let mut show = false;
        for e in events {
            match e {
                BrowserEvent::TabOpened(id) => {
                    refresh.tabs = true;
                    let awake = self.browser().tab(id).is_some_and(|t| !t.sleeping);
                    if awake {
                        self.ensure_page(id);
                    }
                    self.0.hub.send(ShellEvent::TabOpened(id.clone()));
                }
                BrowserEvent::TabClosed(id) => {
                    refresh.tabs = true;
                    self.drop_page(id);
                    self.0.favicons.borrow_mut().remove(id);
                    self.0.hub.send(ShellEvent::TabClosed(id.clone()));
                }
                BrowserEvent::TabChanged(id) => {
                    let (exists, sleeping) = {
                        let b = self.browser();
                        let t = b.tab(id);
                        (t.is_some(), t.is_some_and(|t| t.sleeping))
                    };
                    if exists && sleeping && self.slot(id).is_some() {
                        self.drop_page(id);
                    }
                    refresh.changed.push(id.clone());
                    self.0.hub.send(ShellEvent::TabChanged(id.clone()));
                }
                BrowserEvent::RowChanged { .. } => refresh.tabs = true,
                BrowserEvent::Selected(id) => {
                    refresh.selection = true;
                    show = true;
                    self.0.hub.send(ShellEvent::Selected(id.clone()));
                }
                BrowserEvent::SpaceSwitched(id) => {
                    refresh.tabs = true;
                    refresh.spaces = true;
                    refresh.selection = true;
                    show = true;
                    self.0.hub.send(ShellEvent::SpaceSwitched(id.clone()));
                }
                BrowserEvent::SpacesChanged => {
                    refresh.tabs = true;
                    refresh.spaces = true;
                    self.0.hub.send(ShellEvent::SpacesChanged);
                }
            }
        }
        if let Some(ui) = self.0.ui.get() {
            ui.refresh(self, &refresh);
        }
        if show {
            self.show_active();
        }
    }

    /// Put the selected tab's page on the stage (building it if needed),
    /// or the launch card when the space has nothing open.
    pub fn show_active(&self) {
        let Some(ui) = self.0.ui.get() else { return };
        match self.active_id() {
            Some(id) => {
                let woke = self.0.browser.borrow_mut().update_tab(&id, |t| t.sleeping = false);
                let _ = woke;
                self.0.browser.borrow_mut().take_events();
                self.ensure_page(&id);
                ui.stage.show(self, Some(&id));
                if ui.card.is_auto() {
                    ui.card.close(self);
                }
            }
            None => {
                ui.stage.show(self, None);
                if !ui.card.is_open() {
                    ui.card.open_empty(self);
                }
            }
        }
        ui.refresh(self, &Refresh::chrome());
    }

    // MARK: saving

    fn save_soon(&self) {
        if !self.0.restored.get() || self.0.shut.get() {
            return;
        }
        let generation = self.browser().generation();
        if generation != self.0.saved_generation.get() {
            self.0.saved_generation.set(generation);
            let shape = self.browser().to_session();
            self.0.saver.schedule_session(shape, true);
        }
        if self.0.settings_dirty.replace(false) {
            self.0
                .saver
                .schedule(self.settings().values().clone());
        }
    }

    /// History changed; written a few seconds later (the list is sorted on
    /// write, so not on every visit).
    pub(crate) fn history_changed(&self) {
        if self.0.history_dirty.replace(true) {
            return;
        }
        let weak = self.downgrade();
        glib::timeout_add_local_once(Duration::from_secs(3), move || {
            if let Some(s) = weak.upgrade() {
                s.write_history();
            }
        });
    }

    fn write_history(&self) {
        if !self.0.history_dirty.replace(false) {
            return;
        }
        let list = self.history().to_file(copper_core::SwiftDate::now());
        self.0.saver.schedule(list);
    }

    /// Write everything now (quit, tests).
    pub fn save_now(&self) {
        self.save_soon();
        self.write_history();
        self.0.saver.flush();
    }

    /// The window is closing: write everything and stop the timers.
    pub fn shutdown(&self) {
        if self.0.shut.get() {
            return;
        }
        self.save_now();
        self.0.shut.set(true);
        for t in self.0.timers.borrow_mut().drain(..) {
            t.remove();
        }
        if let Some(err) = self.0.saver.last_error() {
            tracing::error!(%err, "saving failed");
        }
    }

    fn start_timers(&self) {
        // Idle tabs go to sleep (Settings › Tabs; Mac `Sleep.swift`).
        let weak = self.downgrade();
        let t = glib::timeout_add_seconds_local(60, move || {
            let Some(s) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            s.sleep_idle_tabs();
            glib::ControlFlow::Continue
        });
        self.0.timers.borrow_mut().push(t);
    }

    fn sleep_idle_tabs(&self) {
        let (on, after) = {
            let s = self.settings();
            (s.sleeps_tabs(), s.sleep_after_secs())
        };
        if !on {
            return;
        }
        let now = UnixTime::now();
        let ids: Vec<TabId> = {
            let b = self.browser();
            let active = b.active_id();
            b.all_tabs()
                .filter(|t| !t.sleeping && !t.is_pinned() && !t.private)
                .filter(|t| Some(&t.id) != active.as_ref())
                .filter(|t| now.0 - t.seen.0 >= after)
                .filter(|t| !self.slot(&t.id).is_some_and(|s| s.audio.get()))
                .map(|t| t.id.clone())
                .collect()
        };
        if ids.is_empty() {
            return;
        }
        self.with_browser(|b| {
            for id in &ids {
                b.set_sleeping(id, true);
            }
        });
    }

    // MARK: messages

    /// A short line at the foot of the window.
    pub fn toast(&self, text: &str) {
        if let Some(ui) = self.0.ui.get() {
            ui.toast(text);
        }
    }

    pub(crate) fn cache_dir(&self) -> &std::path::Path {
        &self.0.cache_dir
    }

    pub(crate) fn favicons(&self) -> &RefCell<HashMap<TabId, gdk::Texture>> {
        &self.0.favicons
    }

    pub(crate) fn pages_map(&self) -> &RefCell<HashMap<TabId, Rc<PageSlot>>> {
        &self.0.pages
    }

    pub(crate) fn history_mut(&self) -> std::cell::RefMut<'_, History> {
        self.0.history.borrow_mut()
    }

    pub(crate) fn browser_mut(&self) -> std::cell::RefMut<'_, Browser> {
        self.0.browser.borrow_mut()
    }
}

/// `https://www.example.com/x` → `example.com`.
pub fn host_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => match u.host_str() {
            Some(h) if !h.is_empty() => h.strip_prefix("www.").unwrap_or(h).to_string(),
            _ => url.to_string(),
        },
        Err(_) => url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::host_of;

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://www.example.com/a?b"), "example.com");
        assert_eq!(host_of("http://127.0.0.1:8080/x"), "127.0.0.1");
        assert_eq!(host_of("about:blank"), "about:blank");
    }
}
