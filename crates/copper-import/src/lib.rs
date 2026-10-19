//! Moving in from a Chromium-family browser — Chrome, Edge, Brave, Vivaldi,
//! Chromium, Arc — on Linux and Windows (PLAN T-016 backend; macOS:
//! `Import.swift`, `Fork/Flow/*`, `PasswordCSV.swift`).
//!
//! Every file is copied before it is read: the browser it belongs to is
//! usually running, and reading its live database underneath it is how you
//! get a lock error, or its attention. Nothing here ever writes to a source.
//! On Windows a file its browser won't share is [`ImportError::InUse`]:
//! close that browser and try again.
//!
//! - [`Browser`] / [`detect`] — the user-data roots under `~/.config`
//!   (Linux) or `%LOCALAPPDATA%` (Windows); [`Layout`] picks which.
//! - [`profiles`] / [`profile_infos`] — profile folders, in Chrome's order,
//!   with their names and avatars.
//! - [`bookmarks`] — `Bookmarks` / `AccountBookmarks` → [`copper_core::bookmarks::Bookmark`].
//! - [`history`] — `History` (SQLite) → places for [`copper_core::history::History::take`].
//! - [`tabs`] — `Sessions/Session_*` (SNSS) → spaces of open tabs.
//! - [`passwords`] — Linux only: `Login Data`, v10 (`peanuts`) and v11
//!   (Secret Service key) decryption.
//! - [`passwords_csv`] — every system: the CSV a browser or password
//!   manager exports, and (Windows) where each browser's export is.

pub mod bookmarks;
mod browsers;
mod copy;
pub mod history;
mod login;
#[cfg(target_os = "linux")]
pub mod passwords;
pub mod passwords_csv;
mod profiles;
pub mod tabs;

pub use browsers::{Browser, BrowserKind, Layout, config_dir, detect, detect_in};
pub(crate) use copy::{Copied, copy_read};
pub use login::ImportedLogin;
pub use profiles::{ProfileInfo, natural, profile_infos, profile_name, profiles};

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0} isn't on this computer")]
    NotInstalled(String),
    #[error("Couldn't read {0}")]
    Unreadable(String),
    #[error("{0}'s key couldn't be read from the keyring")]
    KeyRefused(String),
    /// Another program — the browser itself — holds the file without
    /// sharing it (Windows). The person closes that browser and tries again.
    #[error("{0} is in use — close the browser and try again")]
    InUse(String),
    /// A chosen file isn't a passwords export; the sentence says why and
    /// what to try next.
    #[error("{0}")]
    NotPasswords(String),
}

/// Chromium's clock (microseconds since 1601-01-01) → Unix seconds.
pub fn chromium_to_unix(micros: i64) -> Option<f64> {
    (micros > 0).then(|| micros as f64 / 1_000_000.0 - 11_644_473_600.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock() {
        assert_eq!(chromium_to_unix(0), None);
        assert_eq!(chromium_to_unix(11_644_473_600_000_000), Some(0.0));
    }
}
