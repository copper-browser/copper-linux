//! Every Mac fixture: load → save → load is equal, and the saved JSON has no
//! removed keys, no added keys, no type changes and no value changes.

use copper_core::cloud::{
    BookmarksDoc, CloudSyncFile, SettingsDoc, SpacesDoc, TabsDoc, encode_doc,
};
use copper_core::json::{self, JsonDifference, json_diff};
use copper_core::store::{DataFile, FileModel, LoadSource, Paths, Store};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fmt::Debug;
use std::path::PathBuf;

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/mac")
        .join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Parse, re-encode with the Mac layout, parse again; demand semantic
/// equality at both levels.
fn check<T>(name: &str, style: json::Style) -> T
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let bytes = fixture(name);
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    let typed: T = serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    let saved_bytes = json::encode(&typed, style).unwrap();
    let saved: Value = serde_json::from_slice(&saved_bytes).unwrap();
    let diff = json_diff(&original, &saved);
    let breaking: Vec<&JsonDifference> = diff.iter().filter(|d| d.is_breaking()).collect();
    assert!(
        breaking.is_empty(),
        "{name}: removed keys / type changes: {breaking:#?}"
    );
    assert!(diff.is_empty(), "{name}: differences: {diff:#?}");
    let again: T = serde_json::from_slice(&saved_bytes).unwrap();
    assert_eq!(typed, again, "{name}: load → save → load differs");
    // And a second save is byte-identical to the first (deterministic).
    assert_eq!(
        json::encode(&again, style).unwrap(),
        saved_bytes,
        "{name}: encoding not stable"
    );
    typed
}

/// The same through the real [`Store`]: copy the fixture into a temp data
/// folder at its real path, load, save, load.
fn check_store<T>(name: &str)
where
    T: FileModel + Default + PartialEq + Debug,
{
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(Paths::new(
        dir.path().join("data"),
        dir.path().join("config"),
    ));
    let path = store.path(T::FILE);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, fixture(name)).unwrap();
    let first = store.load::<T>().unwrap();
    assert_eq!(first.source, LoadSource::Primary, "{name}");
    store.save(&first.value).unwrap();
    let second = store.load::<T>().unwrap();
    assert_eq!(
        first.value, second.value,
        "{name}: store load → save → load differs"
    );
    let original: Value = serde_json::from_slice(&fixture(name)).unwrap();
    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(
        json_diff(&original, &written).is_empty(),
        "{name}: store write differs"
    );
    // The pre-save file became the backup.
    let bak: Value =
        serde_json::from_slice(&std::fs::read(copper_core::store::backup_path(&path)).unwrap())
            .unwrap();
    assert!(json_diff(&original, &bak).is_empty());
    #[cfg(unix)]
    if T::FILE.secret() {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{name} must be 0600");
    }
}

macro_rules! fixture_test {
    ($test:ident, $file:literal, $t:ty) => {
        #[test]
        fn $test() {
            check::<$t>($file, <$t as FileModel>::FILE.style());
            check_store::<$t>($file);
        }
    };
}

fixture_test!(session, "session.json", copper_core::session::SessionShape);
fixture_test!(history, "history.json", Vec<copper_core::history::Visit>);
fixture_test!(
    bookmarks,
    "bookmarks.json",
    Vec<copper_core::bookmarks::Bookmark>
);
fixture_test!(
    windows,
    "windows.json",
    Vec<copper_core::groups::SavedWindow>
);
fixture_test!(groups, "groups.json", copper_core::groups::GroupsFile);
fixture_test!(
    downloads,
    "downloads.json",
    Vec<copper_core::downloads::Keep>
);
fixture_test!(trails, "trails.json", copper_core::trails::TrailsFile);
fixture_test!(updates, "updates.json", copper_core::updates::UpdatesFile);
fixture_test!(cloud_sync, "cloud-sync.json", CloudSyncFile);
fixture_test!(chat, "chat.json", copper_core::agent::ChatConfig);
fixture_test!(
    intelligence,
    "intelligence.json",
    copper_core::intelligence::IntelligenceKeys
);
fixture_test!(agent, "agent.json", copper_core::agent::AgentFile);
fixture_test!(cloud, "cloud.json", copper_core::cloud::CloudFile);
fixture_test!(
    cloud_intelligence,
    "cloud-intelligence.json",
    copper_core::cloud::CloudProvided
);
fixture_test!(
    claude,
    "claude.json",
    copper_core::intelligence::ClaudeCredential
);
fixture_test!(mcp_servers, "mcp.json", copper_core::agent::McpServersFile);
fixture_test!(
    canvases,
    "canvases.json",
    Vec<copper_core::canvas::CanvasEntry>
);
fixture_test!(crashes, "crashes.json", copper_core::crashes::CrashIndex);

