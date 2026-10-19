//! The shell, on GTK: copper-shell-core's [`copper_shell_core::Shell`] for
//! the [`Gtk`] platform, under the names the rest of the app uses
//! ([`Shell`], [`WeakShell`], [`pages::PageSlot`], [`TabView`]), plus what
//! only the GTK app reaches through it ([`GtkShell`]: the window, the
//! widgets, the action registry).
//!
//! [`build`] makes the one shell of the process: the model and its window,
//! then the widgets, then every action and lane, then the session on
//! screen.

use crate::actions::Registry;
use crate::options::Options;
use crate::platform::{Gtk, GtkView};
use crate::window::Ui;
use copper_engine_webkitgtk::WebKitEngine;
use gtk4::gdk;
use libadwaita as adw;
use std::cell::RefCell;

pub use copper_shell_core::shell::{OpenTab, host_of, languages};

/// The shell (cheap to clone; main thread only).
pub type Shell = copper_shell_core::Shell<Gtk>;
/// A weak handle, for closures that widgets and pages keep.
pub type WeakShell = copper_shell_core::WeakShell<Gtk>;
/// What the tab lists draw for one tab.
pub type TabView = copper_shell_core::TabView<gdk::Texture>;
/// The current space's tabs, as the column lays them out.
pub type Visible = copper_shell_core::Visible<gdk::Texture>;

/// Pages: one live `WebKitPage` per awake tab, in its [`GtkFrame`].
///
/// [`GtkFrame`]: crate::platform::GtkFrame
pub mod pages {
    pub use copper_shell_core::shell::pages::{icon_rel, profile_id, profile_key};

    /// A tab's live page and the frame it sits in on the stage.
    pub type PageSlot = copper_shell_core::shell::pages::PageSlot<crate::platform::Gtk>;
}

/// Build the shell and its window, restore the session, show the window.
pub fn build(app: &adw::Application, options: Options) -> Shell {
    let shell = Shell::new(options, WebKitEngine::new, |options| {
        GtkView::new(app, options)
    });
    shell.view().set_ui(Ui::build(&shell));
    crate::actions::register_core(&shell);
    crate::actions::register_lanes(&shell);
    shell.start();
    shell
}

/// What the GTK app reaches through the shell besides the model: its
/// view's window, widgets and registry.
pub trait GtkShell {
    /// Every widget the shell talks to.
    fn ui(&self) -> &Ui;
    fn window(&self) -> &adw::ApplicationWindow;
    fn app(&self) -> &adw::Application;
    /// What the lanes added: commands, settings pages, panels, menu items.
    fn registry(&self) -> &RefCell<Registry>;
    /// A new private tab, in front, with the card asking where to.
    fn new_private_tab(&self);
}

impl GtkShell for Shell {
    fn ui(&self) -> &Ui {
        self.view().ui()
    }

    fn window(&self) -> &adw::ApplicationWindow {
        self.view().window()
    }

    fn app(&self) -> &adw::Application {
        self.view().app()
    }

    fn registry(&self) -> &RefCell<Registry> {
        self.view().registry()
    }

    fn new_private_tab(&self) {
        self.ui()
            .card
            .open(self, crate::palette::Mode::LaunchPrivate);
    }
}
