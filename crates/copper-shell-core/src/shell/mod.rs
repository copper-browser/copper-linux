//! The shell: copper-core's state plus every operation the window, the
//! actions and the [`crate::control`] API perform.
//!
//! One [`Shell`] per process (Copper has one window). It owns the
//! [`Browser`] (spaces, tabs, pins, selection), [`History`], [`Bookmarks`],
//! [`Settings`], the debounced [`Saver`], the engine, the view and one
//! [`pages::PageSlot`] per tab that has a live page. Every mutation goes
//! through a method here and ends in [`Shell::flush`], which reads the
//! browser's change events, redraws what changed (through the
//! [`ShellView`]), wakes or drops pages, writes the session (debounced)
//! and tells [`crate::control`] watchers.
//!
//! Generic over the [`Platform`] (engine, main loop, view); apps name it
//! with an alias (`copper-app`: `type Shell = Shell<Gtk>`). Main thread
//! only (`Rc`); other threads reach it through [`crate::control::Control`].

pub mod languages;
pub mod marks;
pub mod pages;

use crate::control::{Hub, ShellEvent};
use crate::downloads::Downloads;
use crate::main_loop::{MainLoop, Task};
use crate::options::Options;
use crate::session;
use crate::spaces::colour::SpaceColour;
use crate::view::{Refresh, ShellView};
use crate::{PageOf, Platform};
use copper_core::browser::{BrowserEvent, CloseOutcome, Closing};
use copper_core::history::Suggestion;
use copper_core::settings::{Look, TabsPosition, keys};
use copper_core::store::Store;
use copper_core::{Bookmarks, Browser, History, OpenOptions, Saver, Settings, TabId, UnixTime};
use copper_engine::{Engine, EnginePaths, PageView};
use pages::PageSlot;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

thread_local! {
    /// The process's shell, a `WeakShell<P>` ([`Shell::new`]; main thread).
    static CURRENT: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

/// The process's shell (Copper has one window), on the main thread while
/// it lives: for the parts reached without one in hand — calls from other
/// threads posted back ([`crate::cloud::adapter`]), page bundles asking
/// for their state. `None` before [`Shell::new`], after the shell is
/// dropped, or for another platform than the one it was made for.
pub fn current<P: Platform>() -> Option<Shell<P>> {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()?
            .downcast_ref::<WeakShell<P>>()?
            .upgrade()
    })
}

/// The shell (cheap to clone; main thread only).
pub struct Shell<P: Platform>(Rc<Inner<P>>);

impl<P: Platform> Clone for Shell<P> {
    fn clone(&self) -> Self {
        Shell(self.0.clone())
    }
}

/// A weak handle, for closures that widgets and pages keep.
pub struct WeakShell<P: Platform>(Weak<Inner<P>>);

impl<P: Platform> Clone for WeakShell<P> {
    fn clone(&self) -> Self {
        WeakShell(self.0.clone())
    }
}

impl<P: Platform> WeakShell<P> {
    pub fn upgrade(&self) -> Option<Shell<P>> {
        self.0.upgrade().map(Shell)
    }
}

/// Something every new tab page goes through ([`Shell::on_page_created`]).
type PageHook<P> = Rc<dyn Fn(&Shell<P>, &TabId, &PageOf<P>)>;
/// Something that follows the window's light or dark ([`Shell::on_dark_changed`]).
type DarkHook<P> = Rc<dyn Fn(&Shell<P>)>;

