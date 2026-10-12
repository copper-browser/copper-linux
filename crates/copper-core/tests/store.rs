//! Atomic writes, `.bak` fallback on corruption, the session guard, the
//! debounced saver.

use copper_core::Saver;
use copper_core::bookmarks::Bookmark;
use copper_core::session::{SessionEntry, SessionShape};
use copper_core::store::{DataFile, LoadSource, Paths, Store, backup_path};
use std::time::Duration;

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(Paths::new(
        dir.path().join("data"),
        dir.path().join("config"),
    ));
    (dir, store)
}

fn marks(title: &str) -> Vec<Bookmark> {
    vec![Bookmark {
        id: "A".into(),
        title: title.into(),
        url: Some("https://a.test/".into()),
        ..Default::default()
    }]
}

fn shape(n: usize) -> SessionShape {
    SessionShape {
        tabs: (0..n)
            .map(|i| SessionEntry {
                url: format!("https://t{i}.test/"),
                title: format!("T{i}"),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn missing_file_is_missing() {
    let (_d, store) = store();
    let loaded = store.load::<Vec<Bookmark>>().unwrap();
    assert_eq!(loaded.source, LoadSource::Missing);
    assert!(loaded.value.is_empty());
}

#[test]
fn corrupt_file_falls_back_to_bak() {
    let (_d, store) = store();
    store.save(&marks("first")).unwrap();
    store.save(&marks("second")).unwrap();
    let path = store.path(DataFile::Bookmarks);
    // A torn write / disk trouble.
    std::fs::write(&path, b"[{\"id\":\"A\",\"tit").unwrap();
    let loaded = store.load::<Vec<Bookmark>>().unwrap();
    let LoadSource::Backup {
        quarantined: Some(aside),
    } = &loaded.source
    else {
        panic!("expected backup, got {:?}", loaded.source)
    };
    assert_eq!(loaded.value[0].title, "first");
    assert!(
        aside
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("bookmarks.unreadable-")
    );
    assert_eq!(std::fs::read(aside).unwrap(), b"[{\"id\":\"A\",\"tit");
    assert!(
        !path.exists(),
        "the bad file is set aside, not left to be overwritten"
    );
    // The next save must not replace the good backup with garbage.
    store.save(&loaded.value).unwrap();
    let again = store.load::<Vec<Bookmark>>().unwrap();
    assert_eq!(again.source, LoadSource::Primary);
    assert_eq!(again.value[0].title, "first");
}

#[test]
fn corrupt_file_never_rotates_into_bak() {
    let (_d, store) = store();
    store.save(&marks("good")).unwrap();
    store.save(&marks("good2")).unwrap();
    let path = store.path(DataFile::Bookmarks);
    std::fs::write(&path, b"not json").unwrap();
    // Saving over a corrupt file (without loading first) keeps the old .bak.
    store.save(&marks("third")).unwrap();
    let bak: Vec<Bookmark> =
        serde_json::from_slice(&std::fs::read(backup_path(&path)).unwrap()).unwrap();
    assert_eq!(bak[0].title, "good");
}

#[test]
fn both_corrupt_resets_and_says_so() {
    let (_d, store) = store();
    store.save(&marks("one")).unwrap();
    store.save(&marks("two")).unwrap();
    let path = store.path(DataFile::Bookmarks);
    std::fs::write(&path, b"{").unwrap();
    std::fs::write(backup_path(&path), b"}").unwrap();
    let loaded = store.load::<Vec<Bookmark>>().unwrap();
    let LoadSource::Reset { quarantined } = &loaded.source else {
        panic!("{:?}", loaded.source)
    };
    assert_eq!(quarantined.len(), 2);
    assert!(quarantined.iter().all(|p| p.exists()));
    assert!(loaded.value.is_empty());
}

#[test]
fn missing_primary_reads_bak() {
    let (_d, store) = store();
    store.save(&marks("one")).unwrap();
    store.save(&marks("two")).unwrap();
    std::fs::remove_file(store.path(DataFile::Bookmarks)).unwrap();
    let loaded = store.load::<Vec<Bookmark>>().unwrap();
    assert_eq!(loaded.source, LoadSource::Backup { quarantined: None });
    assert_eq!(loaded.value[0].title, "one");
}

#[test]
fn no_temporaries_left_and_secrets_are_0600() {
    let (_d, store) = store();
    let mut agent = copper_core::agent::AgentFile::default();
    assert!(agent.ensure_token());
    store.save(&agent).unwrap();
    store.save(&agent).unwrap();
    let dir = store.paths().data.clone();
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains(".tmp-")), "{names:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for p in [
            store.path(DataFile::Agent),
            backup_path(&store.path(DataFile::Agent)),
        ] {
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600,
                "{}",
                p.display()
            );
        }
    }
    let back = store.load::<copper_core::agent::AgentFile>().unwrap().value;
    assert_eq!(back.token().unwrap().len(), 64);
    assert_eq!(back.port(), 4123);
}

