//! Copper for Linux: the GTK4 / libadwaita browser shell (binary `copper`).
//!
//! `copper --mcp-stdio` (the stdio bridge), `copper <verb>` /
//! `copper --cli <verb>` (the agent command line) and `copper cloud …`
//! (pairing with no window) are answered here without a display; everything
//! else is the browser (`copper_app::main`, which also prints
//! `copper --version` before GTK starts).

use std::process::{Command, ExitCode, Stdio};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit = |code: i32| ExitCode::from(code.clamp(0, 255) as u8);
    if copper_mcp::cloud_cli::is_cloud(&args) {
        return exit(copper_mcp::cloud_cli::run(&args, &open_in_browser));
    }
    if let Some(code) = copper_app::agents::cli::dispatch(&args) {
        return exit(code);
    }
    copper_app::main()
}

/// The system browser, for `copper cloud sso` (the caller prints the
/// address too, so a machine with no browser can still finish elsewhere).
fn open_in_browser(url: &str) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("xdg-open: {e}"))
}
