//! Copper Cloud in the GTK app: Settings › Cloud ([`page`]), the palette's
//! cloud commands, the `history_delete` tool ([`mcp`]) and the QA hooks
//! ([`bench`](mod@bench), [`selftest`](mod@selftest)). The cloud itself — client, sync engine over
//! the shell ([`adapter`]), intelligence, runtime — is copper-shell-core's
//! [`copper_shell_core::cloud`], re-exported here.

pub mod bench;
pub mod page;
pub mod selftest;

use crate::actions::{Command, SettingsPage, register_command, register_settings_page};
use crate::platform::Glib;
use crate::shell::Shell;
use std::future::Future;
use std::rc::Rc;

pub use copper_shell_core::cloud::{
    Cloud, adapter, ago, count, get, history_cleared, history_forgot, mark_seen, mcp, runtime,
    spawn, sync_now,
};

/// Run `f` on the cloud runtime and hand its output to `then` on the GTK
/// main thread (never blocking it).
pub fn run_then<F, T>(f: F, then: impl FnOnce(F::Output) + 'static)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    copper_shell_core::cloud::run_then::<Glib, F, T>(f, then);
}

/// The shell, on the main thread, while it lives.
pub(crate) fn shell() -> Option<Shell> {
    copper_shell_core::shell::current()
}

/// Open the cloud ([`copper_shell_core::cloud::start`]) and add Settings ›
/// Cloud and the commands — unless `cloud.json` could not be read, when
/// Copper Cloud is off this run.
pub fn register(shell: &Shell) {
    if copper_shell_core::cloud::start(shell).is_none() {
        return;
    }

    register_settings_page(
        shell,
        SettingsPage {
            id: "cloud".into(),
            title: "Cloud".into(),
            icon: "network-server-symbolic".into(),
            build: Rc::new(page::build),
        },
    );
    register_command(
        shell,
        Command::new(
            "copper-cloud",
            "Copper Cloud…",
            "network-server-symbolic",
            |s| {
                open_settings(s);
            },
        ),
    );
    register_command(
        shell,
        Command::new(
            "pair-device",
            "Pair another device with Copper Cloud",
            "phone-symbolic",
            |s| {
                if let Some(p) = open_settings(s) {
                    p.make_code();
                }
            },
        )
        .when(|_| get().is_some_and(|c| c.client.is_signed_in())),
    );
    register_command(
        shell,
        Command::new(
            "cloud-sync-now",
            "Sync Now",
            "emblem-synchronizing-symbolic",
            |s| {
                sync_now();
                s.toast("Syncing with Copper Cloud");
            },
        )
        .when(|_| get().is_some_and(|c| c.client.is_signed_in() && c.sync.is_on())),
    );
    bench::start_if_asked(shell);
    selftest::maybe_start(shell);
}

/// Settings, on the Cloud page. The page, once built.
pub fn open_settings(shell: &Shell) -> Option<Rc<page::CloudPage>> {
    use libadwaita::prelude::*;
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name("cloud");
    page::current()
}
