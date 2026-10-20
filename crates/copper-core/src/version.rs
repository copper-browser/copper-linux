//! Which Copper is running: the app binary's own version.
//!
//! The shared crates carry Copper for Linux's version (this workspace's
//! `version`), but they also run inside Copper for Windows, which has its own
//! number. Everything the shared code says about "this build" reads
//! [`app`]: the update check ("You have …" and the comparison with the
//! feed), every `User-Agent` ([`user_agent`]), MCP `serverInfo`, crash
//! entries and the Jev/router requests. The binary sets it once, first thing
//! in `main`, with its own `env!("CARGO_PKG_VERSION")` ([`set_app`]); unset
//! (unit tests, tools) it is this workspace's version.

use std::sync::OnceLock;

static APP: OnceLock<&'static str> = OnceLock::new();

/// Record the running app's version. The first call wins (a version cannot
/// change while the process runs); `false` when another version was already
/// set.
pub fn set_app(version: &'static str) -> bool {
    *APP.get_or_init(|| version) == version
}

/// The running app's version ([`set_app`]), else the shared crates' own.
pub fn app() -> &'static str {
    APP.get().copied().unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The system in a `User-Agent`: `Linux`, `Windows` or `macOS`.
pub fn system() -> &'static str {
    if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// `Copper/<app version> (<system>)`, the `User-Agent` of Copper's own
/// requests (Copper Cloud, the update check).
pub fn user_agent() -> String {
    format!("Copper/{} ({})", app(), system())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_sets_its_version_once() {
        // One process-wide value: this is the only test that sets it.
        assert_eq!(app(), env!("CARGO_PKG_VERSION"));
        assert!(set_app("9.8.7"));
        assert_eq!(app(), "9.8.7");
        assert!(set_app("9.8.7"), "the same version again is fine");
        assert!(!set_app("1.0.0"), "a different one is refused");
        assert_eq!(app(), "9.8.7");
        assert_eq!(user_agent(), format!("Copper/9.8.7 ({})", system()));
    }
}
