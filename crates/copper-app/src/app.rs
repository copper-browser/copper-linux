//! The application: one `adw::Application` (`com.copperbrowser.Copper`), one
//! window. A second `copper example.com` hands its addresses to the running
//! one. Scripted runs (`--screenshot`, `--selftest-ui`) and test worlds
//! (`COPPER_WORLD`) never join a running Copper.

use crate::control;
use crate::options::Options;
use crate::shell::{OpenTab, Shell};
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;
use std::process::ExitCode;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
}

/// The process exit code a scripted run sets.
static EXIT: AtomicI32 = AtomicI32::new(0);

/// Set the exit code (the self-test's verdict).
pub fn set_exit(code: i32) {
    EXIT.store(code, Ordering::SeqCst);
}

pub fn run(opts: Options) -> ExitCode {
    let world = std::env::var("COPPER_WORLD").is_ok_and(|w| !w.trim().is_empty());
    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    if opts.scripted() || world {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::new(Some(crate::APP_ID), flags);
    app.connect_command_line(|app, cmd| {
        let args: Vec<String> = cmd
            .arguments()
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let opts = Options::parse(args).unwrap_or_default();
        activate(app, opts);
        glib::ExitCode::SUCCESS
    });
    app.connect_shutdown(|_| {
        SHELL.with(|s| {
            if let Some(shell) = s.borrow_mut().take() {
                shell.shutdown();
            }
        });
    });
    let args: Vec<String> = std::env::args().collect();
    let code = app.run_with_args(&args);
    let scripted = EXIT.load(Ordering::SeqCst);
    if scripted != 0 {
        return ExitCode::from(scripted.clamp(1, 255) as u8);
    }
    if code == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn activate(app: &adw::Application, opts: Options) {
    let existing = SHELL.with(|s| s.borrow().clone());
    let shell = match existing {
        Some(s) => {
            s.window().present();
            s
        }
        None => {
            let shell = Shell::new(app, opts.clone());
            control::install(&shell);
            SHELL.with(|s| *s.borrow_mut() = Some(shell.clone()));
            hooks(&shell, &opts);
            shell
        }
    };
    for (i, address) in opts.open.iter().enumerate() {
        if let Some(url) = command_line_url(address).or_else(|| shell.destination(address)) {
            shell.open(
                &url,
                OpenTab {
                    foreground: i == 0,
                    ..Default::default()
                },
            );
        }
    }
    if shell.active_id().is_some()
        && let Some(p) = shell.active_page()
    {
        crate::palette::p_focus(&p);
    }
}

/// An argument that is already a whole address (`data:` with spaces
/// included) or a file that exists.
pub fn command_line_url(arg: &str) -> Option<String> {
    if let Ok(u) = url::Url::parse(arg)
        && matches!(u.scheme(), "http" | "https" | "file" | "data" | "about")
    {
        return Some(u.to_string());
    }
    let path = std::path::Path::new(arg);
    if (arg.starts_with('/') || arg.starts_with('.')) && path.exists() {
        let abs = std::fs::canonicalize(path).ok()?;
        return url::Url::from_file_path(abs).ok().map(|u| u.to_string());
    }
    None
}

/// The debug hooks: `--screenshot`, `--selftest-ui`.
fn hooks(shell: &Shell, opts: &Options) {
    if let Some(path) = opts.screenshot.clone() {
        let weak = shell.downgrade();
        glib::spawn_future_local(async move {
            settle(&weak, Duration::from_secs(10)).await;
            let Some(s) = weak.upgrade() else { return };
            match crate::screenshot::save(s.window().upcast_ref(), &path) {
                Ok(()) => println!("copper: saved {}", path.display()),
                Err(e) => {
                    eprintln!("copper: screenshot failed: {e}");
                    set_exit(1);
                }
            }
            s.shutdown();
            s.app().quit();
        });
    }
    #[cfg(debug_assertions)]
    if let Some(phase) = opts.selftest.clone() {
        let out = opts
            .selftest_out
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("copper-selftest"));
        crate::selftest::start(shell, phase, out);
    }
}

/// Wait until the window is drawn and the selected page has loaded (or
/// `limit` passes), then a beat more for the last paint.
pub async fn settle(weak: &crate::shell::WeakShell, limit: Duration) {
    use copper_engine::PageView;
    let start = Instant::now();
    loop {
        glib::timeout_future(Duration::from_millis(100)).await;
        let Some(s) = weak.upgrade() else { return };
        let drawn = s.window().width() > 0;
        let loaded = match s.active_page() {
            Some(p) => !p.is_loading() && p.url().is_some(),
            None => true,
        };
        if (drawn && loaded) || start.elapsed() > limit {
            break;
        }
    }
    glib::timeout_future(Duration::from_millis(600)).await;
}

#[cfg(test)]
mod tests {
    use super::command_line_url;

    #[test]
    fn command_line_addresses() {
        assert_eq!(
            command_line_url("data:text/html,<b>a b</b>").as_deref(),
            Some("data:text/html,<b>a b</b>")
        );
        assert_eq!(command_line_url("https://example.com").as_deref(), Some("https://example.com/"));
        assert_eq!(command_line_url("example.com"), None, "left to the field's rules");
        assert_eq!(command_line_url("rust lifetimes"), None);
        assert!(command_line_url("/").unwrap().starts_with("file:///"));
    }
}
