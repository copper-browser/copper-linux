//! Moving in from Chrome, Chromium or Brave on Linux (PLAN T-016 backend;
//! macOS: `Import.swift`, `Fork/Flow/*`).
//!
//! Every file is copied before it is read: the browser it belongs to is
//! usually running, and reading its live database underneath it is how you
//! get a lock error, or its attention. Nothing here ever writes to a source.
//!
//! - [`Browser`] / [`detect`] — the user-data roots under `~/.config`.
//! - [`profiles`] — profile folders, in Chrome's order.
//! - [`bookmarks`] — `Bookmarks` / `AccountBookmarks` → [`copper_core::bookmarks::Bookmark`].
//! - [`history`] — `History` (SQLite) → places for [`copper_core::history::History::take`].
//! - [`tabs`] — `Sessions/Session_*` (SNSS) → spaces of open tabs.
//! - [`passwords`] — `Login Data`, v10 (`peanuts`) and v11 (Secret Service
//!   key) decryption.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

pub mod bookmarks;
pub mod history;
pub mod passwords;
pub mod tabs;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0} isn't on this computer")]
    NotInstalled(String),
    #[error("Couldn't read {0}")]
    Unreadable(String),
    #[error("{0}'s key couldn't be read from the keyring")]
    KeyRefused(String),
}

/// One Chromium-family browser on Linux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    /// "Chrome", "Chromium", "Brave".
    pub name: String,
    /// The user-data root (`~/.config/google-chrome`).
    pub root: PathBuf,
    /// The Secret Service `application` attribute its key is kept under.
    pub application: String,
    /// The key item's label ("Chrome Safe Storage").
    pub storage_label: String,
}

impl Browser {
    /// The family's known roots under `config` (`~/.config`).
    pub fn known(config: &Path) -> Vec<Browser> {
        [
            ("Chrome", "google-chrome", "chrome"),
            ("Chromium", "chromium", "chromium"),
            ("Brave", "BraveSoftware/Brave-Browser", "brave"),
        ]
        .into_iter()
        .map(|(name, folder, app)| Browser {
            name: name.into(),
            root: config.join(folder),
            application: app.into(),
            storage_label: format!("{name} Safe Storage"),
        })
        .collect()
    }

    /// This browser's profiles.
    pub fn profiles(&self) -> Vec<String> {
        profiles(&self.root)
    }

    /// Each profile's folder.
    pub fn profile_dirs(&self) -> Vec<PathBuf> {
        self.profiles().iter().map(|p| self.root.join(p)).collect()
    }
}

/// `$XDG_CONFIG_HOME` (or `~/.config`).
pub fn config_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|b| b.config_dir().to_path_buf())
}

/// The browsers that have at least one profile under `config`.
pub fn detect(config: &Path) -> Vec<Browser> {
    Browser::known(config)
        .into_iter()
        .filter(|b| !b.profiles().is_empty())
        .collect()
}

/// The profile folders (children with a Preferences file) under `root`, in
/// Chrome's order: Default first, then Profile 1, 2 … 10 compared as
/// numbers. Guest and System profiles hold nothing to move.
pub fn profiles(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "Guest Profile" && n != "System Profile")
        .filter(|n| root.join(n).join("Preferences").is_file())
        .collect();
    names.sort_by(|a, b| match (a == "Default", b == "Default") {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => natural(a, b),
    });
    names
}

/// The name a person gave a profile (`Preferences` → `profile.name`).
pub fn profile_name(profile_dir: &Path) -> Option<String> {
    let data = copy_read(&profile_dir.join("Preferences")).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&data).ok()?;
    let name = v.get("profile")?.get("name")?.as_str()?.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Numbers compared as numbers, the rest without case
/// (`localizedStandardCompare` for the names Chrome uses).
pub fn natural(a: &str, b: &str) -> Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, text)) if *d == digit => text.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (x, y) = (chunks(a), chunks(b));
    for (l, r) in x.iter().zip(&y) {
        let ord = match (l.0, r.0) {
            (true, true) => {
                let (lt, rt) = (l.1.trim_start_matches('0'), r.1.trim_start_matches('0'));
                lt.len().cmp(&rt.len()).then_with(|| lt.cmp(rt))
            }
            _ => l.1.to_lowercase().cmp(&r.1.to_lowercase()),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    x.len().cmp(&y.len())
}

/// A private copy of `file` in the temp dir, removed on drop.
pub(crate) struct Copied {
    pub path: PathBuf,
}

impl Copied {
    pub fn of(file: &Path) -> std::io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "copper-import-{}-{n}-{}",
            std::process::id(),
            file.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("file")
                .replace(' ', "_")
        ));
        std::fs::copy(file, &path)?;
        // A WAL beside a SQLite file holds its newest rows; copy it too.
        for side in ["-wal", "-journal"] {
            let mut from = file.as_os_str().to_owned();
            from.push(side);
            let from = PathBuf::from(from);
            if from.is_file() {
                let mut to = path.as_os_str().to_owned();
                to.push(side);
                let _ = std::fs::copy(&from, PathBuf::from(to));
            }
        }
        Ok(Copied { path })
    }
}

impl Drop for Copied {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        for side in ["-wal", "-journal", "-shm"] {
            let mut p = self.path.as_os_str().to_owned();
            p.push(side);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }
}

/// A file's bytes, read from a copy.
pub(crate) fn copy_read(file: &Path) -> std::io::Result<Vec<u8>> {
    let copy = Copied::of(file)?;
    std::fs::read(&copy.path)
}

/// Chromium's clock (microseconds since 1601-01-01) → Unix seconds.
pub fn chromium_to_unix(micros: i64) -> Option<f64> {
    (micros > 0).then(|| micros as f64 / 1_000_000.0 - 11_644_473_600.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["Profile 10", "Profile 2", "Profile 1", "profile 3"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["Profile 1", "Profile 2", "profile 3", "Profile 10"]);
    }

    #[test]
    fn profile_order() {
        let root = std::env::temp_dir().join(format!("import-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in [
            "Profile 10",
            "Profile 2",
            "Default",
            "Profile 1",
            "Guest Profile",
            "System Profile",
            "Crashpad",
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            if name != "Crashpad" {
                std::fs::write(root.join(name).join("Preferences"), "{}").unwrap();
            }
        }
        assert_eq!(
            profiles(&root),
            ["Default", "Profile 1", "Profile 2", "Profile 10"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clock() {
        assert_eq!(chromium_to_unix(0), None);
        assert_eq!(chromium_to_unix(11_644_473_600_000_000), Some(0.0));
    }
}
