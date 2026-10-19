//! Where the Chromium-family browsers keep their user data, on Linux and on
//! Windows (macOS: `Import.swift` `Source.known`).
//!
//! Both layouts are plain path arithmetic over a base folder, so either can
//! be exercised against a fixture tree on any host; [`Layout::host`] picks
//! the one the running system uses.

use std::path::{Path, PathBuf};

use crate::profiles::profiles;

/// Which operating system's folder layout a base folder follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    /// `$XDG_CONFIG_HOME/google-chrome`, `…/BraveSoftware/Brave-Browser`.
    Linux,
    /// `%LOCALAPPDATA%\Google\Chrome\User Data`, `…\Microsoft\Edge\User Data`.
    Windows,
}

impl Layout {
    /// The layout of the system this was built for. Anything that isn't
    /// Windows is read as Linux (the only other system this crate serves).
    pub const fn host() -> Self {
        if cfg!(windows) {
            Layout::Windows
        } else {
            Layout::Linux
        }
    }
}

/// Which browser a [`Browser`] is, for the things that differ by product
/// (where its password export lives, what its keyring item is called).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrowserKind {
    Chrome,
    Chromium,
    Brave,
    Edge,
    Vivaldi,
    Arc,
}

/// One Chromium-family browser's user-data root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    /// "Chrome", "Chromium", "Brave", "Edge", "Vivaldi", "Arc" — what the
    /// person calls it, and what imported spaces are named after.
    pub name: String,
    pub kind: BrowserKind,
    /// The user-data root (`~/.config/google-chrome`,
    /// `%LOCALAPPDATA%\Google\Chrome\User Data`).
    pub root: PathBuf,
    /// Linux: the Secret Service `application` attribute its key is kept
    /// under. Empty on the Windows layout, where Copper never reads another
    /// browser's key.
    pub application: String,
    /// Linux: the key item's label ("Chrome Safe Storage"). Empty on the
    /// Windows layout.
    pub storage_label: String,
}

/// Linux: name, kind, folder under `~/.config`, Secret Service application
/// and key label. Edge keeps its key under Chromium's own item on Linux.
const LINUX: [(&str, BrowserKind, &str, &str, &str); 4] = [
    (
        "Chrome",
        BrowserKind::Chrome,
        "google-chrome",
        "chrome",
        "Chrome Safe Storage",
    ),
    (
        "Chromium",
        BrowserKind::Chromium,
        "chromium",
        "chromium",
        "Chromium Safe Storage",
    ),
    (
        "Brave",
        BrowserKind::Brave,
        "BraveSoftware/Brave-Browser",
        "brave",
        "Brave Safe Storage",
    ),
    (
        "Edge",
        BrowserKind::Edge,
        "microsoft-edge",
        "chromium",
        "Chromium Safe Storage",
    ),
];

/// Windows: name, kind and the path components of the user-data root under
/// `%LOCALAPPDATA%`. Arc is found separately (its folder carries a package
/// suffix).
const WINDOWS: [(&str, BrowserKind, &[&str]); 5] = [
    (
        "Chrome",
        BrowserKind::Chrome,
        &["Google", "Chrome", "User Data"],
    ),
    (
        "Edge",
        BrowserKind::Edge,
        &["Microsoft", "Edge", "User Data"],
    ),
    (
        "Brave",
        BrowserKind::Brave,
        &["BraveSoftware", "Brave-Browser", "User Data"],
    ),
    ("Vivaldi", BrowserKind::Vivaldi, &["Vivaldi", "User Data"]),
    (
        "Chromium",
        BrowserKind::Chromium,
        &["Chromium", "User Data"],
    ),
];

/// The prefix of Arc's MSIX package folder under `%LOCALAPPDATA%\Packages`
/// (`TheBrowserCompany.Arc_<publisher hash>`).
const ARC_PACKAGE_PREFIX: &str = "TheBrowserCompany.Arc_";

