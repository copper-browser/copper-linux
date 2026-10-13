//! Copper for Linux: the GTK4 / libadwaita browser shell (binary `copper`).
//!
//! `copper --mcp-stdio` (the stdio bridge) and `copper <verb>` /
//! `copper --cli <verb>` (the agent command line) are answered here without
//! a display; everything else is the browser (`copper_app::main`, which
//! also prints `copper --version` before GTK starts).

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = copper_app::agents::cli::dispatch(&args) {
        return std::process::ExitCode::from(code.clamp(0, 255) as u8);
    }
    copper_app::main()
}
