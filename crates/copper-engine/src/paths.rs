use std::path::{Path, PathBuf};

use crate::ProfileId;

/// Where an engine keeps its on-disk state.
///
/// | What | Path |
/// |---|---|
/// | persistent profile data (cookies, storage) | `<data_root>/profiles/<id>` |
/// | persistent profile HTTP cache | `<cache_root>/profiles/<id>` |
/// | compiled content filters | `<cache_root>/content-filters` |
/// | default download folder | `downloads_dir` |
///
/// [`EnginePaths::for_platform`] picks the OS layout: the XDG folders on
/// Linux ([`EnginePaths::from_xdg`]), the CONTRACT §4 folders on Windows
/// ([`EnginePaths::windows_under`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnginePaths {
    /// `$XDG_DATA_HOME/copper` by default; `%LOCALAPPDATA%\Copper\WebView2`
    /// on Windows.
    pub data_root: PathBuf,
    /// `$XDG_CACHE_HOME/copper` by default; `%LOCALAPPDATA%\Copper\Cache` on
    /// Windows.
    pub cache_root: PathBuf,
    /// The XDG download folder (`~/Downloads`) by default; the Downloads
    /// known folder on Windows.
    pub downloads_dir: PathBuf,
}

impl EnginePaths {
    /// Paths from the XDG base directories; `None` when `$HOME` is unknown.
    pub fn from_xdg() -> Option<Self> {
        let base = directories::BaseDirs::new()?;
        let downloads_dir = directories::UserDirs::new()
            .and_then(|u| u.download_dir().map(Path::to_path_buf))
            .unwrap_or_else(|| base.home_dir().join("Downloads"));
        Some(EnginePaths {
            data_root: base.data_dir().join("copper"),
            cache_root: base.cache_dir().join("copper"),
            downloads_dir,
        })
    }

    /// The layout for this OS: [`EnginePaths::from_xdg`] on Linux (and any
    /// other non-Windows host); on Windows [`EnginePaths::windows_under`]
    /// `%LOCALAPPDATA%` (`FOLDERID_LocalAppData`) with `COPPER_WORLD`
    /// honoured, downloads in the Downloads known folder. `None` when the
    /// user's folders cannot be resolved at all.
    pub fn for_platform() -> Option<Self> {
        #[cfg(not(windows))]
        {
            Self::from_xdg()
        }
        #[cfg(windows)]
        {
            let base = directories::BaseDirs::new()?;
            let mut paths = Self::windows_under(base.data_local_dir(), world().as_deref());
            paths.downloads_dir = directories::UserDirs::new()
                .and_then(|u| u.download_dir().map(Path::to_path_buf))
                .unwrap_or_else(|| base.home_dir().join("Downloads"));
            Some(paths)
        }
    }

    /// The Windows layout (CONTRACT §4) under an explicit `%LOCALAPPDATA%`:
    /// root `Copper` (`Copper-<world>` for a `COPPER_WORLD` run, matching
    /// `copper_core::platform::WindowsLayout`), engine state in `WebView2`
    /// (the one WebView2 user data folder; profiles live inside it), caches
    /// in `Cache`. `downloads_dir` defaults to `<root>\Downloads` here;
    /// [`EnginePaths::for_platform`] replaces it with the known folder.
    pub fn windows_under(local_app_data: impl AsRef<Path>, world: Option<&str>) -> Self {
        let name = match world {
            Some(w) => format!("Copper-{w}"),
            None => "Copper".to_string(),
        };
        let root = local_app_data.as_ref().join(name);
        EnginePaths {
            data_root: root.join("WebView2"),
            cache_root: root.join("Cache"),
            downloads_dir: root.join("Downloads"),
        }
    }

    /// Everything under one directory (tests, portable runs):
    /// `<root>/data`, `<root>/cache`, `<root>/downloads`.
    pub fn under(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        EnginePaths {
            data_root: root.join("data"),
            cache_root: root.join("cache"),
            downloads_dir: root.join("downloads"),
        }
    }

    pub fn profile_data_dir(&self, id: &ProfileId) -> PathBuf {
        self.data_root.join("profiles").join(id.as_str())
    }

    pub fn profile_cache_dir(&self, id: &ProfileId) -> PathBuf {
        self.cache_root.join("profiles").join(id.as_str())
    }

    pub fn content_filter_dir(&self) -> PathBuf {
        self.cache_root.join("content-filters")
    }
}

/// `COPPER_WORLD` cleaned to `[a-z0-9-]` (`test` if nothing survives), or
/// `None` when unset or blank. Same rule as `copper_core::platform::world`
/// (this crate does not depend on copper-core), so the engine folders and
/// the store always land in the same world.
#[cfg(windows)]
fn world() -> Option<String> {
    world_from(std::env::var("COPPER_WORLD").ok().as_deref())
}

#[cfg(any(windows, test))]
fn world_from(raw: Option<&str>) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let p = EnginePaths::under("/tmp/x");
        let id = ProfileId::new("work").unwrap();
        assert_eq!(
            p.profile_data_dir(&id),
            PathBuf::from("/tmp/x/data/profiles/work")
        );
        assert_eq!(
            p.profile_cache_dir(&id),
            PathBuf::from("/tmp/x/cache/profiles/work")
        );
        assert_eq!(
            p.content_filter_dir(),
            PathBuf::from("/tmp/x/cache/content-filters")
        );
    }

    #[test]
    fn windows_layout() {
        let local = Path::new("L");
        let p = EnginePaths::windows_under(local, None);
        let root = local.join("Copper");
        assert_eq!(p.data_root, root.join("WebView2"));
        assert_eq!(p.cache_root, root.join("Cache"));
        assert_eq!(p.downloads_dir, root.join("Downloads"));
        let id = ProfileId::new("work").unwrap();
        assert_eq!(
            p.profile_cache_dir(&id),
            root.join("Cache").join("profiles").join("work")
        );
        let w = EnginePaths::windows_under(local, Some("qa"));
        assert_eq!(w.data_root, local.join("Copper-qa").join("WebView2"));
    }

    #[test]
    fn world_is_cleaned() {
        assert_eq!(world_from(None), None);
        assert_eq!(world_from(Some(" ")), None);
        assert_eq!(world_from(Some("QA Run_2")).as_deref(), Some("qarun2"));
        assert_eq!(world_from(Some("!!")).as_deref(), Some("test"));
    }

    #[cfg(not(windows))]
    #[test]
    fn for_platform_is_xdg_off_windows() {
        assert_eq!(EnginePaths::for_platform(), EnginePaths::from_xdg());
    }
}
