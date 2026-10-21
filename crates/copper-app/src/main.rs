//! Copper for Linux: the GTK4 / libadwaita browser shell (binary `copper`).
//!
//! `copper --mcp-stdio` (the stdio bridge), `copper <verb>` /
//! `copper --cli <verb>` (the agent command line), `copper cloud …` and
//! `copper botpass …` (pairing with no window) are answered here without a
//! display; everything else is the browser (`copper_app::main`, which also prints
//! `copper --version` before GTK starts).

use std::process::ExitCode;

fn main() -> ExitCode {
    // Everything shared reads "this build" from here (copper_core::version).
    copper_core::version::set_app(env!("CARGO_PKG_VERSION"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit = |code: i32| ExitCode::from(code.clamp(0, 255) as u8);
    if copper_botpass::cli::is_botpass(&args) {
        return exit(copper_botpass::cli::run(&args));
    }
    if copper_mcp::cloud_cli::is_cloud(&args) {
        return exit(copper_mcp::cloud_cli::run(
            &args,
            &copper_app::cloud::open_in_browser,
        ));
    }
    if let Some(code) = copper_app::agents::cli::dispatch(&args) {
        return exit(code);
    }
    copper_app::main()
}
