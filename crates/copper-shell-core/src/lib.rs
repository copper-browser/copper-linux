//! Copper's browser shell without a UI toolkit.
//!
//! Everything the browser *is* apart from its widgets and its web engine:
//! the [`Shell`] (copper-core's tabs, spaces, history, bookmarks and
//! settings, plus one live engine page per awake tab and what its events
//! do), sessions on disk, downloads, crash records, private tabs, space
//! colours and the thread-safe [`control`] API automation drives it with;
//! and the lanes on top of it — the copper-cloud sync ([`cloud`]), Canvas
//! ([`canvas`]), the agents' MCP server ([`agents`]), the agent in the
//! window ([`chat`]), the Calendar pin's card ([`calendar`]), passwords
//! and form fill ([`forms`]), the ad blocker ([`shield`]), hide-anything
//! ([`curtain`]), the update check ([`updates`]), the palette's matching
//! ([`palette`]) and the shell's words for times and sizes ([`format`](mod@format)).
//! Linux (GTK4 + WebKitGTK, `copper-app`) and Windows (Win32 +
//! DirectComposition + WebView2) each plug three things into it, named by
//! one [`Platform`] type:
//!
//! * an [`Engine`] (`copper-engine`'s traits: `WebKitEngine`, the WebView2
//!   engine);
//! * a [`MainLoop`] (GLib's default context, `copper-win-rt`'s loop);
//! * a [`ShellView`]: the window, told what changed and asked small
//!   questions ([`view`]).
//!
//! A lane that draws something of its own takes the app's widgets through
//! a small trait of its own ([`agents::AgentsApp`], [`forms::FormsUi`],
//! [`updates::UpdatesApp`]).
//!
//! Everything here is main-thread only (`Rc`, `!Send` engine pages); other
//! threads reach the shell through [`control::Control`]. See `README.md`
//! for how an app is put together and what is still to move here.

pub mod agents;
pub mod archive;
pub mod calendar;
pub mod canvas;
pub mod chat;
pub mod clear;
pub mod cloud;
pub mod control;
pub mod cover;
pub mod crash;
pub mod curtain;
pub mod downloads;
pub mod folders;
pub mod format;
pub mod forms;
pub mod main_loop;
pub mod options;
pub mod palette;
pub mod private;
pub mod recent;
pub mod session;
pub mod shell;
pub mod shield;
pub mod sidedrop;
pub mod sleep;
pub mod spaces;
pub mod updates;
pub mod util;
pub mod view;

use copper_engine::Engine;

pub use main_loop::{MainLoop, Task, spawn, timeout};
pub use shell::{OpenTab, Shell, TabView, Visible, WeakShell};
pub use view::{PageFrame, PageStatus, Question, Refresh, ShellView};

/// The toolkit, engine and loop one app runs the shell on. Implemented by
/// a marker type (`copper-app`'s `Gtk`); the shell's types are generic over
/// it (`Shell<Gtk>`), and apps name them with aliases
/// (`type Shell = copper_shell_core::Shell<Gtk>`).
pub trait Platform: Sized + 'static {
    /// The web engine. A cheap handle (an `Rc` inside), so async work —
    /// compiling the ad-block list — can hold one without the shell.
    type Engine: Engine + Clone;
    /// The UI thread's main loop.
    type Loop: MainLoop;
    /// The window the shell is drawn in.
    type View: ShellView<Self>;
    /// A decoded favicon, ready to draw (`gdk::Texture`).
    type Icon: Clone + std::fmt::Debug + 'static;
    /// A tab's page in its frame on the stage ([`view::PageFrame`]).
    type Frame: PageFrame;

    /// The languages pages are told the user reads, best first, as BCP 47
    /// tags. The default reads them the way GLib does (`LANGUAGE`,
    /// `LC_ALL`, `LC_MESSAGES`, `LANG`; [`shell::languages`]); a platform
    /// with its own list (Windows' user languages) returns that instead.
    fn preferred_languages() -> Vec<String> {
        shell::languages::from_env()
    }

    /// The person signed in to this computer, as they would be named (the
    /// full name when there is one, else the login name): a signed-out
    /// canvas's own persona.
    fn user_name() -> String;

    /// This computer's memory, total and available, in bytes: the tab
    /// sleeper's budget and how short memory is ([`sleep`]). The default
    /// reads `/proc/meminfo` on Linux.
    fn memory() -> Option<(u64, u64)> {
        sleep::system_memory()
    }

    /// The picture the engine kept of `page` when it was last hidden — a
    /// JPEG and its size in pixels — for the wake cover ([`cover`]), where
    /// the engine can't picture a hidden page on demand. Default: none (the
    /// page is pictured as it is now).
    fn kept_picture(_page: &PageOf<Self>) -> Option<(Vec<u8>, u32, u32)> {
        None
    }
}

/// The engine's page type for platform `P`.
pub type PageOf<P> = <<P as Platform>::Engine as Engine>::Page;
/// The engine's profile type for platform `P`.
pub type ProfileOf<P> = <<P as Platform>::Engine as Engine>::Profile;
