//! Canvas in the GTK app: the "New Canvas" action and the palette's canvas
//! commands. Canvas itself — the `copper://canvas` page, the bridge, the
//! list, the history on disk, the `canvas_*` tools — is copper-shell-core's
//! [`copper_shell_core::canvas`], re-exported here.

pub use copper_shell_core::canvas::*;

use crate::actions::{Command, register_action, register_command};
use crate::shell::Shell;

/// Turn canvas on ([`start`]) and add its action and commands.
pub fn register(shell: &Shell) {
    start(shell);

    register_action(shell, "new-canvas", &["<Control><Shift>e"], |s| {
        new_canvas(s, None)
    });
    register_command(
        shell,
        Command::new("new-canvas", "New Canvas", "document-edit-symbolic", |s| {
            new_canvas(s, None)
        })
        .shortcut("Ctrl+Shift+E"),
    );
    register_action(shell, "personal-canvas", &["<Control><Shift>o"], personal);
    register_command(
        shell,
        Command::new(
            "personal-canvas",
            "Personal Canvas",
            "document-edit-symbolic",
            personal,
        )
        .shortcut("Ctrl+Shift+O"),
    );
}

/// Ctrl+Shift+O (Mac ⇧⌘O): your Personal canvas.
fn personal(s: &Shell) {
    show(s, copper_core::canvas::PERSONAL_CANVAS_ID, true);
}
