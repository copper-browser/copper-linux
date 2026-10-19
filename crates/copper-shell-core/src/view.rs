//! What the shell asks of the window it is drawn in.
//!
//! The shell model ([`crate::Shell`]) decides *what* changed and *what* to
//! say; a [`ShellView`] draws it with a real toolkit (GTK4 + libadwaita in
//! `copper-app`, Win32 + DirectComposition on Windows). Each call is a
//! notification or a small question, made on the main thread, and never
//! while the shell holds a borrow of its own state, so a view may call back
//! into the shell from any of them.

use std::path::Path;
use std::rc::Rc;

use copper_core::TabId;
use copper_core::settings::{Look, TabsPosition};

use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use crate::{PageOf, Platform};

/// What needs redrawing.
#[derive(Debug, Clone, Default)]
pub struct Refresh {
    pub all: bool,
    /// The lists of tabs (order, membership, pins).
    pub tabs: bool,
    /// Spaces: names, colours, which is current.
    pub spaces: bool,
    /// Which tab is selected.
    pub selection: bool,
    /// The field, the back/forward buttons, the private tint.
    pub chrome: bool,
    /// Tabs whose title, icon or state changed.
    pub changed: Vec<TabId>,
}

impl Refresh {
    pub fn all() -> Refresh {
        Refresh {
            all: true,
            ..Default::default()
        }
    }

    pub fn spaces() -> Refresh {
        Refresh {
            spaces: true,
            tabs: true,
            chrome: true,
            ..Default::default()
        }
    }

    pub fn chrome() -> Refresh {
        Refresh {
            chrome: true,
            ..Default::default()
        }
    }

    pub fn tab(id: &TabId) -> Refresh {
        Refresh {
            changed: vec![id.clone()],
            ..Default::default()
        }
    }
}

/// The message a page's frame lays over the page when it crashed or failed
/// to load (§7), with one button that reloads the tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageStatus {
    pub title: String,
    pub description: String,
    /// The button's label (`Reload`, `Try Again`).
    pub button: String,
}

/// A yes/no question for the user, over the window (a site asking for the
/// camera).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The whole question, one sentence (`Allow example.com to use your
    /// camera?`).
    pub title: String,
    /// More words under it, if any.
    pub detail: Option<String>,
    /// The refusing button; also what closing the question means.
    pub deny: String,
    /// The accepting button.
    pub allow: String,
}

/// A tab's page as it sits on the stage: the page's own view, and room to
/// lay a [`PageStatus`] over it. Made by [`ShellView::new_frame`]; its
/// button reloads the tab ([`Shell::reload`]).
pub trait PageFrame: 'static {
    /// Show `status` over the page.
    fn show_status(&self, status: &PageStatus);
    /// Take the status away (the page shows again).
    fn hide_status(&self);
}

/// The window the shell is drawn in. Main thread only.
///
/// Every method is a notification the view acts on right away; none may
/// block. A view built in steps (GTK's widgets need the shell to exist) may
/// ignore calls that come before it is complete.
pub trait ShellView<P: Platform>: 'static {
    // -- pages ---------------------------------------------------------------
    /// A frame for a new tab page: the page's view inside it, the status
    /// hidden, its button wired to [`Shell::reload`] of `tab`.
    fn new_frame(&self, shell: &Shell<P>, tab: &TabId, page: &PageOf<P>) -> P::Frame;
    /// A tab got a live page ([`Shell::adopt_page`]): put its frame on the
    /// stage (hidden until [`ShellView::show_tab`] picks it).
    fn page_added(&self, shell: &Shell<P>, slot: &Rc<PageSlot<P>>);
    /// A tab's page is gone (slept, closed, rebuilt): take its frame away.
    fn page_removed(&self, shell: &Shell<P>, slot: &Rc<PageSlot<P>>);
    /// Show the selected tab's page (`None`: the space has nothing open, so
    /// the empty stage and the launch card).
    fn show_tab(&self, shell: &Shell<P>, tab: Option<&TabId>);

    // -- redraws -------------------------------------------------------------
    /// Bring the tab lists, spaces and chrome in line with the shell.
    fn refresh(&self, shell: &Shell<P>, what: &Refresh);
    /// Tabs down the left or across the top.
    fn set_layout(&self, shell: &Shell<P>, position: TabsPosition);
    /// The selected page's progress line: shown while `loading`.
    fn set_progress(&self, loading: bool, fraction: f64);
    /// The address of the link under the pointer on the selected page.
    fn set_hover(&self, url: Option<&str>);
    /// Light, dark or the system's.
    fn apply_look(&self, look: Look);
    /// The window is dark right now.
    fn is_dark(&self) -> bool;
    /// One short line at the foot of the window.
    fn toast(&self, text: &str);
    /// Ask `question`; `answer` gets `true` for allow. Never called back
    /// re-entrantly from inside `ask`.
    fn ask(&self, shell: &Shell<P>, question: Question, answer: Box<dyn FnOnce(bool)>);
    /// The window is up and the session restored: show it.
    fn present(&self, shell: &Shell<P>);

    // -- questions -----------------------------------------------------------
    /// The network is reachable (a failed load says "You're offline" when
    /// not).
    fn is_online(&self) -> bool;
    /// The downloads list is on screen (it then says what finished, not a
    /// toast).
    fn showing_downloads(&self) -> bool;
    /// Close the side panel (History, Downloads, Hidden…) if one is open:
    /// picking something to hide wants the whole page.
    fn hide_panel(&self);
    /// A PNG of the whole window (automation's screenshot).
    fn screenshot(&self) -> Option<Vec<u8>>;

    // -- the window ------------------------------------------------------------
    /// Open Settings on the page named `page` (`"cloud"`): where a canvas
    /// link that needs Copper Cloud sends the user.
    fn open_settings(&self, shell: &Shell<P>, page: &str);
    /// Make the window `width` × `height` logical pixels (`browser_resize`).
    fn resize_window(&self, width: i32, height: i32);

    // -- icons ---------------------------------------------------------------
    /// A favicon from its PNG bytes; `None` when it does not decode.
    fn decode_icon(&self, png: &[u8]) -> Option<P::Icon>;
    /// A favicon kept on disk; `None` when it is missing or unreadable.
    fn load_icon(&self, path: &Path) -> Option<P::Icon>;
}
