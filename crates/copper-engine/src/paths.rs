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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnginePaths {
    /// `$XDG_DATA_HOME/copper` by default.
    pub data_root: PathBuf,
    /// `$XDG_CACHE_HOME/copper` by default.
    pub cache_root: PathBuf,
    /// The XDG download folder (`~/Downloads`) by default.
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
}
