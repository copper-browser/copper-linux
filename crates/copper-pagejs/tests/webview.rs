//! Every curated script, run in a real WebKitGTK view against `tests/pages`.
//!
//! Where an MCP result is produced by page JavaScript, the expected text is the
//! macOS build's own answer for the same page and the same sequence of calls,
//! captured from a headless Mac Copper into `tests/golden/mac/` (see the crate
//! README). Run with `cargo test -p copper-pagejs -- --test-threads=1` under a
//! display (`bin/dbx`); `cargo test -p copper-pagejs --test webview -- jev`
//! runs only the cases whose names contain `jev`.
//!
//! WebKitGTK exists only on Linux, so the harness (`tests/support`) and the
//! cases (`tests/webview/linux.rs`) are compiled there; on other hosts
//! (Windows CI) this target prints SKIPPED and passes.

#[cfg(target_os = "linux")]
#[macro_use]
mod support;

#[cfg(target_os = "linux")]
#[path = "webview/linux.rs"]
mod linux;

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    if !std::env::args().any(|a| a == "--list") {
        eprintln!("SKIPPED: webview (WebKitGTK is Linux-only)");
    }
}
