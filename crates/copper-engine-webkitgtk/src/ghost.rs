//! `GhostHost`: a widget that keeps its children *mapped* inside a window the
//! user can see, gives them a full-size allocation, and never draws them
//! (T-002 rung a2, `spikes/backstage/src/ghost.rs`).
//!
//! WebKitGTK decides a page is visible from `gtk_widget_get_mapped()` on the
//! web view plus its toplevel's state (`webkitWebViewBaseUpdateVisibility`).
//! An unmapped view is a hidden page: rAF suspended, no rendering. And WebKit
//! only acknowledges a frame (`AcceleratedBackingStore::frameDone`) when GTK
//! snapshots the view, so a mapped view that is never snapshotted stalls
//! after one frame. The host does both halves: its children are mapped (they
//! live in the user's window) and its `snapshot` snapshots each child into a
//! throwaway `GtkSnapshot`, so nothing reaches the screen.
//!
//! The host is parented straight to a `GtkWindow`, next to the window's own
//! child. A window only allocates its child and titlebar, so the host
//! allocates itself after every layout phase of the window's frame clock
//! (before the paint phase snapshots it). It cannot be targeted by the
//! pointer, cannot take focus (and neither can anything inside it) and is
//! hidden from accessibility.

use std::cell::{Cell, RefCell};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GhostHost {
        /// Size given to each child: the stage the page would have on screen.
        pub stage: Cell<(i32, i32)>,
        /// The frame clock's `layout` handler (connected while realized).
        pub layout: RefCell<Option<(gtk4::gdk::FrameClock, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GhostHost {
        const NAME: &'static str = "CopperGhostHost";
        type Type = super::GhostHost;
        type ParentType = gtk4::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("copper-ghost-host");
            klass.set_accessible_role(gtk4::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for GhostHost {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_can_target(false);
            // can-focus=false keeps focus out of the whole subtree, so Tab
            // never walks into an agent's page and a click cannot grab it.
            obj.set_can_focus(false);
            obj.set_focusable(false);
            obj.set_halign(gtk4::Align::Start);
            obj.set_valign(gtk4::Align::Start);
            obj.update_state(&[gtk4::accessible::State::Hidden(true)]);
        }

        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl GhostHost {
        /// Give every child the stage size. Also called from the nudge: a
        /// child that queued a resize while the window's frame clock is
        /// stalled would otherwise stay "without a current allocation" and
        /// GTK refuses to snapshot it.
        pub fn allocate_children(&self) {
            let (w, h) = self.stage.get();
            let mut child = self.obj().first_child();
            while let Some(c) = child {
                // GTK wants a measure before every allocate.
                let _ = c.measure(gtk4::Orientation::Horizontal, -1);
                let _ = c.measure(gtk4::Orientation::Vertical, w);
                c.size_allocate(&gtk4::Allocation::new(0, 0, w, h), -1);
                child = c.next_sibling();
            }
        }

        /// Allocate the host itself: its window never does.
        pub fn allocate_self(&self) {
            let obj = self.obj();
            if !obj.is_visible() || obj.parent().is_none() {
                return;
            }
            let _ = obj.measure(gtk4::Orientation::Horizontal, -1);
            let _ = obj.measure(gtk4::Orientation::Vertical, -1);
            obj.allocate(1, 1, -1, None);
        }

        /// Snapshot every child into a snapshot that is thrown away: WebKit
        /// gets its frame acknowledged, nothing is drawn.
        pub fn snapshot_children(&self) {
            let obj = self.obj();
            let throwaway = gtk4::Snapshot::new();
            let mut child = obj.first_child();
            while let Some(c) = child {
                obj.snapshot_child(&c, &throwaway);
                child = c.next_sibling();
            }
            drop(throwaway.to_node());
        }
    }

    impl WidgetImpl for GhostHost {
        fn measure(&self, _orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (1, 1, -1, -1)
        }

        fn size_allocate(&self, _width: i32, _height: i32, _baseline: i32) {
            self.allocate_children();
        }

        fn snapshot(&self, _snapshot: &gtk4::Snapshot) {
            // No clip and nothing appended: the children's snapshot vfuncs
            // run (that is what acknowledges WebKit's frame), their render
            // nodes go nowhere.
            self.snapshot_children();
        }

        fn realize(&self) {
            self.parent_realize();
            let obj = self.obj();
            if let Some(clock) = obj.frame_clock() {
                let weak = obj.downgrade();
                // After every other handler: GTK lays the window out in its
                // own `layout` handlers, and the paint phase comes next.
                let id = clock.connect_local("layout", true, move |_| {
                    if let Some(host) = weak.upgrade() {
                        host.imp().allocate_self();
                    }
                    None
                });
                *self.layout.borrow_mut() = Some((clock, id));
            }
        }

        fn unrealize(&self) {
            if let Some((clock, id)) = self.layout.borrow_mut().take() {
                clock.disconnect(id);
            }
            self.parent_unrealize();
        }
    }
}

glib::wrapper! {
    pub struct GhostHost(ObjectSubclass<imp::GhostHost>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl GhostHost {
    pub(crate) fn new(stage: (i32, i32)) -> Self {
        let host: Self = glib::Object::new();
        host.imp().stage.set(stage);
        host
    }

    pub(crate) fn set_stage(&self, stage: (i32, i32)) {
        if self.imp().stage.replace(stage) != stage {
            self.queue_allocate();
        }
    }

    /// Hang a page here. It stays mapped while the host is in a mapped window.
    pub(crate) fn hang(&self, child: &impl IsA<gtk4::Widget>) {
        child.set_parent(self);
        self.queue_allocate();
        self.imp().allocate_self();
    }

    pub(crate) fn has_children(&self) -> bool {
        self.first_child().is_some()
    }

    /// Lay out and snapshot every child now, independent of the window's
    /// frame clock. A window whose compositor withholds frame callbacks (an
    /// occluded window that keeps committing) never runs its paint phase, so
    /// the host's `snapshot` never runs and WebKit never gets frameDone.
    /// Snapshotting a child runs the web view's snapshot vfunc only when
    /// WebKit queued a draw (its render node is cached otherwise), so idle
    /// nudges cost almost nothing. (A `GtkWidgetPaintable` does not work for
    /// this: it only refreshes when the window draws the widget.)
    pub(crate) fn nudge(&self) {
        let imp = self.imp();
        imp.allocate_self();
        imp.allocate_children();
        imp.snapshot_children();
    }
}
