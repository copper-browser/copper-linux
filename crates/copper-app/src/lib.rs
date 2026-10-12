//! Copper for Linux: the GTK4 / libadwaita browser shell.
//!
//! Module map (who owns what, for the lanes that build on this shell):
//!
//! | Module | What |
//! |---|---|
//! | [`app`] | `adw::Application`, startup, single window |
//! | [`shell`] | [`shell::Shell`]: the state (copper-core) + every operation the UI, the actions and [`control`] use; pages per tab |
//! | [`window`] | the window: tab column / tab row, the stage (page stack, progress line, find bar, crash and error pages) |
//! | [`palette`] | the one field: ⌘L / ⌘T / ⌘K card with inline completion and suggestions |
//! | [`spaces`] | space colours and tint, header, chips, menu, editor, delete dialog |
//! | [`session`] | loading and saving `session.json`, history, settings |
//! | [`private`] | private tabs: ephemeral profiles, private tint |
//! | [`crash`] | the crashed-page state and `crashes.json` |
//! | [`downloads`] | where downloads go and who hears about them (the panel is a later lane) |
//! | [`actions`] | every action + shortcut, and the `register_*` hooks later lanes add theirs through |
//! | [`control`] | the internal command API automation (MCP) drives the app with |
//! | [`screenshot`] | PNG of the whole window (debug QA) |

pub mod actions;
pub mod app;
pub mod control;
pub mod crash;
pub mod downloads;
pub mod options;
pub mod palette;
pub mod private;
pub mod screenshot;
#[cfg(debug_assertions)]
pub mod selftest;
pub mod session;
pub mod shell;
pub mod spaces;
pub mod style;
pub mod window;

pub use shell::Shell;

/// The application id (D-Bus name, desktop file, icon).
pub const APP_ID: &str = "com.copperbrowser.Copper";

/// Entry point of the `copper` binary.
pub fn main() -> std::process::ExitCode {
    use std::process::ExitCode;
    let filter = tracing_subscriber::EnvFilter::try_from_env("COPPER_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,copper_app=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();

    let opts = match options::Options::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("copper: {e}\n\n{}", options::USAGE);
            return ExitCode::from(2);
        }
    };
    if opts.help {
        print!("{}", options::USAGE);
        return ExitCode::SUCCESS;
    }
    if opts.version {
        println!("copper {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if cfg!(not(debug_assertions)) && opts.scripted() {
        eprintln!("copper: --screenshot and --selftest-ui exist only in debug builds");
        return ExitCode::from(2);
    }
    app::run(opts)
}
