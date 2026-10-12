//! `WebKitBackstage`: pages an agent works in while nobody is looking at
//! them (D-006). See `copper_engine::Backstage` for the contract and
//! `ghost.rs` for why a mapped, never-drawn host keeps WebKit rendering.
//!
//! Each host window gets one [`GhostHost`], parented straight to the window.
//! Held pages hang in it at stage size. An 8 ms timer drives every host whose
//! window's frame clock has not advanced lately (an idle window, or one whose
//! compositor withholds frame callbacks), so a stalled window never stalls an
//! agent's page. A held page the shell is showing is left where it is; when
//! the shell takes it out of its container again, the backstage hangs it
//! back on the next idle.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use copper_engine::{Backstage, BackstageMode, BackstageStatus, EngineError, Result};
use glib::prelude::*;
use gtk4::prelude::*;
use webkit6::WebView;

use crate::ghost::GhostHost;
use crate::page::{Shared, WebKitPage};

/// Timer period that drives hosts whose window is not painting.
const NUDGE_EVERY: Duration = Duration::from_millis(8);
/// A window whose frame counter has not moved for this long is not painting.
const STALL_AFTER: Duration = Duration::from_millis(20);
/// The stage size until the shell sets one: a laptop's worth (macOS default).
const DEFAULT_STAGE: (i32, i32) = (1280, 800);

thread_local! {
    static BLOCKED: Cell<u64> = const { Cell::new(0) };
}

/// Count one popup / dialog / prompt swallowed for a held page.
pub(crate) fn note_blocked() {
    BLOCKED.with(|b| b.set(b.get() + 1));
}

/// Keeps held pages mapped (so WebKit renders them at full rate) inside the
/// shell's windows without drawing them. Cheap to clone (shared handle).
#[derive(Clone)]
pub struct WebKitBackstage {
    inner: Rc<Inner>,
}

struct Inner {
    stage: Cell<(i32, i32)>,
    page_focus: Cell<bool>,
    hosts: RefCell<Vec<Host>>,
    held: RefCell<Vec<Held>>,
    drive: RefCell<Option<glib::SourceId>>,
    nudges: Cell<u64>,
}

/// The ghost host in one window.
struct Host {
    window: glib::WeakRef<gtk4::Window>,
    ghost: GhostHost,
    /// `destroy` and `unmap` handlers on the window.
    watches: Vec<glib::SignalHandlerId>,
    /// Last frame counter seen, and when it last moved.
    frame: Cell<(i64, Instant)>,
}

struct Held {
    key: usize,
    view: glib::WeakRef<WebView>,
    shared: Rc<Shared>,
    /// The window it should hang in.
    window: glib::WeakRef<gtk4::Window>,
    parent_watch: glib::SignalHandlerId,
    /// Page focus was turned on for it (`set_page_focus`).
    focused: Cell<bool>,
}

fn key(view: &WebView) -> usize {
    view.as_ptr() as usize
}

impl Default for WebKitBackstage {
    fn default() -> Self {
        Self::new()
    }
}

impl WebKitBackstage {
    /// A backstage with no hosts yet. `WebKitEngine::backstage` is the usual
    /// one; this is for tools that drive pages without an engine handle.
    pub fn new() -> Self {
        WebKitBackstage {
            inner: Rc::new(Inner {
                stage: Cell::new(DEFAULT_STAGE),
                page_focus: Cell::new(false),
                hosts: RefCell::default(),
                held: RefCell::default(),
                drive: RefCell::default(),
                nudges: Cell::new(0),
            }),
        }
    }

    /// Lay hosted pages out at `width` x `height` (the size the shell's tab
    /// area has; macOS `Backstage.stageSize`). Default 1280 x 800.
    pub fn set_stage_size(&self, width: i32, height: i32) {
        let stage = (width.max(1), height.max(1));
        self.inner.stage.set(stage);
        for h in self.inner.hosts.borrow().iter() {
            h.ghost.set_stage(stage);
        }
    }

    pub fn stage_size(&self) -> (i32, i32) {
        self.inner.stage.get()
    }

    /// Also give hosted pages page focus (`document.hasFocus()`, caret,
    /// focus events) without GTK focus, like macOS's backstage window that
    /// "says it is key". Off by default: WebKit also tells its input-method
    /// context the view got focus, which on an IBus/Fcitx desktop might pull
    /// the IM's focus off the user's field (T-002 risk R5).
    pub fn set_page_focus(&self, on: bool) {
        if self.inner.page_focus.replace(on) == on {
            return;
        }
        let held: Vec<(WebView, Rc<Shared>)> = self
            .inner
            .held
            .borrow()
            .iter()
            .filter_map(|h| Some((h.view.upgrade()?, h.shared.clone())))
            .collect();
        for (view, shared) in held {
            self.inner.focus(&view, &shared, on && hung(&view));
        }
    }

