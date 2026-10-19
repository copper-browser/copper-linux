//! A whole Chrome-on-Linux user-data root, built here (bookmarks from
//! copper-ref's docs/fixtures/flow-chrome, a History DB, SNSS session files,
//! a Login Data DB with v10 and v11 rows), read through every importer, with
//! the v11 key in a private Secret Service. Every source file is checksummed
//! before and after: an import never writes to the other browser.
//!
//! `harness = false`: the private bus address goes in the environment before
//! any thread exists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use copper_core::history::History;
use copper_creds::sandbox::PrivateSecretService;
use copper_import::passwords::{self, Keys, derive, wrap};
use copper_import::{Browser, bookmarks, history, profiles, tabs};
use rusqlite::Connection;

type Res = Result<String, String>;

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

const V11_PASSWORD: &str = "c2VjcmV0LWNocm9tZS1rZXk=";

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flow-chrome")
}

fn checksums(root: &Path) -> BTreeMap<String, String> {
    use sha1::Digest;
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap();
                let meta = std::fs::metadata(&p).unwrap();
                let digest = sha1::Sha1::digest(&bytes);
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                out.insert(
                    p.strip_prefix(root).unwrap().display().to_string(),
                    format!("{hex} {:?}", meta.modified().ok()),
                );
            }
        }
    }
    out
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

// ------------------------------------------------------------ SNSS writer

