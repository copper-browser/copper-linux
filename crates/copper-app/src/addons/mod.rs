//! Addons: Copper's slots, filled from Copper Cloud (Mac: `Fork/Addons/`,
//! `docs/addons.md`).
//!
//! A slot is a region of the window whose content an addon may replace. v1
//! has one, the **Agent sidebar** (`agent-sidebar`): the pane right of the
//! page, toggled by Ctrl+E. With no addon it shows Copper's own
//! placeholder; with one it shows the addon's page, served from its
//! installed bundle as `copper-addon://<id>/` in a web view of its own, and
//! speaking to Copper through `window.copper` (bridge v1).
//!
//! | Module | What | Mac |
//! |---|---|---|
//! | [`pane`] | the slot on screen: revealer, width grip, placeholder | `Fork/Addons/AddonSlots.swift` (`AgentSlot`) |
//! | [`view`] | the addon's web view: profile, scripts, navigation rules, events | `Fork/Addons/AddonHost.swift` |
//! | [`scheme`] | `copper-addon://` from the installed bundle | `Fork/Addons/AddonHost.swift` (`AddonFiles`) |
//! | [`policy`] | WebKit's `decide-policy`, and the pure address rules | `Fork/Addons/AddonHost.swift` |
//! | [`host`] | what the bridge asks of Copper (identity, tabs, close, sign-in) | `Fork/Addons/AddonBridge.swift` |
//! | [`oauth`] | the sign-in window for `copper.oauth.authorize` | `Fork/Addons/AddonBridge.swift` |
//! | [`settings`] | Settings › Addons | `Fork/Addons/AddonsSettings.swift` |
//! | [`updates`] | "Update available" on Settings › Updates, the toast, Install | `Fork/Addons/AddonUpdates.swift` |
//! | [`core_glue`] | the one door to `copper_core::addons` and `copper_cloud::addons` | — |
//!
//! Everything platform-neutral (manifests, bundles, the on-disk store, the
//! cloud sync, the bridge's own methods) lives in the shared core; this lane
//! is the GTK side only. Every call into the core goes through
//! [`core_glue`], so the shape of the core is known in one file.

pub mod core_glue;
pub mod host;
pub mod oauth;
pub mod pane;
pub mod policy;
pub mod scheme;
#[cfg(debug_assertions)]
pub mod selftest;
#[cfg(debug_assertions)]
pub mod selftest_cloud;
pub mod settings;
pub mod updates;
pub mod view;

use crate::actions::{Command, register_action, register_command, register_menu_item};
use crate::shell::{Shell, WeakShell};
use std::cell::RefCell;

/// The slot's id (CONTRACT §1).
pub const SLOT: &str = "agent-sidebar";
/// The slot's name, as people read it.
pub const SLOT_NAME: &str = "Agent sidebar";
/// The window action that shows and hides the slot.
pub const ACTION: &str = "toggle-agent";

thread_local! {
    static SHELL: RefCell<Option<WeakShell>> = const { RefCell::new(None) };
}

/// The shell, on the main thread, while it lives.
pub(crate) fn shell() -> Option<Shell> {
    SHELL.with(|s| s.borrow().as_ref().and_then(WeakShell::upgrade))
}

/// The slot itself, mounted by the stage as its third child (after the side
/// panels). Built empty; what it shows is made on first use.
pub fn mount(shell: &Shell, stage: &gtk4::Box) {
    pane::mount(shell, stage);
}

/// Called once from `actions::register_lanes`, after the cloud lane.
pub fn register(shell: &Shell) {
    SHELL.with(|s| *s.borrow_mut() = Some(shell.downgrade()));
    pane::install_css();
    // Before any addon page: WebKit must know the scheme before it is asked.
    scheme::install();

    register_action(shell, ACTION, &["<Control>e"], toggle);
    register_command(
        shell,
        Command::new(
            "agent-sidebar",
            SLOT_NAME,
            "sidebar-show-right-symbolic",
            toggle,
        )
        .shortcut("Ctrl+E"),
    );
    register_menu_item(shell, SLOT_NAME, &format!("win.{ACTION}"));

    core_glue::start(shell, changed);
    view::watch_shell(shell);
    #[cfg(debug_assertions)]
    selftest::maybe_start(shell);
    #[cfg(debug_assertions)]
    selftest_cloud::maybe_start(shell);
}

/// Ctrl+E: show the slot, or put it away.
pub fn toggle(shell: &Shell) {
    pane::toggle(shell);
}

/// Show the slot (and give it the keyboard).
pub fn show(shell: &Shell) {
    pane::show(shell);
}

/// Put the slot away (Ctrl+E again, `copper.ui.close()`).
pub fn hide(shell: &Shell) {
    pane::hide(shell);
}

/// Whether the slot is on screen.
pub fn showing() -> bool {
    pane::showing()
}

/// The addons, their state or the cloud's answer changed: bring the slot,
/// Settings › Addons and Settings › Updates in line. Main thread.
pub fn changed(shell: &Shell) {
    pane::sync(shell);
    settings::redraw(shell);
    updates::changed(shell);
}

/// Settings, on the Addons page.
pub fn open_settings(shell: &Shell) {
    settings::present(shell);
}
