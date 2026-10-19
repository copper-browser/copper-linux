//! A whole Chrome user-data root read through every importer, the Linux
//! password path (v11 key in a private Secret Service) included; see
//! `chrome/linux.rs`. The Secret Service exists only on Linux, so on other
//! hosts this target prints SKIPPED and passes — the Windows layout is
//! covered by `windows_layout.rs`, which runs everywhere.
//!
//! `harness = false`: the private bus address goes in the environment before
//! any thread exists.

#[cfg(target_os = "linux")]
#[path = "chrome/linux.rs"]
mod linux;

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    if std::env::args().any(|a| a == "--list") {
        println!("chrome: test");
    } else {
        eprintln!("SKIPPED: chrome (Secret Service is Linux-only)");
    }
}