fn u32b(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn pad(mut b: Vec<u8>) -> Vec<u8> {
    while !b.len().is_multiple_of(4) {
        b.push(0);
    }
    b
}

fn field(s: &str) -> Vec<u8> {
    let mut v = u32b(s.len() as u32);
    v.extend(pad(s.as_bytes().to_vec()));
    v
}

fn field16(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut v = u32b(units.len() as u32);
    v.extend(pad(units.iter().flat_map(|u| u.to_le_bytes()).collect()));
    v
}

fn pickle(body: Vec<u8>) -> Vec<u8> {
    let mut v = u32b(body.len() as u32);
    v.extend(body);
    v
}

fn command(id: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut v = ((payload.len() + 1) as u16).to_le_bytes().to_vec();
    v.push(id);
    v.extend(payload);
    v
}

fn nav(tab: u32, index: u32, url: &str, title: &str) -> Vec<u8> {
    let mut body = u32b(tab);
    body.extend(u32b(index));
    body.extend(field(url));
    body.extend(field16(title));
    body.extend(u32b(0)); // page state (empty)
    body.extend(u32b(0)); // transition
    pickle(body)
}

fn pair(a: u32, b: u32) -> Vec<u8> {
    let mut v = u32b(a);
    v.extend(u32b(b));
    v
}

fn session_file() -> Vec<u8> {
    let mut out = b"SNSS".to_vec();
    out.extend(u32b(1));
    // Window 1: tabs 7 (pinned), 8 (grouped, two navigations, back one),
    // 9 (chrome://, filtered), 10 (grouped), 11 (closed later).
    let w1 = [
        (7, 0, vec![("https://mail.example/", "Mail")]),
        (
            8,
            1,
            vec![
                ("https://news.example/a", "A"),
                ("https://news.example/b", "B"),
            ],
        ),
        (9, 2, vec![("chrome://settings/", "Settings")]),
        (10, 3, vec![("https://docs.example/", "Docs")]),
        (11, 4, vec![("https://gone.example/", "Gone")]),
    ];
    for (tab, index, navs) in &w1 {
        out.extend(command(0, pair(1, *tab)));
        out.extend(command(2, pair(*tab, *index)));
        for (i, (url, title)) in navs.iter().enumerate() {
            out.extend(command(6, nav(*tab, i as u32, url, title)));
        }
    }
    out.extend(command(7, pair(8, 0))); // tab 8 went back to A
    let mut pinned = u32b(7);
    pinned.extend([1, 0, 0, 0]);
    out.extend(command(12, pinned));
    // Group key (high, low) for tabs 8 and 10, titled "Reading", color 3.
    for tab in [8u32, 10] {
        let mut g = u32b(tab);
        g.extend([0, 0, 0, 0]);
        g.extend(42u64.to_le_bytes());
        g.extend(7u64.to_le_bytes());
        g.extend([1, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(command(25, g));
    }
    let mut meta = 42u64.to_le_bytes().to_vec();
    meta.extend(7u64.to_le_bytes());
    meta.extend(field16("Reading"));
    meta.extend(u32b(3));
    out.extend(command(27, pickle(meta)));
    // Tab 10 last active at 2026-01-01T00:00:00Z.
    let micros: i64 = (1_767_225_600 + 11_644_473_600) * 1_000_000;
    let mut active = u32b(10);
    active.extend([0, 0, 0, 0]);
    active.extend(micros.to_le_bytes());
    out.extend(command(21, active));
    // Tab 11 closed.
    let mut closed = u32b(11);
    closed.extend([0; 12]);
    out.extend(command(16, closed));
    out.extend(command(8, pair(1, 3))); // tab at index 3 selected
    // Window 2: one tab. Window 3: an app window (type 1), left out.
    out.extend(command(0, pair(2, 20)));
    out.extend(command(2, pair(20, 0)));
    out.extend(command(6, nav(20, 0, "https://second.example/", "Second")));
    out.extend(command(9, pair(3, 1)));
    out.extend(command(0, pair(3, 30)));
    out.extend(command(6, nav(30, 0, "https://app.example/", "App")));
    out
}

fn closed_file() -> Vec<u8> {
    let mut out = b"SNSS".to_vec();
    out.extend(u32b(1));
    // The same small ids as the open window: must not leak in.
    out.extend(command(1, nav(7, 0, "https://closed.example/x", "Closed")));
    out.extend(command(1, nav(8, 5, "https://closed.example/y", "Closed")));
    out
}

// ------------------------------------------------------------ the root

fn build_root(root: &Path) {
    copy_tree(&fixture(), root);
    std::fs::write(
        root.join("Default/Preferences"),
        r#"{"profile":{"name":"Person 1"}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("Profile 1/Preferences"),
        r#"{"profile":{"name":"Work"}}"#,
    )
    .unwrap();
    std::fs::write(root.join("Profile 2/Preferences"), "{}").unwrap();

    // History in Default and Profile 1.
    let micros = |unix: i64| (unix + 11_644_473_600) * 1_000_000;
    for (profile, rows) in [
        (
            "Default",
            vec![
                (
                    "https://alpha.example/one",
                    "One",
                    3,
                    micros(1_700_000_300),
                    0,
                ),
                (
                    "https://hidden.example/",
                    "Hidden",
                    5,
                    micros(1_700_000_400),
                    1,
                ),
                (
                    "https://never.example/",
                    "Never",
                    0,
                    micros(1_700_000_500),
                    0,
                ),
            ],
        ),
        (
            "Profile 1",
            vec![("https://beta.example/", "Beta", 1, micros(1_700_000_600), 0)],
        ),
    ] {
        let db = Connection::open(root.join(profile).join("History")).unwrap();
        db.execute_batch(
            "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, \
             typed_count INTEGER, last_visit_time INTEGER, hidden INTEGER)",
        )
        .unwrap();
        for (url, title, count, last, hidden) in rows {
            db.execute(
                "INSERT INTO urls (url, title, visit_count, typed_count, last_visit_time, hidden) \
                 VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                rusqlite::params![url, title, count, last, hidden],
            )
            .unwrap();
        }
    }

    // Sessions.
    let sessions = root.join("Default/Sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("Session_13400000000000001"), session_file()).unwrap();
    std::fs::write(sessions.join("Tabs_13400000000000001"), closed_file()).unwrap();
    let work = root.join("Profile 1/Sessions");
    std::fs::create_dir_all(&work).unwrap();
    let mut one = b"SNSS".to_vec();
    one.extend(u32b(1));
    one.extend(command(0, pair(5, 1)));
    one.extend(command(6, nav(1, 0, "https://work.example/", "Work")));
    std::fs::write(work.join("Session_13400000000000009"), one).unwrap();

    // Login Data: v10, v11, never, a duplicate, and one under another key.
    let k10 = derive(b"peanuts");
    let k11 = derive(V11_PASSWORD.as_bytes());
    let db = Connection::open(root.join("Default/Login Data")).unwrap();
    db.execute_batch(
        "CREATE TABLE logins (origin_url VARCHAR NOT NULL, action_url VARCHAR, \
         username_element VARCHAR, username_value VARCHAR, password_element VARCHAR, \
         password_value BLOB, signon_realm VARCHAR NOT NULL, date_created INTEGER NOT NULL, \
         blacklisted_by_user INTEGER NOT NULL, scheme INTEGER NOT NULL, date_last_used INTEGER NOT NULL DEFAULT 0)",
    )
    .unwrap();
    let rows: Vec<(&str, &str, Vec<u8>, i64, i64)> = vec![
        (
            "https://www.example.com/login",
            "ann",
            wrap(b"ann-v10-pw", &k10, "v10"),
            0,
            micros(1_700_000_000),
        ),
        (
            "https://accounts.example.org/",
            "bob",
            wrap(b"bob-v11-pw", &k11, "v11"),
            0,
            0,
        ),
        ("https://never.example/", "", Vec::new(), 1, 0),
        (
            "https://www.example.com/other",
            "ann",
            wrap(b"dup", &k10, "v10"),
            0,
            0,
        ),
        (
            "https://lost.example/",
            "zed",
            wrap(b"zz", &derive(b"other"), "v11"),
            0,
            0,
        ),
    ];
    for (origin, user, blob, never, used) in rows {
        db.execute(
            "INSERT INTO logins (origin_url, username_value, password_value, signon_realm, \
             date_created, blacklisted_by_user, scheme, date_last_used) VALUES (?1, ?2, ?3, ?1, 0, ?4, 0, ?5)",
            rusqlite::params![origin, user, blob, never, used],
        )
        .unwrap();
    }
}

// ------------------------------------------------------------ checks

fn check_profiles(root: &Path) -> Res {
    let found = profiles(root);
    ensure(found == ["Default", "Profile 1", "Profile 2"], || {
        format!("{found:?}")
    })?;
    let detected = copper_import::detect(root.parent().unwrap());
    // `root` is named google-chrome under a fake ~/.config.
    ensure(detected.len() == 1 && detected[0].name == "Chrome", || {
        format!("{detected:?}")
    })?;
    Ok(format!("{found:?}"))
}

fn check_bookmarks(root: &Path) -> Res {
    let dirs: Vec<PathBuf> = profiles(root).iter().map(|p| root.join(p)).collect();
    let read = bookmarks::read(&dirs);
    ensure(read.complete(), || {
        format!("failed {:?}", read.failed_files)
    })?;
    ensure(read.count() == 12, || format!("count {}", read.count()))?;
    let titles: Vec<&str> = read.nodes.iter().map(|n| n.title.as_str()).collect();
    ensure(
        titles
            == [
                "Alpha", "Projects", "Shared", "Local", "Beta", "Other", "Mobile",
            ],
        || format!("top {titles:?}"),
    )?;
    let projects = &read.nodes[1];
    let kids: Vec<&str> = projects.kids().iter().map(|n| n.title.as_str()).collect();
    ensure(kids == ["Work", "Extra"], || format!("projects {kids:?}"))?;
    let deep = &projects.kids()[0].kids()[0].kids()[0];
    ensure(deep.url.as_deref() == Some("https://deep.example/"), || {
        "deep".into()
    })?;
    let other: Vec<&str> = read.nodes[5]
        .kids()
        .iter()
        .map(|n| n.title.as_str())
        .collect();
    ensure(other == ["Reading", "Offline", "Second other"], || {
        format!("other {other:?}")
    })?;
    let mobile: Vec<&str> = read.nodes[6]
        .kids()
        .iter()
        .map(|n| n.title.as_str())
        .collect();
    ensure(mobile == ["Phone", "Pocket", "Second mobile"], || {
        format!("mobile {mobile:?}")
    })?;
    let json = serde_json::to_string(&read.nodes).unwrap();
    ensure(
        !json.contains("chrome://")
            && !json.contains("javascript:")
            && json.matches("https://shared.example/").count() == 1,
        || "filter/dedupe".into(),
    )?;
    // A corrupt file is reported, not taken as empty.
    let broken = root.parent().unwrap().join("broken/P");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("Bookmarks"), "{not json").unwrap();
    let r = bookmarks::read(std::slice::from_ref(&broken));
    ensure(!r.complete() && r.nodes.is_empty(), || "corrupt".into())?;
    Ok("12 sites; account→local→next profile; Other/Mobile at the foot".into())
}

fn check_history(root: &Path) -> Res {
    let dirs: Vec<PathBuf> = profiles(root).iter().map(|p| root.join(p)).collect();
    let places = history::read(&dirs, 100);
    let urls: Vec<&str> = places.iter().map(|p| p.url.as_str()).collect();
    ensure(
        urls == ["https://beta.example/", "https://alpha.example/one"],
        || format!("{urls:?}"),
    )?;
    ensure(
        (places[1].last.unix() - 1_700_000_300.0).abs() < 1e-3,
        || format!("{:?}", places[1].last),
    )?;
    let mut h = History::new();
    for p in &places {
        p.take_into(&mut h);
    }
    ensure(h.len() == 2, || format!("history {}", h.len()))?;
    let one = h
        .visits()
        .find(|v| v.url == "https://alpha.example/one")
        .unwrap();
    ensure(one.count == 3 && one.title == "One", || format!("{one:?}"))?;
    ensure(history::read(&dirs, 1).len() == 1, || "limit".into())?;
    Ok("2 places, newest first, hidden/unvisited left out, into History".into())
}

fn check_tabs(root: &Path) -> Res {
    let spaces = tabs::read(root, "Chrome").map_err(|e| e.to_string())?;
    let names: Vec<&str> = spaces.iter().map(|s| s.space.name.as_str()).collect();
    ensure(names == ["Chrome", "Chrome 2", "Chrome · Work"], || {
        format!("{names:?}")
    })?;
    let first = &spaces[0];
    let urls: Vec<&str> = first.tabs.iter().map(|t| t.url.as_str()).collect();
    ensure(
        urls == [
            "https://mail.example/",
            "https://news.example/a",
            "https://docs.example/",
        ],
        || format!("window 1 {urls:?}"),
    )?;
    ensure(first.tabs[0].pinned && !first.tabs[1].pinned, || {
        "pin".into()
    })?;
    ensure(first.tabs[2].active && !first.tabs[0].active, || {
        "active".into()
    })?;
    ensure(first.tabs[2].seen == Some(1_767_225_600.0), || {
        format!("seen {:?}", first.tabs[2].seen)
    })?;
    ensure(
        first.groups.len() == 1 && first.groups[0].name == "Reading",
        || format!("{:?}", first.groups),
    )?;
    ensure(first.groups[0].hue == Some(3.0 / 8.0), || "hue".into())?;
    let gid = &first.groups[0].id;
    ensure(
        first.tabs[1].group.as_ref() == Some(gid)
            && first.tabs[2].group.as_ref() == Some(gid)
            && first.tabs[0].group.is_none(),
        || "group ids".into(),
    )?;
    ensure(first.space.profile.is_none(), || {
        "Default wears the shared jar".into()
    })?;
    ensure(
        spaces[2].space.profile.as_deref() == Some("Profile 1"),
        || "work jar".into(),
    )?;
    let all = serde_json::to_string(
        &spaces
            .iter()
            .flat_map(|s| s.tabs.iter().map(|t| t.url.clone()))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    ensure(
        !all.contains("closed.example")
            && !all.contains("gone.example")
            && !all.contains("app.example"),
        || all.clone(),
    )?;
    // Only a closed list: read, nothing open.
    let only = root.parent().unwrap().join("closed-only/Default/Sessions");
    std::fs::create_dir_all(&only).unwrap();
    std::fs::write(only.parent().unwrap().join("Preferences"), "{}").unwrap();
    std::fs::write(only.join("Tabs_1"), closed_file()).unwrap();
    let none = tabs::read(&root.parent().unwrap().join("closed-only"), "Chrome")
        .map_err(|e| e.to_string())?;
    ensure(none.is_empty(), || "closed-only".into())?;
    // Unreadable session files say so.
    std::fs::write(only.join("Session_2"), "not a session").unwrap();
    std::fs::remove_file(only.join("Tabs_1")).unwrap();
    ensure(
        tabs::read(&root.parent().unwrap().join("closed-only"), "Chrome").is_err(),
        || "garbage".into(),
    )?;
    Ok("3 spaces; pins, groups, active, last-active; closed/app/chrome:// left out".into())
}

async fn check_passwords(root: &Path) -> Res {
    let chrome = Browser::known(root.parent().unwrap())
        .into_iter()
        .find(|b| b.name == "Chrome")
        .unwrap();
    ensure(chrome.root == root, || format!("{:?}", chrome.root))?;
    let dirs = chrome.profile_dirs();
    ensure(passwords::count(&dirs) == 4, || {
        format!("count {}", passwords::count(&dirs))
    })?;

    // Without the keyring item: v10 only.
    let basic = Keys::for_browser(&chrome)
        .await
        .map_err(|e| e.to_string())?;
    let found = passwords::read(&dirs, &basic).map_err(|e| e.to_string())?;
    ensure(found.logins.len() == 1 && found.undecryptable == 2, || {
        format!("basic {found:?}")
    })?;

    // Chrome's own "Chrome Safe Storage" item, as Chrome writes it.
    copper_creds::store::store_raw(
        "Chrome Safe Storage",
        &[
            ("xdg:schema", "chrome_libsecret_os_crypt_password_v2"),
            ("application", "chrome"),
        ],
        V11_PASSWORD.as_bytes(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let keys = Keys::for_browser(&chrome)
        .await
        .map_err(|e| e.to_string())?;
    ensure(keys.v11.is_some(), || "no v11 key".into())?;
    let found = passwords::read(&dirs, &keys).map_err(|e| e.to_string())?;
    let got: Vec<(String, String, String)> = found
        .logins
        .iter()
        .map(|l| (l.host.clone(), l.user.clone(), l.password.clone()))
        .collect();
    ensure(
        got == [
            ("example.com".into(), "ann".into(), "ann-v10-pw".into()),
            (
                "accounts.example.org".into(),
                "bob".into(),
                "bob-v11-pw".into(),
            ),
        ],
        || format!("logins {:?}", found.logins),
    )?;
    ensure(found.never == ["never.example"], || {
        format!("never {:?}", found.never)
    })?;
    ensure(found.undecryptable == 1, || {
        format!("undecryptable {}", found.undecryptable)
    })?;
    ensure(
        (found.logins[0].used.unwrap() - 1_700_000_000.0).abs() < 1e-3,
        || "used".into(),
    )?;
    // Chromium has no item: v11 rows stay locked.
    let chromium = Browser::known(root.parent().unwrap())
        .into_iter()
        .find(|b| b.name == "Chromium")
        .unwrap();
    ensure(
        Keys::for_browser(&chromium)
            .await
            .map_err(|e| e.to_string())?
            .v11
            .is_none(),
        || "chromium key".into(),
    )?;
    Ok("v10 + v11 (Secret Service 'Chrome Safe Storage') decrypted; never-list; dedupe".into())
}

pub fn main() {
    if std::env::args().any(|a| a == "--list") {
        println!("chrome: test");
        return;
    }
    let svc = PrivateSecretService::start().expect("private Secret Service");
    // SAFETY: no other thread exists yet.
    unsafe { svc.export() };
    let base = std::env::temp_dir().join(format!("copper-import-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("config/google-chrome");
    build_root(&root);
    let fixture_before = checksums(&fixture());
    let before = checksums(&root);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut fails = 0;
    let results = vec![
        ("profiles", check_profiles(&root)),
        ("bookmarks", check_bookmarks(&root)),
        ("history", check_history(&root)),
        ("tabs", check_tabs(&root)),
        ("passwords", rt.block_on(check_passwords(&root))),
        ("untouched", {
            let after = checksums(&root);
            let fixture_after = checksums(&fixture());
            if after == before && fixture_after == fixture_before {
                Ok(format!(
                    "{} source files byte- and mtime-identical",
                    before.len()
                ))
            } else {
                Err(format!(
                    "changed: {:?}",
                    after
                        .iter()
                        .filter(|(k, v)| before.get(*k) != Some(v))
                        .collect::<Vec<_>>()
                ))
            }
        }),
    ];
    for (name, r) in results {
        match r {
            Ok(d) => println!("PASS {name}: {d}"),
            Err(e) => {
                fails += 1;
                println!("FAIL {name}: {e}");
            }
        }
    }
    drop(rt);
    drop(svc);
    let _ = std::fs::remove_dir_all(&base);
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