impl Browser {
    /// The family's known roots under `base` for this system's layout
    /// (`~/.config` on Linux, `%LOCALAPPDATA%` on Windows).
    pub fn known(base: &Path) -> Vec<Browser> {
        Self::known_in(Layout::host(), base)
    }

    /// The family's known roots under `base`, read as `layout`. Linux lists
    /// every root whether or not it exists; Windows does too, except Arc,
    /// which is listed only when its package folder is there.
    pub fn known_in(layout: Layout, base: &Path) -> Vec<Browser> {
        match layout {
            Layout::Linux => LINUX
                .into_iter()
                .map(|(name, kind, folder, app, label)| Browser {
                    name: name.into(),
                    kind,
                    root: base.join(folder),
                    application: app.into(),
                    storage_label: label.into(),
                })
                .collect(),
            Layout::Windows => {
                let mut out: Vec<Browser> = WINDOWS
                    .into_iter()
                    .map(|(name, kind, parts)| windows_browser(name, kind, base, parts))
                    .collect();
                out.extend(arc_roots(base).into_iter().take(1).map(|root| Browser {
                    root,
                    ..windows_browser("Arc", BrowserKind::Arc, base, &[])
                }));
                out
            }
        }
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

fn windows_browser(name: &str, kind: BrowserKind, base: &Path, parts: &[&str]) -> Browser {
    Browser {
        name: name.into(),
        kind,
        root: parts.iter().fold(base.to_path_buf(), |p, c| p.join(c)),
        application: String::new(),
        storage_label: String::new(),
    }
}

/// `Packages\TheBrowserCompany.Arc_*\LocalCache\Local\Arc\User Data` under
/// `base`, sorted so the choice among several installs is stable.
fn arc_roots(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base.join("Packages")) else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(ARC_PACKAGE_PREFIX))
        })
        .map(|e| {
            e.path()
                .join("LocalCache")
                .join("Local")
                .join("Arc")
                .join("User Data")
        })
        .filter(|p| p.is_dir())
        .collect();
    roots.sort();
    roots
}

/// The folder the browsers' user-data roots live under on this system:
/// `$XDG_CONFIG_HOME` (or `~/.config`) on Linux, `%LOCALAPPDATA%`
/// (`FOLDERID_LocalAppData`) on Windows.
pub fn config_dir() -> Option<PathBuf> {
    let dirs = directories::BaseDirs::new()?;
    Some(match Layout::host() {
        Layout::Linux => dirs.config_dir().to_path_buf(),
        Layout::Windows => dirs.data_local_dir().to_path_buf(),
    })
}

/// The browsers that have at least one profile under `base`.
pub fn detect(base: &Path) -> Vec<Browser> {
    detect_in(Layout::host(), base)
}

/// [`detect`] for an explicit layout (fixtures, tests).
pub fn detect_in(layout: Layout, base: &Path) -> Vec<Browser> {
    Browser::known_in(layout, base)
        .into_iter()
        .filter(|b| !b.profiles().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_roots() {
        let base = Path::new("/home/u/.config");
        let known = Browser::known_in(Layout::Linux, base);
        let names: Vec<&str> = known.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Chrome", "Chromium", "Brave", "Edge"]);
        assert_eq!(known[0].root, base.join("google-chrome"));
        assert_eq!(known[0].application, "chrome");
        assert_eq!(known[0].storage_label, "Chrome Safe Storage");
        assert_eq!(known[3].root, base.join("microsoft-edge"));
        assert_eq!(known[3].application, "chromium");
    }

    #[test]
    fn windows_roots_without_arc() {
        let base = std::env::temp_dir().join(format!("import-win-roots-{}", std::process::id()));
        let known = Browser::known_in(Layout::Windows, &base);
        let names: Vec<&str> = known.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["Chrome", "Edge", "Brave", "Vivaldi", "Chromium"]);
        assert_eq!(
            known[1].root,
            base.join("Microsoft").join("Edge").join("User Data")
        );
        assert!(known.iter().all(|b| b.application.is_empty()));
    }
}
