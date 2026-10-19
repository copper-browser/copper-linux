//! Tabs an agent works in while nobody looks at them (Mac:
//! `Fork/MCP/Backstage.swift`, D-006). The policy over
//! `copper_engine_webkitgtk::WebKitBackstage`:
//!
//! * [`Stage::prepare`] (before every call on a tab): wake it, build its
//!   page, and — when it is not the tab on screen — take its view out of its
//!   hidden stack frame and hold it in the window's ghost host, where WebKit
//!   counts it as visible (rAF, layout, snapshots, real input). The user's
//!   tab, focus and pixels do not change.
//! * The user selects a held tab: its frame maps, and the view is handed
//!   back into it before the next paint. They leave it: the frame unmaps,
//!   the view comes out again and the backstage hangs it back.
//! * [`Stage::release`] (five minutes after the agent's last call, or the
//!   tab closed): the view goes back into its frame, a background tab again.
//! * A page the shell drops while held (sleep, close) is let go on the next
//!   tick.

use crate::shell::GtkShell;
use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_engine::{Backstage as _, Engine as _};
use copper_engine_webkitgtk::WebKitPage;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How often held pages are checked against the shell's.
const TICK: Duration = Duration::from_secs(2);
/// How often an agent's tab is stamped as looked at (so the idle sleeper
/// leaves it alone while an agent works in it).
const STAMP_EVERY: Duration = Duration::from_secs(60);

struct Held {
    page: WebKitPage,
    frame: gtk::Overlay,
    map: glib::SignalHandlerId,
    unmap: glib::SignalHandlerId,
}

/// The holds this window's agents have.
pub struct Stage {
    shell: WeakShell,
    held: RefCell<HashMap<TabId, Held>>,
    stamped: RefCell<HashMap<TabId, Instant>>,
    tick: RefCell<Option<glib::SourceId>>,
}

impl Stage {
    pub fn new(shell: &Shell) -> Rc<Stage> {
        Rc::new(Stage {
            shell: shell.downgrade(),
            held: RefCell::default(),
            stamped: RefCell::default(),
            tick: RefCell::default(),
        })
    }

    /// Whether the agents hold this tab right now.
    pub fn holds(&self, id: &TabId) -> bool {
        self.held.borrow().contains_key(id)
    }

    /// The tabs held right now.
    pub fn held(&self) -> Vec<TabId> {
        self.held.borrow().keys().cloned().collect()
    }

    /// Ready to be worked on: awake, built, and hung backstage if nobody is
    /// looking at it. True when the page's visibility just changed (woken,
    /// built, or newly held), so the caller gives WebKit a beat.
    pub fn prepare(self: &Rc<Self>, shell: &Shell, id: &TabId) -> bool {
        let mut changed = false;
        let sleeping = shell.browser().tab(id).map(|t| t.sleeping);
        match sleeping {
            None => return false,
            Some(true) => {
                shell.with_browser(|b| b.set_sleeping(id, false));
                changed = true;
            }
            Some(false) => {}
        }
        if shell.slot(id).is_none() {
            if shell.ensure_page(id).is_none() {
                return changed;
            }
            changed = true;
        }
        self.prune(shell);
        self.stamp(shell, id);
        let Some(slot) = shell.slot(id) else {
            return changed;
        };
        if shell.active_id().as_ref() == Some(id) {
            // On screen: left where it is (a hold, if any, stays).
            return changed;
        }
        self.follow_stage_size(shell);
        let view = slot.page.widget();
        let frame_widget: &gtk::Widget = slot.frame.upcast_ref();
        // A background tab's frame is a hidden stack child: the view has to
        // come out of it before the backstage can keep it visible.
        if view.parent().as_ref() == Some(frame_widget) && !view.is_mapped() {
            slot.frame.set_child(None::<&gtk::Widget>);
        }
        let window: gtk::Window = shell.window().clone().upcast();
        if let Err(e) = shell.engine().backstage().hold(&slot.page, &window) {
            tracing::warn!(error = %e, "could not hold an agent's tab backstage");
            if view.parent().is_none() {
                slot.frame.set_child(Some(&view));
            }
            return changed;
        }
        let fresh = !self.held.borrow().contains_key(id);
        if fresh {
            let map = {
                let weak = shell.downgrade();
                let page = slot.page.clone();
                slot.frame.connect_map(move |frame| {
                    // The user is looking at it now: onto their stage.
                    let Some(s) = weak.upgrade() else { return };
                    let frame = frame.clone();
                    s.engine()
                        .backstage()
                        .hand_off_with(&page, |w| frame.set_child(Some(w)));
                })
            };
            let unmap = {
                let page = slot.page.clone();
                slot.frame.connect_unmap(move |frame| {
                    // They moved on while the hold lasts: back backstage
                    // (the backstage hangs a held view that loses its parent).
                    let frame = frame.clone();
                    let page = page.clone();
                    glib::idle_add_local_once(move || {
                        let view = page.widget();
                        let frame_widget: &gtk::Widget = frame.upcast_ref();
                        if !frame.is_mapped()
                            && frame.root().is_some()
                            && view.parent().as_ref() == Some(frame_widget)
                        {
                            frame.set_child(None::<&gtk::Widget>);
                        }
                    });
                })
            };
            self.held.borrow_mut().insert(
                id.clone(),
                Held {
                    page: slot.page.clone(),
                    frame: slot.frame.clone(),
                    map,
                    unmap,
                },
            );
            self.ensure_tick();
        }
        changed || fresh
    }

