//! The few operating-system facts the shared crates need: where Copper's
//! folders are, which `COPPER_WORLD` is in force, and the user's home.
//!
//! | | Linux | Windows |
//! |---|---|---|
//! | data (store) | `$XDG_DATA_HOME/copper` | `%LOCALAPPDATA%\Copper\Data` |
//! | config (`settings.json`) | `$XDG_CONFIG_HOME/copper` | `%LOCALAPPDATA%\Copper\Data` |
//! | cache | `$XDG_CACHE_HOME/copper` | `%LOCALAPPDATA%\Copper\Cache` |
//! | engine user data | under data | `%LOCALAPPDATA%\Copper\WebView2` |
//! | logs | journal / stderr | `%LOCALAPPDATA%\Copper\Data\logs` |
//!
//! With `COPPER_WORLD=<name>` every folder above is the `copper-<name>`
//! (Linux) or `Copper-<name>` (Windows) sibling, so a test run never touches
//! the real profile. Nothing on Windows roams: `%LOCALAPPDATA%`, not
//! `%APPDATA%`, because the WebView2 profile and caches are machine-bound and
//! the store holds per-device ids.
//!
//! The Windows layout is plain path arithmetic ([`WindowsLayout::under`]) so
//! it is unit-tested on any host; only the lookup of `%LOCALAPPDATA%` itself
//! ([`WindowsLayout::from_env`]) is Windows-only.

use std::path::{Path, PathBuf};

/// `COPPER_WORLD`, cleaned to `[a-z0-9-]` (`test` if nothing survives), or
/// `None` when unset or blank.
pub fn world() -> Option<String> {
    world_from(std::env::var("COPPER_WORLD").ok().as_deref())
}

/// [`world`] on an explicit value (tests, callers with their own env).
pub fn world_from(raw: Option<&str>) -> Option<String> {
    let w = raw.filter(|w| !w.trim().is_empty())?;
    let clean: String = w
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    Some(if clean.is_empty() {
        "test".into()
    } else {
        clean
    })
}

/// The user's home folder: `$HOME` on Unix (as given, unchecked — the Linux
/// behaviour since the first release); on Windows `%USERPROFILE%`, else
/// `%HOMEDRIVE%%HOMEPATH%`. A `HOME` set by Git Bash or MSYS is ignored on
/// Windows on purpose: it is not where Windows programs keep user files.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(windows)]
    {
        let nonempty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        nonempty("USERPROFILE").map(PathBuf::from).or_else(|| {
            let drive = nonempty("HOMEDRIVE")?;
            let path = nonempty("HOMEPATH")?;
            let mut joined = drive;
            joined.push(path);
            Some(PathBuf::from(joined))
        })
    }
}

/// `~/rest` → `<home>/rest` (on Windows `~\rest` too); anything else, or no
/// known home, is returned as given.
pub fn expand_tilde(path: &str) -> PathBuf {
    let rest = path.strip_prefix("~/");
    #[cfg(windows)]
    let rest = rest.or_else(|| path.strip_prefix("~\\"));
    match (rest, home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

/// Copper's folders on Windows (CONTRACT §4): one root per world under
/// `%LOCALAPPDATA%`, with `Data\`, `Cache\`, `WebView2\` and `Data\logs\`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsLayout {
    /// `%LOCALAPPDATA%\Copper` (or `Copper-<world>`).
    pub root: PathBuf,
}

impl WindowsLayout {
    /// The layout under an explicit `%LOCALAPPDATA%`; `world` as returned by
    /// [`world`].
    pub fn under(local_app_data: impl AsRef<Path>, world: Option<&str>) -> Self {
        let name = match world {
            Some(w) => format!("Copper-{w}"),
            None => "Copper".to_string(),
        };
        WindowsLayout {
            root: local_app_data.as_ref().join(name),
        }
    }

    /// The real layout: `FOLDERID_LocalAppData` (through the `directories`
    /// crate, which calls `SHGetKnownFolderPath`), else `%LOCALAPPDATA%`,
    /// else `%USERPROFILE%\AppData\Local`, else the temp folder — never a
    /// panic, so a broken profile still starts.
    #[cfg(windows)]
    pub fn from_env() -> Self {
        let local = directories::BaseDirs::new()
            .map(|b| b.data_local_dir().to_path_buf())
            .or_else(|| {
                std::env::var_os("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
            })
            .or_else(|| home_dir().map(|h| h.join("AppData").join("Local")))
            .unwrap_or_else(std::env::temp_dir);
        WindowsLayout::under(local, world().as_deref())
    }

    /// The copper-core store: every [`crate::store::DataFile`], settings
    /// included (Windows has no separate config folder).
    pub fn data(&self) -> PathBuf {
        self.root.join("Data")
    }

    /// Disposable caches (HTTP cache, compiled content filters).
    pub fn cache(&self) -> PathBuf {
        self.root.join("Cache")
    }

    /// The WebView2 user data folder (one environment, profiles inside).
    pub fn webview2(&self) -> PathBuf {
        self.root.join("WebView2")
    }

    /// Log files.
    pub fn logs(&self) -> PathBuf {
        self.data().join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_is_cleaned() {
        assert_eq!(world_from(None), None);
        assert_eq!(world_from(Some("  ")), None);
        assert_eq!(world_from(Some("QA Run_2")).as_deref(), Some("qarun2"));
        assert_eq!(world_from(Some("a-b")).as_deref(), Some("a-b"));
        assert_eq!(world_from(Some("!!")).as_deref(), Some("test"));
    }

    #[test]
    fn windows_layout() {
        let l = WindowsLayout::under(Path::new("L"), None);
        assert_eq!(l.root, Path::new("L").join("Copper"));
        assert_eq!(l.data(), Path::new("L").join("Copper").join("Data"));
        assert_eq!(l.cache(), Path::new("L").join("Copper").join("Cache"));
        assert_eq!(l.webview2(), Path::new("L").join("Copper").join("WebView2"));
        assert_eq!(
            l.logs(),
            Path::new("L").join("Copper").join("Data").join("logs")
        );
        let w = WindowsLayout::under(Path::new("L"), Some("qa"));
        assert_eq!(w.data(), Path::new("L").join("Copper-qa").join("Data"));
    }

    #[test]
    fn tilde_needs_a_slash() {
        assert_eq!(expand_tilde("/abs/x.png"), PathBuf::from("/abs/x.png"));
        assert_eq!(expand_tilde("~x/y"), PathBuf::from("~x/y"));
        if let Some(home) = home_dir() {
            assert_eq!(expand_tilde("~/a/b.png"), home.join("a/b.png"));
        }
    }
}
