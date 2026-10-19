//! The Shield list through WebKitGTK's own content-extension compiler, then
//! enforced on real page loads from a local HTTP server.
//!
//! Third-party requests need a host other than the page's. Every tracker is
//! requested as `http://<tracker domain>@127.0.0.1:<port>/t/<name>.js` from a
//! page served as `http://localhost:<port>/…`: the URL string starts with the
//! tracker's domain (what `url-filter` matches), the request goes to this
//! server, and its host (127.0.0.1) differs from the page's (localhost), so
//! WebKit classes it third-party exactly as it would a real ad server.
//!
//! `harness = false`: GTK must own the main thread. Needs a display
//! (headless weston via `bin/dbx`).
//!
//! WebKitGTK exists only on Linux, so the body (`tests/webkit/linux.rs`) is
//! compiled there; on other hosts (Windows CI) this target prints SKIPPED
//! and passes.

#[cfg(target_os = "linux")]
#[path = "webkit/linux.rs"]
mod linux;

fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    if !std::env::args().any(|a| a == "--list") {
        eprintln!("SKIPPED: webkit (WebKitGTK is Linux-only)");
    }
}