    /// Let a tab go: back into its frame, a background tab again.
    pub fn release(&self, shell: &Shell, id: &TabId) {
        let Some(h) = self.held.borrow_mut().remove(id) else {
            return;
        };
        self.stamped.borrow_mut().remove(id);
        let_go(shell, h);
    }

    /// Let every tab go (the server stopped).
    pub fn release_all(&self, shell: &Shell) {
        let all: Vec<Held> = self.held.borrow_mut().drain().map(|(_, h)| h).collect();
        self.stamped.borrow_mut().clear();
        for h in all {
            let_go(shell, h);
        }
    }

    /// Holds whose tab closed, slept or got a new page.
    fn prune(&self, shell: &Shell) {
        let stale: Vec<TabId> = self
            .held
            .borrow()
            .iter()
            .filter(|(id, h)| {
                shell
                    .slot(id)
                    .is_none_or(|s| s.page.widget() != h.page.widget())
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            if let Some(h) = self.held.borrow_mut().remove(&id) {
                h.frame.disconnect(h.map);
                h.frame.disconnect(h.unmap);
                shell.engine().backstage().release(&h.page);
            }
            self.stamped.borrow_mut().remove(&id);
        }
    }

    /// Lay held pages out at the size the user's tab has.
    fn follow_stage_size(&self, shell: &Shell) {
        let shown = shell
            .active_id()
            .and_then(|a| shell.slot(&a))
            .map(|s| (s.frame.width(), s.frame.height()))
            .filter(|(w, h)| *w > 200 && *h > 200);
        let size = shown.or_else(|| {
            let w = &shell.ui().stage.widget;
            Some((w.width(), w.height())).filter(|(w, h)| *w > 200 && *h > 200)
        });
        if let Some((w, h)) = size {
            let backstage = shell.engine().backstage();
            if backstage.stage_size() != (w, h) {
                backstage.set_stage_size(w, h);
            }
        }
    }

    /// Mark the tab as looked at now and then, so the idle sleeper does not
    /// put an agent's tab to sleep under it.
    fn stamp(&self, shell: &Shell, id: &TabId) {
        let due = self
            .stamped
            .borrow()
            .get(id)
            .is_none_or(|at| at.elapsed() >= STAMP_EVERY);
        if due {
            self.stamped.borrow_mut().insert(id.clone(), Instant::now());
            shell
                .browser_mut()
                .update_tab(id, |t| t.seen = copper_core::date::UnixTime::now());
        }
    }

    fn ensure_tick(self: &Rc<Self>) {
        if self.tick.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(TICK, move || {
            let Some(stage) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let Some(shell) = stage.shell.upgrade() else {
                return glib::ControlFlow::Break;
            };
            stage.prune(&shell);
            if stage.held.borrow().is_empty() {
                stage.tick.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
        *self.tick.borrow_mut() = Some(id);
    }
}

fn let_go(shell: &Shell, h: Held) {
    h.frame.disconnect(h.map);
    h.frame.disconnect(h.unmap);
    shell.engine().backstage().release(&h.page);
    let view = h.page.widget();
    if view.parent().is_none() && shell.slot_has_frame(&h.frame) {
        h.frame.set_child(Some(&view));
    }
}

/// Small accessors this lane needs on the shell, kept here.
trait ShellExt {
    fn slot_has_frame(&self, frame: &gtk::Overlay) -> bool;
}

impl ShellExt for Shell {
    fn slot_has_frame(&self, frame: &gtk::Overlay) -> bool {
        self.pages_map()
            .borrow()
            .values()
            .any(|s| s.frame.overlay() == frame)
    }
}
