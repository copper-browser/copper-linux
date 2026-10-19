//! Copper Cloud in the GTK app: Settings › Cloud ([`page`]), the palette's
//! cloud commands, the `history_delete` tool ([`mcp`]) and the QA hooks
//! ([`bench`](mod@bench), [`selftest`](mod@selftest)). The cloud itself — client, sync engine over
//! the shell ([`adapter`]), intelligence, runtime — is copper-shell-core's
//! [`copper_shell_core::cloud`], re-exported here, and so are single sign-on
//! and `copper://cloud/join` links ([`copper_shell_core::cloud::sso`],
//! [`copper_shell_core::cloud::join`]): the page only draws them, and
//! [`open_join`] carries a join link out.

pub mod bench;
pub mod page;
pub mod selftest;
pub mod sso_selftest;

use crate::actions::{Command, SettingsPage, register_command, register_settings_page};
use crate::platform::Glib;
use crate::shell::{GtkShell, Shell};
use copper_shell_core::cloud::join::{self, JoinAction};
use copper_shell_core::cloud::sso::Opener;
use std::future::Future;
use std::process::{Command as Process, Stdio};
use std::rc::Rc;
use std::sync::Arc;

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
    // No search in Settings on Linux: the palette finds "join link", "sso".
    register_command(
        shell,
        Command::new(
            "cloud-join",
            "Join Copper Cloud — Link Code, Join Link or SSO Sign-In…",
            "network-server-symbolic",
            |s| {
                if let Some(p) = open_settings(s) {
                    p.focus_code();
                }
            },
        )
        .when(|_| get().is_some_and(|c| !c.client.is_signed_in())),
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
    sso_selftest::maybe_start(shell);
}

/// The system browser at `url`, for single sign-on (and `copper cloud
/// sso`): `xdg-open`, or the command `COPPER_SSO_BROWSER` names (QA: a
/// fake browser, so a test never opens the desktop's own), with the address
/// as its one argument. The child is reaped on a thread of its own.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    let program = std::env::var("COPPER_SSO_BROWSER")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "xdg-open".into());
    let mut child = Process::new(&program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    std::thread::Builder::new()
        .name("browser-open".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map(drop)
        .map_err(|e| e.to_string())
}

/// A browser the self-test plays instead of the system's.
static FAKE_BROWSER: std::sync::Mutex<Option<Opener>> = std::sync::Mutex::new(None);

/// [`open_in_browser`] as single sign-on takes it (the self-test's own
/// browser while it runs).
pub fn opener() -> Opener {
    if let Some(fake) = FAKE_BROWSER.lock().expect("fake browser").clone() {
        return fake;
    }
    Arc::new(open_in_browser)
}

/// Play the system browser (`None`: the real one again). QA only.
pub(crate) fn set_fake_browser(open: Option<Opener>) {
    *FAKE_BROWSER.lock().expect("fake browser") = open;
}

/// A `copper://cloud/join?code=…` link (the command line, a second `copper`
/// handing it over, the address field): Settings › Cloud with the code, the
/// sign-in started at once for a join link (`#sso=`), and never another
/// instance without a press ([`join::handle`]). `false`: not such a link —
/// an address like any other.
pub fn open_join(shell: &Shell, text: &str) -> bool {
    let action = join::handle(text, opener());
    match &action {
        JoinAction::NotJoin => return false,
        JoinAction::Off => {
            if let Some(m) = action.message() {
                shell.toast(&m);
            }
            return true;
        }
        _ => {}
    }
    let page = open_settings(shell);
    // "This link is for HOST" is the page's own notice (with Dismiss);
    // the rest is said there once.
    if let Some(m) = action.message()
        && !matches!(action, JoinAction::OtherInstance { .. })
    {
        match &page {
            Some(p) => p.say(&m),
            None => shell.toast(&m),
        }
    }
    true
}

/// Settings, on the Cloud page (the Settings already open, if it is). The
/// page, once built.
pub fn open_settings(shell: &Shell) -> Option<Rc<page::CloudPage>> {
    use libadwaita::prelude::*;
    if let Some(open) = shell
        .window()
        .visible_dialog()
        .and_then(|d| d.downcast::<libadwaita::PreferencesDialog>().ok())
        && let Some(p) = page::current()
    {
        open.set_visible_page_name("cloud");
        return Some(p);
    }
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name("cloud");
    page::current()
}
