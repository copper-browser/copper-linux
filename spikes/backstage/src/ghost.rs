//! `GhostHost`: a widget that keeps its children *mapped* inside a window the
//! user can see, gives them a full-size allocation, and never draws them.
//!
//! Why: WebKitGTK decides a page is visible from `gtk_widget_get_mapped()` on
//! the web view plus its toplevel's state (`webkitWebViewBaseUpdateVisibility`
//! in WebKitWebViewBase.cpp). An unmapped view — a non-visible `GtkStack` page,
//! a window that was never presented — is a hidden page: `document.hidden`,
//! rAF suspended, no rendering. And WebKit only acknowledges a rendered frame
//! (`AcceleratedBackingStore::frameDone`) when GTK snapshots the widget, so a
//! mapped view that is never snapshotted stalls after one frame.
//!
//! So the host does both halves: its children are mapped (they live in the
//! user's window), and its `snapshot` snapshots each child into a throwaway
//! `GtkSnapshot` that is dropped — WebKit swaps its buffer and sends frameDone,
//! nothing reaches the screen. The host itself is 1x1, cannot be targeted by
//! the pointer, cannot take focus (and neither can anything inside it), and is
//! hidden from accessibility.

use std::cell::Cell;
use std::time::Duration;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GhostHost {
        /// Size given to each child: the stage the page would have on screen.
        pub stage: Cell<(i32, i32)>,
        /// How many times a child was snapshotted (for the report).
        pub snapshots: Cell<u64>,
        pub self_driven: Cell<u64>,
        /// How many times GTK allocated the host (a relayout loop would show here).
        pub allocations: Cell<u64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GhostHost {
        const NAME: &'static str = "CopperGhostHost";
        type Type = super::GhostHost;
        type ParentType = gtk4::Widget;
    }

    impl ObjectImpl for GhostHost {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_can_target(false);
            // GTK4: can-focus=false keeps focus out of the whole subtree, so
            // Tab never walks into an agent's page.
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
        /// Give every child the stage size. Also called from the self-drive
        /// tick: a child that queued a resize while the window's frame clock
        /// is stalled would otherwise stay "without a current allocation" and
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
    }

    impl WidgetImpl for GhostHost {
        fn measure(&self, _orientation: gtk4::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (1, 1, -1, -1)
        }

        fn size_allocate(&self, _width: i32, _height: i32, _baseline: i32) {
            self.allocations.set(self.allocations.get() + 1);
            self.allocate_children();
        }

        fn snapshot(&self, _snapshot: &gtk4::Snapshot) {
            let obj = self.obj();
            // No clip: GTK could cull a child outside it before its snapshot
            // vfunc runs, and that vfunc is what acknowledges WebKit's frame.
            let throwaway = gtk4::Snapshot::new();
            let mut child = obj.first_child();
            while let Some(c) = child {
                obj.snapshot_child(&c, &throwaway);
                self.snapshots.set(self.snapshots.get() + 1);
                child = c.next_sibling();
            }
            drop(throwaway.to_node());
        }
    }
}

glib::wrapper! {
    pub struct GhostHost(ObjectSubclass<imp::GhostHost>)
        @extends gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl GhostHost {
    pub fn new(stage_w: i32, stage_h: i32) -> Self {
        let host: Self = glib::Object::new();
        host.imp().stage.set((stage_w, stage_h));
        host
    }

    /// Hang a page here. It stays mapped while the host is in a mapped window.
    pub fn hang(&self, child: &impl IsA<gtk4::Widget>) {
        child.set_parent(self);
        self.queue_allocate();
    }

    pub fn snapshots(&self) -> u64 {
        self.imp().snapshots.get()
    }

    pub fn allocations(&self) -> u64 {
        self.imp().allocations.get()
    }

    pub fn self_driven(&self) -> u64 {
        self.imp().self_driven.get()
    }

    /// Also snapshot every child from a timer, independent of the window's
    /// frame clock. A window that keeps committing (the user's own content
    /// animates) waits for frame callbacks, and a compositor that withholds
    /// them from an occluded window stalls GTK's paint cycle — so the host's
    /// `snapshot` never runs and WebKit never gets frameDone. Calling
    /// `gtk_widget_snapshot_child` from a timer runs the web view's snapshot
    /// vfunc whenever WebKit queued a draw (the child's render node is cached
    /// otherwise, so idle ticks cost almost nothing).
    ///
    /// (A `GtkWidgetPaintable` does not work for this: it only refreshes its
    /// image when the widget is drawn by its window — measured, rAF stays 0.)
    pub fn self_drive(&self, every: Duration) -> glib::SourceId {
        let weak = self.downgrade();
        glib::timeout_add_local(every, move || {
            let Some(host) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            host.imp().allocate_children();
            let throwaway = gtk4::Snapshot::new();
            let mut child = host.first_child();
            while let Some(c) = child {
                host.snapshot_child(&c, &throwaway);
                host.imp().self_driven.set(host.imp().self_driven.get() + 1);
                child = c.next_sibling();
            }
            drop(throwaway.to_node());
            glib::ControlFlow::Continue
        })
    }
}