#[test]
fn session_guard() {
    let (_d, store) = store();
    assert!(store.save_session(&shape(10), false).unwrap());
    // Empty before restore finished: refused.
    assert!(!store.save_session(&shape(0), false).unwrap());
    assert_eq!(store.load::<SessionShape>().unwrap().value.tabs.len(), 10);
    // Dropping by more than half keeps session.previous.json.
    assert!(store.save_session(&shape(3), true).unwrap());
    let previous = store.paths().data.join("session.previous.json");
    let prev: SessionShape = serde_json::from_slice(&std::fs::read(previous).unwrap()).unwrap();
    assert_eq!(prev.tabs.len(), 10);
    // Empty after restore is a real choice.
    assert!(store.save_session(&shape(0), true).unwrap());
}

#[test]
fn saver_debounces_and_flushes() {
    let (_d, store) = store();
    let saver = Saver::with_delay(store.clone(), Duration::from_millis(300));
    for i in 0..20 {
        saver.schedule(marks(&format!("v{i}")));
    }
    saver.schedule_session(shape(2), true);
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !store.path(DataFile::Bookmarks).exists(),
        "not before the delay"
    );
    std::thread::sleep(Duration::from_millis(600));
    let got = store.load::<Vec<Bookmark>>().unwrap().value;
    assert_eq!(got[0].title, "v19", "the newest snapshot wins");
    assert_eq!(store.load::<SessionShape>().unwrap().value.tabs.len(), 2);
    // Only one write happened for twenty schedules: no backup yet.
    assert!(!backup_path(&store.path(DataFile::Bookmarks)).exists());

    saver.schedule(marks("flushed"));
    saver.flush();
    assert_eq!(
        store.load::<Vec<Bookmark>>().unwrap().value[0].title,
        "flushed"
    );
    saver.schedule(marks("on drop"));
    drop(saver);
    assert_eq!(
        store.load::<Vec<Bookmark>>().unwrap().value[0].title,
        "on drop"
    );
    assert!(store.path(DataFile::Bookmarks).exists());
    assert_eq!(
        std::fs::read_dir(&store.paths().data).unwrap().count(),
        3,
        "session.json, bookmarks.json, bookmarks.json.bak"
    );
}

#[test]
fn settings_live_in_config() {
    let (_d, store) = store();
    let mut s = copper_core::Settings::default();
    s.set_shield_paused("news.test", true);
    s.set_look(copper_core::settings::Look::Dark);
    store.save(s.values()).unwrap();
    assert!(store.paths().config.join("settings.json").exists());
    let back = copper_core::Settings::new(
        store
            .load::<serde_json::Map<String, serde_json::Value>>()
            .unwrap()
            .value,
    );
    assert_eq!(back.look(), copper_core::settings::Look::Dark);
    assert!(back.shield_paused_on("news.test"));
}