struct Inner<P: Platform> {
    view: P::View,
    engine: P::Engine,
    store: Store,
    saver: Saver,
    options: Options,
    browser: RefCell<Browser>,
    history: RefCell<History>,
    bookmarks: RefCell<Bookmarks>,
    settings: RefCell<Settings>,
    pages: RefCell<HashMap<TabId, Rc<PageSlot<P>>>>,
    favicons: RefCell<HashMap<TabId, P::Icon>>,
    /// Kept icons read from disk, by their file under `favicons/`.
    host_icons: RefCell<HashMap<String, Option<P::Icon>>>,
    /// Sites already asked for an icon this launch (`marks`), and those
    /// waiting to be asked.
    marks_asked: RefCell<std::collections::HashSet<String>>,
    marks_waiting: RefCell<Vec<String>>,
    /// Tab → the tab whose page opened it (popups, links to a new window).
    openers: RefCell<HashMap<TabId, TabId>>,
    /// Run for every page a tab gets ([`Shell::on_page_created`]).
    page_hooks: RefCell<Vec<PageHook<P>>>,
    dark_hooks: RefCell<Vec<DarkHook<P>>>,
    hub: Hub,
    downloads: Downloads<P>,
    cache_dir: PathBuf,
    /// What loading the session had to say, shown once the window is up.
    notice: RefCell<Option<String>>,
    restored: Cell<bool>,
    saved_generation: Cell<u64>,
    flushing: Cell<bool>,
    again: Cell<bool>,
    history_dirty: Cell<bool>,
    settings_dirty: Cell<bool>,
    timers: RefCell<Vec<Task>>,
    shut: Cell<bool>,
    /// Sleeping tabs' pictures and the wake covers up now.
    covers: crate::cover::Covers<P>,
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

/// What the tab lists draw for one tab (`I`: the view's decoded icon,
/// [`Platform::Icon`]).
#[derive(Clone, Debug)]
pub struct TabView<I> {
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
    pub favicon: Option<I>,
}

/// The current space's tabs, as the column lays them out.
#[derive(Clone, Debug)]
pub struct Visible<I> {
    pub pins: Vec<TabView<I>>,
    /// Loose tabs you keep (Saved), in row order.
    pub saved: Vec<TabView<I>>,
    /// Loose tabs opened today, in row order.
    pub today: Vec<TabView<I>>,
}

impl<I> Default for Visible<I> {
    fn default() -> Self {
        Visible {
            pins: Vec::new(),
            saved: Vec::new(),
            today: Vec::new(),
        }
    }
}

impl<I> Visible<I> {
    /// Pins, then the row (saved then today) — the order ⌘1–9 counts.
    pub fn all(&self) -> impl Iterator<Item = &TabView<I>> {
        self.pins.iter().chain(&self.saved).chain(&self.today)
    }
}

impl<P: Platform> Shell<P> {
    /// Load the session and build the shell: `engine` gets the on-disk
    /// layout (from the store and settings), then `view` makes the window.
    /// Nothing is drawn and no page made yet: the app finishes its view
    /// (which may need the shell), registers its actions, then calls
    /// [`Shell::start`].
    pub fn new(
        options: Options,
        engine: impl FnOnce(EnginePaths) -> P::Engine,
        view: impl FnOnce(&Options) -> P::View,
    ) -> Shell<P> {
        let store = Store::from_env();
        let saver = Saver::new(store.clone());
        let loaded = session::load(&store);
        let cache_dir = session::cache_dir(&store);
        let engine = engine(session::engine_paths(&store, &loaded.settings));
        // Before the first page: every web process takes the list it starts with.
        let languages = P::preferred_languages();
        tracing::debug!(?languages, "page languages");
        engine.set_preferred_languages(&languages);
        let view = view(&options);

        let shell = Shell(Rc::new(Inner {
            view,
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
            marks_asked: RefCell::default(),
            marks_waiting: RefCell::default(),
            openers: RefCell::default(),
            page_hooks: RefCell::default(),
            dark_hooks: RefCell::default(),
            hub: Hub::default(),
            downloads: Downloads::default(),
            cache_dir,
            notice: RefCell::new(loaded.notice),
            restored: Cell::new(false),
            saved_generation: Cell::new(0),
            flushing: Cell::new(false),
            again: Cell::new(false),
            history_dirty: Cell::new(false),
            settings_dirty: Cell::new(false),
            timers: RefCell::default(),
            shut: Cell::new(false),
            covers: Default::default(),
        }));
        CURRENT.with(|c| *c.borrow_mut() = Some(Box::new(shell.downgrade())));
        shell
    }

