//! The ad blocker's palette rows (Mac: the per-site switch in Settings ›
//! Privacy, which has it too). The blocker itself — the list, compiling
//! it, every page's filter, the settings — is copper-shell-core's
//! [`copper_shell_core::shield`], re-exported here.

pub use copper_shell_core::shield::*;

use crate::actions::{Command, register_command};
use crate::shell::Shell;

/// Start the blocker and add its palette rows.
pub fn register(shell: &Shell) {
    start(shell);
    commands(shell);
}

/// Off or on for the site in front, and on everywhere when it is off.
fn commands(shell: &Shell) {
    let on_here = |s: &Shell| {
        crate::features::curtain::here(s).is_some_and(|h| enabled(s) && !paused_on(s, &h))
    };
    let off_here = |s: &Shell| {
        crate::features::curtain::here(s).is_some_and(|h| enabled(s) && paused_on(s, &h))
    };
    register_command(
        shell,
        Command::new(
            "shield-off-here",
            "Turn Off Ad Blocker for This Site",
            "security-low-symbolic",
            |s| here(s, true),
        )
        .when(on_here),
    );
    register_command(
        shell,
        Command::new(
            "shield-on-here",
            "Turn On Ad Blocker for This Site",
            "security-high-symbolic",
            |s| here(s, false),
        )
        .when(off_here),
    );
    register_command(
        shell,
        Command::new(
            "shield-on",
            "Turn On Ad Blocker",
            "security-high-symbolic",
            |s| {
                set_enabled(s, true);
                s.toast("Blocking ads and trackers");
            },
        )
        .when(|s| !enabled(s)),
    );
}

/// The per-site switch, for the site in front.
fn here(shell: &Shell, off: bool) {
    if let Some(site) = crate::features::curtain::here(shell) {
        toggle_here(shell, &site, off);
    }
}