    /// [`hand_off`](Backstage::hand_off) and put the page's widget where
    /// the shell shows it, in one step: `place` gets the widget, free of any
    /// parent, and must parent it (`stack.add_named(w, …)`, `bin.set_child(…)`).
    /// Works for held and unheld pages alike. `false` (and `place` is not
    /// called) when the widget already sits in a shell container.
    pub fn hand_off_with(&self, page: &WebKitPage, place: impl FnOnce(&gtk4::Widget)) -> bool {
        self.hand_off(page);
        let widget = page.widget();
        if widget.parent().is_some() {
            return false;
        }
        place(&widget);
        true
    }

    /// Whether `window` can host pages at full rate right now: mapped, and
    /// neither minimized nor suspended (GNOME suspends windows on other
    /// workspaces; WebKit then hides every page in them, T-002 risk R1).
    pub fn can_host(window: &gtk4::Window) -> bool {
        window_ok(window)
    }
}

fn window_ok(window: &gtk4::Window) -> bool {
    if !window.is_mapped() {
        return false;
    }
    let bad = gdk4::ToplevelState::MINIMIZED | gdk4::ToplevelState::SUSPENDED;
    window
        .surface()
        .and_downcast::<gdk4::Toplevel>()
        .is_none_or(|t| !t.state().intersects(bad))
}

/// Hanging in a ghost host right now.
fn hung(view: &WebView) -> bool {
    view.parent().is_some_and(|p| p.is::<GhostHost>())
}

