//! Pages an agent works in while nobody is looking at them (R-012, D-006).
//!
//! A page in a tab that is not on screen is a hidden page to the engine:
//! `requestAnimationFrame` stops, `document.hidden` is true, screenshots come
//! back stale and trusted clicks have nowhere to land. While an agent works in
//! such a tab, the shell asks the [`Backstage`] to *hold* it: the page is kept
//! laid out at stage size, rendering and frame-acknowledged inside one of the
//! shell's windows, without ever being drawn there, taking focus or showing
//! anything (menus, pickers, prompts) over that window.

use serde::{Deserialize, Serialize};

use crate::{PageView, Result};

/// Keeps pages alive and visible to their engine while they are not shown.
///
/// Policy (which tabs, how long to linger after the agent's last call) is the
/// caller's; this is only the mechanism. Main-thread only, like every engine
/// object.
pub trait Backstage {
    type Page: PageView;
    /// A shell window a page can be kept in while it is not shown.
    type Host;

    /// Keep `page` rendering while it is not shown, inside `host`. Idempotent;
    /// holding a held page again moves it to `host` if it lives elsewhere.
    ///
    /// A page the shell is showing right now is left where it is (and is
    /// still counted as held, so it comes back here when the shell lets go of
    /// it). A page sitting in a shell container that hides it is refused with
    /// `InvalidArgument`: take it out of that container first.
    ///
    /// While a page is held and not shown, nothing it opens appears over the
    /// host: context menus, `<select>` popups (kept for keyboard input),
    /// file and colour pickers, print dialogs and full screen are swallowed,
    /// and permission requests are denied without a `PermissionRequest` event.
    fn hold(&self, page: &Self::Page, host: &Self::Host) -> Result<()>;

    /// Let go: back to an ordinary background tab (hidden, throttled, free to
    /// sleep). The page is taken out of the backstage host if it is there.
    fn release(&self, page: &Self::Page);

    /// Whether `page` is held (shown or not).
    fn holds(&self, page: &Self::Page) -> bool;

    /// The shell is about to show a held page (the user picked its tab): take
    /// it out of the backstage host so the shell can put it in a visible
    /// container *in the same main-loop turn*. It stays held; when the shell
    /// takes it out of that container again, the backstage takes it back.
    fn hand_off(&self, page: &Self::Page);

    /// For `copper tabs` and the bench (macOS `Backstage.report`).
    fn status(&self) -> BackstageStatus;
}

/// What the backstage is doing right now. (Engines build it from
/// `Default` and fill the fields in.)
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BackstageStatus {
    /// Pages held, shown or not.
    pub held: usize,
    /// Held pages kept off screen in a host window.
    pub hosted: usize,
    /// Held pages the shell is showing.
    pub shown: usize,
    pub mode: BackstageMode,
    /// Size (logical px) every hosted page is laid out at.
    pub stage: (u32, u32),
    /// Windows hosting at least one page.
    pub hosts: usize,
    /// Times a host was driven from the timer because its window was not
    /// painting (a stalled or idle frame clock).
    pub nudges: u64,
    /// Popups, pickers, dialogs and prompts swallowed for held pages.
    pub popups_blocked: u64,
}

/// How well held pages are hosted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackstageMode {
    /// Nothing is held.
    #[default]
    Idle,
    /// Every held page is shown, or hosted in a mapped window that is
    /// neither minimized nor suspended: rendering at full rate.
    InWindow,
    /// Some held page has no usable host (its window closed, was hidden,
    /// minimized or suspended): script and input still work, but the engine
    /// may treat the page as hidden (rAF paused, snapshots stale or failing)
    /// until it is held again in a usable window.
    Degraded,
}