#[test]
fn standalone_cloud_docs() {
    check::<SpacesDoc>("cloud-spaces.json", json::Style::Compact);
    check::<BookmarksDoc>("cloud-bookmarks.json", json::Style::Compact);
    check::<TabsDoc>("cloud-tabs.json", json::Style::Compact);
}

/// Every base64 payload in cloud-sync.json decodes to its typed document and
/// re-encodes to the same JSON (and, sorted and compact like the Mac, to the
/// same bytes).
#[test]
fn cloud_sync_payloads() {
    let file: CloudSyncFile = serde_json::from_slice(&fixture("cloud-sync.json")).unwrap();
    assert!(!file.docs.is_empty());
    for (domain, base) in &file.docs {
        let raw = base.payload_bytes().unwrap();
        let original: Value = serde_json::from_slice(&raw).unwrap();
        let encoded = match domain.as_str() {
            "spaces" => encode_doc(&base.decode::<SpacesDoc>().unwrap()),
            "settings" => encode_doc(&base.decode::<SettingsDoc>().unwrap()),
            "bookmarks" => encode_doc(&base.decode::<BookmarksDoc>().unwrap()),
            d if d.starts_with("tabs:") => encode_doc(&base.decode::<TabsDoc>().unwrap()),
            other => panic!("unexpected domain {other}"),
        }
        .unwrap();
        let again: Value = serde_json::from_slice(&encoded).unwrap();
        assert!(
            json_diff(&original, &again).is_empty(),
            "{domain}: {:#?}",
            json_diff(&original, &again)
        );
        assert_eq!(
            String::from_utf8(encoded).unwrap(),
            String::from_utf8(json::encode_value(&original, json::Style::Compact).unwrap())
                .unwrap(),
            "{domain}: canonical bytes"
        );
    }
}

/// Unknown keys at every level survive (a newer Mac's fields).
#[test]
fn unknown_keys_survive() {
    let text = r#"{"tabs":[{"url":"https://a.test/","title":"A","future":{"x":[1,2]}}],"active":0,
        "spaces":[{"id":"S","name":"N","theme":{"colors":[{"r":1,"g":0,"b":0,"alpha":0.5}],"glow":true}}],
        "newTopLevel":"kept"}"#;
    let shape: copper_core::SessionShape = serde_json::from_str(text).unwrap();
    let out: Value =
        serde_json::from_slice(&json::encode(&shape, json::Style::Compact).unwrap()).unwrap();
    let original: Value = serde_json::from_str(text).unwrap();
    assert_eq!(json_diff(&original, &out), vec![]);
}

/// The session fixture restores into a Browser and writes back the same tabs.
#[test]
fn session_fixture_through_browser() {
    let shape: copper_core::SessionShape =
        serde_json::from_slice(&fixture("session.json")).unwrap();
    let b = copper_core::Browser::from_session(&shape);
    assert_eq!(b.pins().len(), shape.pins.as_ref().unwrap().len());
    assert_eq!(b.current_space_id(), shape.space.as_deref().unwrap());
    let back = b.to_session();
    assert_eq!(back.tab_count(), shape.tab_count());
    let loose = |s: &copper_core::SessionShape| {
        let mut v: Vec<(String, Option<String>, Option<bool>)> = s
            .tabs
            .iter()
            .filter(|e| e.pin.is_none())
            .map(|e| (e.url.clone(), e.space.clone(), e.saved.or(Some(true))))
            .collect();
        v.sort();
        v
    };
    assert_eq!(loose(&back), loose(&shape));
    let pins = |s: &copper_core::SessionShape| {
        s.pins
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| (e.url.clone(), e.pin.clone(), e.space.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(pins(&back), pins(&shape));
    let active = |s: &copper_core::SessionShape| {
        s.tabs
            .iter()
            .find(|e| e.active == Some(true))
            .map(|e| e.url.clone())
    };
    assert_eq!(active(&back), active(&shape));
    assert_eq!(back.spaces, shape.spaces);
}

/// Every DataFile has a fixture-backed type, except the ones the Mac
/// fixture set lacks.
#[test]
fn data_file_paths() {
    let paths = Paths::new("/d", "/c");
    assert_eq!(
        paths.file(DataFile::Crashes),
        PathBuf::from("/d/crashes/crashes.json")
    );
    assert_eq!(
        paths.file(DataFile::Canvases),
        PathBuf::from("/d/canvas/canvases.json")
    );
    assert_eq!(
        paths.file(DataFile::Settings),
        PathBuf::from("/c/settings.json")
    );
    assert_eq!(
        paths.profile_dir(Some("Work stuff")),
        PathBuf::from("/d/profiles/Work_stuff")
    );
}