impl Inner {
    /// The host in `window`, made on first use.
    fn host(self: &Rc<Self>, window: &gtk4::Window) -> GhostHost {
        if let Some(h) = self
            .hosts
            .borrow()
            .iter()
            .find(|h| h.window.upgrade().as_ref() == Some(window))
        {
            return h.ghost.clone();
        }
        let ghost = GhostHost::new(self.stage.get());
        ghost.set_parent(window);
        let rehome_later = {
            let weak = Rc::downgrade(self);
            move || {
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.rehome();
                    }
                });
            }
        };
        let g = ghost.downgrade();
        let later = rehome_later.clone();
        // Before the window finalizes: take the host out of it (GTK only
        // unparents the children it knows about) and re-home its pages.
        let destroy = window.connect_destroy(move |_| {
            if let Some(g) = g.upgrade() {
                g.unparent();
            }
            later();
        });
        // `gtk_window_destroy` (and hiding) unmaps the window long before it
        // is disposed: its pages would be hidden pages from then on.
        let unmap = window.connect_unmap(move |_| rehome_later());
        self.hosts.borrow_mut().push(Host {
            window: window.downgrade(),
            ghost: ghost.clone(),
            watches: vec![destroy, unmap],
            frame: Cell::new((-1, Instant::now())),
        });
        ghost
    }

    /// Any host in a window that can show pages, for pages whose own
    /// window went away.
    fn any_live_host(&self) -> Option<(gtk4::Window, GhostHost)> {
        let hosts = self.hosts.borrow();
        let live = || {
            hosts
                .iter()
                .filter_map(|h| Some((h.window.upgrade()?, h.ghost.clone())))
                .filter(|(w, g)| g.parent().is_some() && w.is_mapped())
        };
        live().find(|(w, _)| window_ok(w)).or_else(|| live().next())
    }

    /// Drop hosts whose window is gone, and move pages out of windows that
    /// cannot show them (closed, hidden) into one that can.
    fn rehome(self: &Rc<Self>) {
        self.hosts.borrow_mut().retain(|h| {
            let alive = h.window.upgrade().is_some() && h.ghost.parent().is_some();
            if !alive {
                // Its window is gone (the destroy handler took the host out).
                while let Some(c) = h.ghost.first_child() {
                    c.unparent();
                }
            }
            alive
        });
        let keys: Vec<usize> = self.held.borrow().iter().map(|h| h.key).collect();
        for k in keys {
            self.settle(k);
        }
    }

    /// Put held page `k` where it can render: in the host of its window if
    /// that window is mapped, else in any host whose window is. A page the
    /// shell shows is left alone; with no usable window at all, a page with
    /// nowhere to be still goes into its own window's host (degraded).
    fn settle(self: &Rc<Self>, k: usize) {
        let (view, shared, window) = {
            let held = self.held.borrow();
            let Some(h) = held.iter().find(|h| h.key == k) else {
                return;
            };
            let Some(view) = h.view.upgrade() else {
                return;
            };
            (view, h.shared.clone(), h.window.upgrade())
        };
        let current = match view.parent() {
            None => None,
            Some(p) => match p.downcast::<GhostHost>() {
                Ok(g) => Some(g),
                Err(_) => return,
            },
        };
        let usable = |g: &GhostHost| {
            g.root()
                .and_downcast::<gtk4::Window>()
                .is_some_and(|w| w.is_mapped())
        };
        if current.as_ref().is_some_and(usable) {
            return;
        }
        let target = match window {
            Some(w) if w.is_mapped() => Some(self.host(&w)),
            w => match (self.any_live_host(), w) {
                (Some((live, g)), _) => {
                    if let Some(h) = self.held.borrow().iter().find(|h| h.key == k) {
                        h.window.set(Some(&live));
                    }
                    Some(g)
                }
                (None, Some(w)) if current.is_none() => Some(self.host(&w)),
                _ => None,
            },
        };
        let Some(ghost) = target else {
            return;
        };
        if current.as_ref() == Some(&ghost) {
            return;
        }
        if current.is_some() {
            view.unparent();
        }
        ghost.hang(&view);
        self.focus(&view, &shared, self.page_focus.get());
        self.ensure_drive();
    }

    /// Turn page focus on/off for a held page (tracked per page).
    fn focus(&self, view: &WebView, shared: &Shared, on: bool) {
        let held = self.held.borrow();
        let Some(h) = held.iter().find(|h| h.key == key(view)) else {
            return;
        };
        if h.focused.get() != on {
            h.focused.set(on);
            shared.hands.page_focus(on);
        }
    }

    fn ensure_drive(self: &Rc<Self>) {
        if self.drive.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(NUDGE_EVERY, move || match weak.upgrade() {
            Some(inner) => inner.tick(),
            None => glib::ControlFlow::Break,
        });
        *self.drive.borrow_mut() = Some(id);
    }

    /// Drive every host whose window is not painting; stop when no host
    /// holds a page.
    fn tick(&self) -> glib::ControlFlow {
        let now = Instant::now();
        let mut due = Vec::new();
        let mut any = false;
        for h in self.hosts.borrow().iter() {
            if !h.ghost.has_children() {
                continue;
            }
            any = true;
            let counter = h
                .window
                .upgrade()
                .and_then(|w| w.frame_clock())
                .map_or(-1, |c| c.frame_counter());
            let (last, moved) = h.frame.get();
            if counter != last {
                h.frame.set((counter, now));
            } else if now.duration_since(moved) >= STALL_AFTER {
                due.push(h.ghost.clone());
            }
        }
        if !any {
            self.drive.borrow_mut().take();
            return glib::ControlFlow::Break;
        }
        for ghost in due {
            ghost.nudge();
            self.nudges.set(self.nudges.get() + 1);
        }
        glib::ControlFlow::Continue
    }

    /// Forget held pages whose view is gone.
    fn prune(&self) {
        let dead: Vec<Held> = {
            let mut held = self.held.borrow_mut();
            let (dead, live): (Vec<Held>, Vec<Held>) =
                held.drain(..).partition(|h| h.view.upgrade().is_none());
            *held = live;
            dead
        };
        for h in dead {
            h.shared.held.set(false);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(id) = self.drive.get_mut().take() {
            id.remove();
        }
        for h in self.held.get_mut().drain(..) {
            h.shared.held.set(false);
            if let Some(v) = h.view.upgrade() {
                v.disconnect(h.parent_watch);
            }
        }
        for h in self.hosts.get_mut().drain(..) {
            if let Some(w) = h.window.upgrade() {
                for id in h.watches {
                    w.disconnect(id);
                }
            }
            if h.ghost.parent().is_some() {
                h.ghost.unparent();
            }
        }
    }
}

impl Backstage for WebKitBackstage {
    type Page = WebKitPage;
    type Host = gtk4::Window;