    /// The view is complete: apply the look and layout, start saving and
    /// the idle-tab timer, draw everything, show the selected tab, say what
    /// loading the session had to say, and present the window.
    pub fn start(&self) {
        self.apply_look();
        self.view().set_layout(self, self.tabs_position());
        self.0.restored.set(true);
        self.0.saved_generation.set(self.browser().generation());
        self.start_timers();
        self.view().refresh(self, &Refresh::all());
        self.show_active();
        let notice = self.0.notice.borrow_mut().take();
        if let Some(note) = notice {
            self.toast(&note);
        }
        self.view().present(self);
    }

    pub fn downgrade(&self) -> WeakShell<P> {
        WeakShell(Rc::downgrade(&self.0))
    }

    /// The window the shell is drawn in.
    pub fn view(&self) -> &P::View {
        &self.0.view
    }

    pub fn engine(&self) -> &P::Engine {
        &self.0.engine
    }

    pub fn hub(&self) -> &Hub {
        &self.0.hub
    }

    pub fn downloads(&self) -> &Downloads<P> {
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

    /// Run `hook` for every page a tab gets from now on (woken, rebuilt, a
    /// popup), right after it is framed and listened to and before it
    /// loads anything. Features that dress pages (style sheets, scripts,
    /// message handlers) register here once, at startup, and go over the
    /// pages already open themselves.
    pub fn on_page_created(&self, hook: impl Fn(&Shell<P>, &TabId, &PageOf<P>) + 'static) {
        self.0.page_hooks.borrow_mut().push(Rc::new(hook));
    }

    pub(crate) fn covers(&self) -> &crate::cover::Covers<P> {
        &self.0.covers
    }

    /// Run `hook` whenever the window goes light or dark
    /// ([`Shell::dark_changed`]): pages that draw their own theme (Canvas)
    /// follow it.
    pub fn on_dark_changed(&self, hook: impl Fn(&Shell<P>) + 'static) {
        self.0.dark_hooks.borrow_mut().push(Rc::new(hook));
    }

    /// The view saw the window go light or dark (the system switched): the
    /// Spaces tints are redrawn, then every [`Shell::on_dark_changed`] hook
    /// runs.
    pub fn dark_changed(&self) {
        self.view().refresh(self, &Refresh::spaces());
        let hooks = self.0.dark_hooks.borrow().clone();
        for hook in hooks {
            hook(self);
        }
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
        self.view().set_layout(self, p);
        self.view().refresh(self, &Refresh::all());
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
        self.view().apply_look(self.look());
        self.view().refresh(self, &Refresh::spaces());
    }

    /// The window is dark right now.
    pub fn is_dark(&self) -> bool {
        self.view().is_dark()
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

    fn view_of(&self, b: &Browser, id: &TabId, active: Option<&TabId>) -> Option<TabView<P::Icon>> {
        let t = b.tab(id)?;
        let slot = self.0.pages.borrow().get(id).cloned();
        let favicon = self
            .0
            .favicons
            .borrow()
            .get(id)
            .cloned()
            .or_else(|| self.cached_icon(&t.url));
        if favicon.is_none() && !t.private {
            self.want_mark(&t.url);
        }
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
    pub fn tab_view(&self, id: &TabId) -> Option<TabView<P::Icon>> {
        let b = self.browser();
        let active = b.active_id();
        self.view_of(&b, id, active.as_ref())
    }

    /// The current space's tabs, as drawn.
    pub fn visible(&self) -> Visible<P::Icon> {
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
    pub fn page(&self, id: &TabId) -> Option<PageOf<P>> {
        self.0.pages.borrow().get(id).map(|s| s.page.clone())
    }

    /// The tab's live page with its frame and state.
    pub fn slot(&self, id: &TabId) -> Option<Rc<PageSlot<P>>> {
        self.0.pages.borrow().get(id).cloned()
    }

    /// The selected tab's page.
    pub fn active_page(&self) -> Option<PageOf<P>> {
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
                b.row(&space)
                    .iter()
                    .position(|r| r.id == *after)
                    .map(|p| p + 1)
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
        // Sent somewhere new, a sleeping tab's picture means nothing.
        self.forget_cover(id);
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

    /// Close a tab the everyday way (⌘W, middle-click, an agent): a pin is
    /// put down and a Saved row unloaded — it stays saved, in its place,
    /// dimmed — instead (Arc; Mac `docs/sections.md`).
    pub fn close(&self, id: &TabId) -> CloseOutcome {
        self.close_with(id, Closing::CLOSE)
    }

    /// Close a tab for good, pin or Saved row or not (the menu's Close Tab,
    /// the cloud removing a kept tab). A Saved row comes back saved with
    /// Reopen Closed Tab.
    pub fn close_force(&self, id: &TabId) -> CloseOutcome {
        self.close_with(id, Closing::REMOVE)
    }

    /// The command line's close and a page's `window.close()`: a pin closes
    /// for good, a Saved row is only unloaded.
    pub fn close_keeping_saved(&self, id: &TabId) -> CloseOutcome {
        self.close_with(id, Closing::KEEP_SAVED)
    }

    /// The cross on a row: on a Saved row it is the way to take the tab out
    /// of Saved (and close it); anywhere else it closes.
    pub fn cross(&self, id: &TabId) -> CloseOutcome {
        let saved = self
            .browser()
            .tab(id)
            .is_some_and(|t| t.saved && !t.is_pinned());
        if saved {
            self.close_force(id)
        } else {
            self.close(id)
        }
    }

    fn close_with(&self, id: &TabId, how: Closing) -> CloseOutcome {
        let out = self.close_tab(id, how);
        if let CloseOutcome::PutDown { .. } | CloseOutcome::Unloaded { .. } = out {
            self.drop_page(id);
        }
        out
    }

    /// Remember that `opener`'s page opened tab `id`.
    pub fn set_opener(&self, id: &TabId, opener: &TabId) {
        if id != opener {
            self.0
                .openers
                .borrow_mut()
                .insert(id.clone(), opener.clone());
        }
    }

    /// The still-open tab whose page opened `id`.
    pub fn opener_of(&self, id: &TabId) -> Option<TabId> {
        let opener = self.0.openers.borrow().get(id).cloned()?;
        self.browser().tab(&opener).is_some().then_some(opener)
    }

    /// Close `id`. When it is the tab on screen and another tab opened it
    /// (a sign-in popup closing itself once you are in, a link that opened
    /// a new window), you land back on that tab rather than on whichever
    /// neighbour the row would pick (Mac `webViewDidClose`).
    fn close_tab(&self, id: &TabId, how: Closing) -> CloseOutcome {
        let back = self
            .opener_of(id)
            .filter(|_| self.active_id().as_ref() == Some(id));
        self.with_browser(|b| {
            let out = b.close_with(id, how);
            match (out, back) {
                (CloseOutcome::Closed { .. }, Some(opener)) if b.select(&opener) => {
                    CloseOutcome::Closed {
                        selected: Some(opener),
                    }
                }
                (CloseOutcome::Unloaded { .. }, Some(opener)) if b.select(&opener) => {
                    CloseOutcome::Unloaded {
                        selected: Some(opener),
                    }
                }
                (out, _) => out,
            }
        })
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

    /// Save a tab or let it go (the menu's Save / Unsave; Mac
    /// `Sections.set`): saving parks the row at the end of Saved, unsaving
    /// puts it at the top of Today. A pin is left as it is.
    pub fn set_saved(&self, id: &TabId, saved: bool) {
        if self.browser().tab(id).is_none_or(|t| t.is_pinned()) {
            return;
        }
        if saved {
            self.drop_at(id, true, None);
        } else {
            self.drop_in_section(id, false);
        }
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
                b.update_tab(dragged, |t| t.saved = t_saved && !t.private);
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

    /// A drag let go in the column's Saved (`saved`) or Today block, before
    /// `before` (a row of that block) or at the block's end — where the
    /// column's drop marker showed it would land.
    pub fn drop_at(&self, dragged: &TabId, saved: bool, before: Option<&TabId>) {
        self.with_browser(|b| b.place_in_section(dragged, saved, before));
    }

    /// A drag let go on a section's head (Saved: kept, at the top; Today:
    /// today's, at the top).
    pub fn drop_in_section(&self, dragged: &TabId, saved: bool) {
        let first = self
            .browser()
            .row(self.browser().current_space_id())
            .iter()
            .find(|t| t.saved == saved && t.id != *dragged)
            .map(|t| t.id.clone());
        self.drop_at(dragged, saved, first.as_ref());
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
        P::Loop::spawn_local(async move {
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
            Some(true) => STEPS
                .iter()
                .copied()
                .find(|z| *z > now + 0.001)
                .unwrap_or(3.0),
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
        let icon = icon
            .map(str::trim)
            .filter(|i| !i.is_empty())
            .map(str::to_string);
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
                    self.forget_cover(id);
                    self.0.favicons.borrow_mut().remove(id);
                    self.0
                        .openers
                        .borrow_mut()
                        .retain(|tab, opener| tab != id && opener != id);
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
        self.view().refresh(self, &refresh);
        if show {
            self.show_active();
        }
    }

    /// Put the selected tab's page on the stage (building it if needed),
    /// or the launch card when the space has nothing open.
    pub fn show_active(&self) {
        match self.active_id() {
            Some(id) => {
                let woke = self
                    .0
                    .browser
                    .borrow_mut()
                    .update_tab(&id, |t| t.sleeping = false);
                let _ = woke;
                self.0.browser.borrow_mut().take_events();
                self.ensure_page(&id);
                self.view().show_tab(self, Some(&id));
            }
            None => self.view().show_tab(self, None),
        }
        self.view().refresh(self, &Refresh::chrome());
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
            self.0.saver.schedule(self.settings().values().clone());
        }
    }

    /// History changed; written a few seconds later (the list is sorted on
    /// write, so not on every visit).
    pub fn history_changed(&self) {
        if self.0.history_dirty.replace(true) {
            return;
        }
        let weak = self.downgrade();
        P::Loop::spawn_local(async move {
            P::Loop::sleep(Duration::from_secs(3)).await;
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
            t.abort();
        }
        if let Some(err) = self.0.saver.last_error() {
            tracing::error!(%err, "saving failed");
        }
    }

    fn start_timers(&self) {
        // Idle tabs go to sleep (Mac `Sleep.swift`): looked at every quarter
        // of `sleep.after`, 5 s to a minute.
        let weak = self.downgrade();
        let t = crate::main_loop::spawn::<P::Loop>(async move {
            loop {
                let every = {
                    let Some(s) = weak.upgrade() else { break };
                    crate::sleep::every(s.settings().sleep_after_secs())
                };
                P::Loop::sleep(every).await;
                let Some(s) = weak.upgrade() else { break };
                s.sleep_idle_tabs();
            }
        });
        self.0.timers.borrow_mut().push(t);
    }

    /// Why tab `id` has to stay awake now ([`crate::sleep::keep`]).
    pub fn keeps_awake(&self, id: &TabId) -> Option<crate::sleep::Keep> {
        let agents = crate::agents::get::<P>();
        let b = self.browser();
        let t = b.tab(id)?;
        let active = b.active_id();
        let on_screen = active.as_ref() == Some(id)
            || t.split.is_some()
                && active
                    .as_ref()
                    .and_then(|a| b.tab(a))
                    .is_some_and(|a| a.split == t.split);
        let slot = self.slot(id);
        let facts = crate::sleep::Facts {
            on_screen,
            pinned: t.is_pinned(),
            agent: agents.as_ref().is_some_and(|a| a.works_on(id)),
            blank: crate::clear::is_blank(&t.url),
            private: t.private,
            asleep: t.sleeping,
            page: slot.is_some(),
            loading: slot.as_ref().is_some_and(|s| s.loading.get()),
            audio: slot.as_ref().is_some_and(|s| s.audio.get()),
            call: slot.as_ref().is_some_and(|s| s.capture.get()),
            downloading: slot
                .as_ref()
                .is_some_and(|s| !s.downloads.borrow().is_empty()),
            opener: false,
        };
        drop(b);
        let opener = active.is_some_and(|a| self.opener_of(&a).as_ref() == Some(id));
        crate::sleep::keep(&crate::sleep::Facts { opener, ..facts })
    }

    /// Put the tabs the policy picks to sleep ([`crate::sleep::choose`]).
    pub fn sleep_idle_tabs(&self) {
        let rules = {
            let s = self.settings();
            if !s.sleeps_tabs() {
                return;
            }
            let memory = P::memory();
            crate::sleep::Rules {
                after: s.sleep_after_secs(),
                after_set: s.sleep_after_set(),
                budget: crate::sleep::budget(memory.map(|(total, _)| total), s.sleep_keep()),
                pressure: memory
                    .map(|(total, free)| crate::sleep::Pressure::of(total, free))
                    .unwrap_or_default(),
            }
        };
        let now = UnixTime::now();
        let ids: Vec<(TabId, f64, bool)> = {
            let b = self.browser();
            b.all_tabs()
                .map(|t| (t.id.clone(), now.0 - t.seen.0, b.is_remembered(&t.id)))
                .collect()
        };
        let candidates: Vec<crate::sleep::Candidate<TabId>> = ids
            .into_iter()
            .map(|(id, idle, remembered)| crate::sleep::Candidate {
                loaded: self.slot(&id).is_some(),
                keep: self.keeps_awake(&id),
                remembered,
                idle,
                id,
            })
            .collect();
        let chosen = crate::sleep::choose(&candidates, &rules);
        if chosen.is_empty() {
            return;
        }
        tracing::debug!(count = chosen.len(), "tabs going to sleep");
        let weak = self.downgrade();
        P::Loop::spawn_local(async move {
            for id in chosen {
                put_to_sleep(&weak, id).await;
            }
        });
    }

    // MARK: messages

    /// A short line at the foot of the window.
    pub fn toast(&self, text: &str) {
        self.view().toast(text);
    }

    /// Copper's cache folder (favicons, thumbnails).
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.0.cache_dir
    }

    /// Live tabs' favicons, as the view decoded them.
    pub fn favicons(&self) -> &RefCell<HashMap<TabId, P::Icon>> {
        &self.0.favicons
    }

    /// Every live page, by tab. Never hold the borrow across a call back in.
    pub fn pages_map(&self) -> &RefCell<HashMap<TabId, Rc<PageSlot<P>>>> {
        &self.0.pages
    }

    /// Change history directly; call [`Shell::history_changed`] after.
    pub fn history_mut(&self) -> std::cell::RefMut<'_, History> {
        self.0.history.borrow_mut()
    }

    pub fn bookmarks_mut(&self) -> std::cell::RefMut<'_, Bookmarks> {
        self.0.bookmarks.borrow_mut()
    }

    /// Change the browser state without a [`Shell::flush`] (the caller
    /// flushes once after a batch).
    pub fn browser_mut(&self) -> std::cell::RefMut<'_, Browser> {
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

/// Ask tab `id`'s page whether it holds anything typed, then let it go —
/// looking again after each step, since each takes a moment and the tab
/// may have been picked meanwhile (Mac `Browser.sleep`).
pub async fn put_to_sleep<P: Platform>(weak: &WeakShell<P>, id: TabId) {
    let page = {
        let Some(s) = weak.upgrade() else { return };
        if s.keeps_awake(&id).is_some() {
            return;
        }
        let Some(page) = s.page(&id) else { return };
        page
    };
    let typed = crate::forms::watch::call(&page, &copper_pagejs::forms::unsaved())
        .await
        .unwrap_or(false);
    drop(page);
    let Some(s) = weak.upgrade() else { return };
    if typed {
        tracing::debug!(tab = %id.short(), "awake: holding something typed");
        return;
    }
    if let Some(k) = s.keeps_awake(&id) {
        tracing::debug!(tab = %id.short(), "awake: {}", k.reason());
        return;
    }
    // Its picture, for the cover it wakes under (if anyone draws covers).
    let Some(page) = s.page(&id) else { return };
    let drawn = s.draws_covers();
    drop(s);
    let cover = match drawn {
        true => crate::cover::take::<P>(&page).await,
        false => None,
    };
    drop(page);
    let Some(s) = weak.upgrade() else { return };
    if let Some(k) = s.keeps_awake(&id) {
        tracing::debug!(tab = %id.short(), "awake: {}", k.reason());
        return;
    }
    if let Some(cover) = cover {
        s.keep_picture(&id, cover);
    }
    s.with_browser(|b| b.set_sleeping(&id, true));
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
