//! Loading what the last run left: `session.json`, history, bookmarks,
//! settings — and where the engine keeps its own files.
//!
//! The format is the Mac's (copper-core); a `session.json` copied from a
//! Mac opens here. A file that cannot be read is never silently replaced
//! with an empty session: the store sets it aside and reads the `.bak`, and
//! if both are gone the user is told ([`Loaded::notice`]).

use copper_core::session::SessionShape;
use copper_core::settings::keys;
use copper_core::store::{LoadSource, Store};
use copper_core::{Bookmarks, Browser, History, Settings};
use copper_engine::EnginePaths;
use std::path::{Path, PathBuf};

/// Everything read at launch.
pub struct Loaded {
    pub browser: Browser,
    pub history: History,
    pub bookmarks: Bookmarks,
    pub settings: Settings,
    /// A sentence for the user when something could not be read.
    pub notice: Option<String>,
}

/// Read everything; never fails (a missing or broken file is a default,
/// with a notice when it was broken).
pub fn load(store: &Store) -> Loaded {
    let mut notice: Option<String> = None;
    let note = |notice: &mut Option<String>, what: &str, source: &LoadSource| match source {
        LoadSource::Reset { .. } => {
            notice.get_or_insert_with(|| {
                format!("Your {what} couldn’t be read; the damaged file was set aside.")
            });
        }
        LoadSource::Backup { .. } => {
            tracing::warn!("{what}: read the backup copy");
        }
        _ => {}
    };

    let session: SessionShape = match store.load::<SessionShape>() {
        Ok(l) => {
            note(&mut notice, "tabs", &l.source);
            l.value
        }
        Err(e) => {
            tracing::error!(error = %e, "session.json");
            notice.get_or_insert_with(|| "Your tabs couldn’t be read.".into());
            SessionShape::default()
        }
    };
    let browser = browser_from(&session);

    let history = match store.load::<Vec<copper_core::history::Visit>>() {
        Ok(l) => {
            note(&mut notice, "history", &l.source);
            History::from_file(l.value)
        }
        Err(e) => {
            tracing::error!(error = %e, "history.json");
            History::new()
        }
    };
    let bookmarks = match store.load::<Vec<copper_core::bookmarks::Bookmark>>() {
        Ok(l) => Bookmarks::new(l.value),
        Err(e) => {
            tracing::error!(error = %e, "bookmarks.json");
            Bookmarks::default()
        }
    };
    let settings = match store.load::<serde_json::Map<String, serde_json::Value>>() {
        Ok(l) => Settings::new(l.value),
        Err(e) => {
            tracing::error!(error = %e, "settings.json");
            Settings::default()
        }
    };
    Loaded {
        browser,
        history,
        bookmarks,
        settings,
        notice,
    }
}

/// The browser a session file describes: everything asleep but the active
/// tab, which wakes when it is shown.
pub fn browser_from(session: &SessionShape) -> Browser {
    let mut b = Browser::from_session(session);
    // Nothing the window draws should treat the restore as a change.
    b.take_events();
    b
}

/// Where the engine keeps cookies, caches and downloads, next to Copper's
/// own files (the same `copper-<world>` names under the XDG cache folder).
pub fn engine_paths(store: &Store, settings: &Settings) -> EnginePaths {
    let data_root = store.paths().data.clone();
    let cache_root = cache_dir(store);
    let downloads_dir = settings
        .get(keys::DOWNLOADS)
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| EnginePaths::from_xdg().map(|p| p.downloads_dir))
        .unwrap_or_else(|| home().join("Downloads"));
    EnginePaths {
        data_root,
        cache_root,
        downloads_dir,
    }
}

/// `$XDG_CACHE_HOME/<copper or copper-world>`.
pub fn cache_dir(store: &Store) -> PathBuf {
    let name = store
        .paths()
        .data
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "copper".into());
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".cache"));
    base.join(name)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("/").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/mac")
            .join(name)
    }

    #[test]
    fn a_mac_session_opens() {
        let bytes = std::fs::read(fixture("session.json")).expect("fixture");
        let shape: SessionShape = serde_json::from_slice(&bytes).expect("parses");
        let b = browser_from(&shape);
        assert!(b.spaces().count() >= 1);
        let tabs = b.all_tabs().count();
        assert!(tabs >= 1, "the fixture has tabs");
        let active = b.active_id();
        let awake: Vec<_> = b
            .all_tabs()
            .filter(|t| !t.sleeping)
            .map(|t| t.id.clone())
            .collect();
        assert!(
            awake.len() <= 1,
            "everything sleeps but the active tab: {awake:?}"
        );
        if let Some(a) = awake.first() {
            assert_eq!(Some(a), active.as_ref());
        }
        // And it writes back in the same shape.
        let again = b.to_session();
        let json = serde_json::to_value(&again).unwrap();
        assert!(json.get("spaces").is_some() || json.get("tabs").is_some());
    }

    #[test]
    fn loads_a_world_and_survives_a_broken_session() {
        let dir = std::env::temp_dir().join(format!("copper-app-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(copper_core::store::Paths::new(
            dir.join("data"),
            dir.join("config"),
        ));
        let first = load(&store);
        assert_eq!(first.browser.all_tabs().count(), 0);
        assert!(first.notice.is_none());
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/session.json"), b"{ not json").unwrap();
        let broken = load(&store);
        assert!(broken.notice.is_some(), "a broken session is never silent");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_follows_the_world_name() {
        let store = Store::new(copper_core::store::Paths::new(
            "/x/copper-test",
            "/y/copper-test",
        ));
        assert_eq!(cache_dir(&store).file_name().unwrap(), "copper-test");
    }
}