    fn hold(&self, page: &WebKitPage, window: &gtk4::Window) -> Result<()> {
        let inner = &self.inner;
        inner.prune();
        let view = page.view().clone();
        let k = key(&view);
        match view.parent() {
            None => {}
            Some(p) if p.is::<GhostHost>() => {}
            // Shown by the shell: leave it there.
            Some(_) if view.is_mapped() => {}
            Some(p) => {
                return Err(EngineError::InvalidArgument(format!(
                    "the page sits in a {} that hides it; take it out of that container first",
                    p.type_().name()
                )));
            }
        }
        let known = inner.held.borrow().iter().position(|h| h.key == k);
        if let Some(i) = known {
            inner.held.borrow()[i].window.set(Some(window));
        } else {
            let weak = Rc::downgrade(inner);
            // The shell took the page out of its container (or out of the
            // host after `hand_off`): hang it back unless something else
            // took it by the next idle.
            let parent_watch = view.connect_parent_notify(move |v| {
                if v.parent().is_some() {
                    return;
                }
                let weak = weak.clone();
                let k = key(v);
                glib::idle_add_local_once(move || {
                    if let Some(inner) = weak.upgrade() {
                        inner.settle(k);
                    }
                });
            });
            inner.held.borrow_mut().push(Held {
                key: k,
                view: view.downgrade(),
                shared: page.shared().clone(),
                window: window.downgrade(),
                parent_watch,
                focused: Cell::new(false),
            });
            page.shared().held.set(true);
        }
        // Where it hangs: the host in `window` (moving it there from another
        // host if needed). A page the shell shows stays put.
        match view.parent() {
            None => inner.host(window).hang(&view),
            Some(p) => {
                if let Ok(ghost) = p.downcast::<GhostHost>() {
                    let want = inner.host(window);
                    if ghost != want {
                        view.unparent();
                        want.hang(&view);
                    }
                }
            }
        }
        inner.focus(&view, page.shared(), inner.page_focus.get() && hung(&view));
        inner.ensure_drive();
        Ok(())
    }

    fn release(&self, page: &WebKitPage) {
        let view = page.view();
        let k = key(view);
        let h = {
            let mut held = self.inner.held.borrow_mut();
            match held.iter().position(|h| h.key == k) {
                Some(i) => held.remove(i),
                None => return,
            }
        };
        view.disconnect(h.parent_watch);
        page.shared().held.set(false);
        if h.focused.get() {
            page.shared().hands.page_focus(false);
        }
        if let Some(menu) = page.shared().open_menu() {
            menu.cancel();
        }
        if hung(view) {
            view.unparent();
        }
    }

    fn holds(&self, page: &WebKitPage) -> bool {
        let k = key(page.view());
        self.inner
            .held
            .borrow()
            .iter()
            .any(|h| h.key == k && h.view.upgrade().is_some())
    }

    fn hand_off(&self, page: &WebKitPage) {
        let view = page.view();
        if !hung(view) {
            return;
        }
        if let Some(menu) = page.shared().open_menu() {
            menu.cancel();
        }
        self.inner.focus(view, page.shared(), false);
        view.unparent();
    }

    fn status(&self) -> BackstageStatus {
        let inner = &self.inner;
        inner.prune();
        let (mut hosted, mut shown, mut ok) = (0, 0, true);
        let held = inner.held.borrow();
        for h in held.iter() {
            let Some(view) = h.view.upgrade() else {
                continue;
            };
            if hung(&view) {
                hosted += 1;
                let window = view.root().and_downcast::<gtk4::Window>();
                ok &= view.is_mapped() && window.is_some_and(|w| window_ok(&w));
            } else if view.parent().is_some() && view.is_mapped() {
                shown += 1;
            } else {
                ok = false;
            }
        }
        let mode = if held.is_empty() {
            BackstageMode::Idle
        } else if ok {
            BackstageMode::InWindow
        } else {
            BackstageMode::Degraded
        };
        let (w, h) = inner.stage.get();
        let mut status = BackstageStatus::default();
        status.mode = mode;
        status.held = held.len();
        status.hosted = hosted;
        status.shown = shown;
        status.stage = (w as u32, h as u32);
        status.hosts = inner
            .hosts
            .borrow()
            .iter()
            .filter(|h| h.ghost.has_children())
            .count();
        status.nudges = inner.nudges.get();
        status.popups_blocked = BLOCKED.with(Cell::get);
        status
    }
}

impl std::fmt::Debug for WebKitBackstage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebKitBackstage")
            .field("held", &self.inner.held.borrow().len())
            .field("hosts", &self.inner.hosts.borrow().len())
            .field("stage", &self.inner.stage.get())
            .finish()
    }
}
