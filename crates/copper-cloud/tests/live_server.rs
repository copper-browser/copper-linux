//! End to end against a REAL copper-cloud server on this machine, with native
//! PostgreSQL (never Docker).
//!
//! The test starts the server itself, on a free localhost port with a fresh
//! self-signed certificate, from:
//!
//! * `COPPER_CLOUD_BIN` — the server binary (default
//!   `~/src/copper-cloud-ref/target/debug/copper-cloud`, built from
//!   copper-browser/copper-cloud with `cargo build -p copper-cloud`);
//! * `COPPER_CLOUD_TEST_DATABASE_URL` — a dedicated database (default
//!   `postgres://copper_cloud_linux:copper_cloud_linux_test_pw@127.0.0.1:5433/copper_cloud_linux_test`).
//!   Its `public` schema is reset at the start of each run (with `psql`).
//!
//! Without the binary the test says so and passes, unless
//! `COPPER_CLOUD_REQUIRE_LIVE=1` (then it fails).

use copper_cloud::canvas::doc::{CanvasDoc, LiveCanvas, Me};
use copper_cloud::canvas::{Author, CanvasApi};
use copper_cloud::client::CloudClient;
use copper_cloud::link::{Code, normalize_fingerprint, parse_code, parse_link_code};
use copper_cloud::sync::{
    Domain, LiveKept, LocalVisit, MemoryBrowser, MemoryState, Selector, SyncEngine,
};
use copper_cloud::{CloudIntelligence, Failure};
use copper_core::bookmarks::Bookmark;
use copper_core::cloud::{OpenTab, SpacesDoc, decode_base64};
use copper_core::session::Space;
use copper_core::{DataFile, Paths, Store};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};

const DEFAULT_DB: &str = "postgres://copper_cloud_linux:copper_cloud_linux_test_pw@127.0.0.1:5433/copper_cloud_linux_test";
// Test-only keys (the server checks their shape, nothing else).
const INSTANCE_KEY: &str = "linuxTestInstanceKey0123456789-_abcdefghijk";
const MASTER_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";

struct Server {
    _child: Child,
    _dir: tempfile::TempDir,
    config: PathBuf,
    bin: PathBuf,
    link_code: String,
    log: PathBuf,
}

impl Server {
    async fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(&self.bin)
            .args(args)
            .env("COPPER_CLOUD_CONFIG", &self.config)
            .env("COPPER_CLOUD_METRICS_BIND", "off")
            .output()
            .await
            .expect("run copper-cloud")
    }

    fn log_tail(&self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    }
}

fn server_bin() -> Option<PathBuf> {
    let path = std::env::var_os("COPPER_CLOUD_BIN")
        .map(PathBuf::from)
        .or_else(|| {
            copper_core::platform::home_dir().map(|h| {
                h.join(format!(
                    "src/copper-cloud-ref/target/debug/copper-cloud{}",
                    std::env::consts::EXE_SUFFIX
                ))
            })
        })?;
    path.is_file().then_some(path)
}

/// A secret file is 0600 on Unix. Windows has no mode bits: the store relies
/// on the per-user profile ACL there, so only existence is checked.
fn assert_private(path: &Path) {
    assert!(path.is_file(), "{} missing", path.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600,
            "{}",
            path.display()
        );
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn start_server() -> Option<Server> {
    let Some(bin) = server_bin() else {
        let required = std::env::var("COPPER_CLOUD_REQUIRE_LIVE").is_ok_and(|v| v == "1");
        assert!(
            !required,
            "COPPER_CLOUD_REQUIRE_LIVE=1 but no copper-cloud binary (set COPPER_CLOUD_BIN)"
        );
        eprintln!(
            "SKIPPED: no copper-cloud server binary (set COPPER_CLOUD_BIN) — live test not run"
        );
        return None;
    };
    let db = std::env::var("COPPER_CLOUD_TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DB.into());
    // A clean database each run (native PostgreSQL; the role owns the schema).
    let reset = std::process::Command::new("psql")
        .args([
            &db,
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "-c",
            "DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;",
        ])
        .output();
    match reset {
        Ok(o) if o.status.success() => {}
        Ok(o) => panic!(
            "resetting the test database failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => eprintln!("psql not available ({e}); using the database as it is"),
    }
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let run = |args: Vec<String>| {
        let bin = bin.clone();
        let config = config.clone();
        async move {
            let out = Command::new(&bin)
                .args(&args)
                .env("COPPER_CLOUD_CONFIG", &config)
                .env("COPPER_CLOUD_METRICS_BIND", "off")
                .output()
                .await
                .expect("run copper-cloud");
            assert!(
                out.status.success(),
                "copper-cloud {args:?}: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
    };
    run(vec![
        "init-config".into(),
        "--write".into(),
        config.display().to_string(),
        "--public-url".into(),
        addr.clone(),
        "--listen".into(),
        addr.clone(),
        "--database-url".into(),
        db.clone(),
        "--tls-mode".into(),
        "self-signed".into(),
        "--tls-dir".into(),
        dir.path().join("tls").display().to_string(),
        "--instance-key".into(),
        INSTANCE_KEY.into(),
        "--master-key".into(),
        MASTER_KEY.into(),
    ])
    .await;
    run(vec!["tls-init".into()]).await;
    let link_code = run(vec!["link-code".into()]).await;
    let log = dir.path().join("server.log");
    let file = std::fs::File::create(&log).unwrap();
    let child = Command::new(&bin)
        .arg("serve")
        .env("COPPER_CLOUD_CONFIG", &config)
        .env("COPPER_CLOUD_METRICS_BIND", "off")
        .env("COPPER_CLOUD_LIMITS__AUTH_PER_MINUTE", "100000")
        .env("COPPER_CLOUD_LOG_LEVEL", "info")
        .stdout(Stdio::from(file.try_clone().unwrap()))
        .stderr(Stdio::from(file))
        .kill_on_drop(true)
        .spawn()
        .expect("start copper-cloud");
    let server = Server {
        _child: child,
        _dir: dir,
        config,
        bin,
        link_code,
        log,
    };
    let link = parse_link_code(&server.link_code).expect("the server's link code reads");
    for _ in 0..240 {
        if CloudClient::probe_health(&link.endpoint, link.fingerprint.as_deref())
            .await
            .is_ok()
        {
            return Some(server);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!(
        "copper-cloud never answered /healthz:\n{}",
        server.log_tail()
    );
}

fn store(dir: &Path, name: &str) -> Store {
    Store::new(Paths {
        data: dir.join(name).join("data"),
        config: dir.join(name).join("config"),
    })
}

fn space(id: &str, name: &str) -> Space {
    Space {
        id: id.into(),
        name: name.into(),
        ..Default::default()
    }
}

fn kept(tab: &str, space: &str, url: &str, title: &str, pin: Option<&str>) -> LiveKept {
    LiveKept {
        tab_id: tab.into(),
        key: None,
        space: space.into(),
        pinned: pin.is_some(),
        url: url.into(),
        title: title.into(),
        pin: pin.map(str::to_string),
    }
}

fn bookmark(id: &str, title: &str, url: Option<&str>, kids: Option<Vec<Bookmark>>) -> Bookmark {
    Bookmark {
        id: id.into(),
        title: title.into(),
        url: url.map(str::to_string),
        children: kids,
        extra: Default::default(),
    }
}

fn visit(url: &str, title: &str, last: f64) -> LocalVisit {
    LocalVisit {
        url: url.into(),
        key: copper_core::address::history_key(&url::Url::parse(url).unwrap()),
        title: title.into(),
        count: 1,
        last,
    }
}

fn flat(roots: &[Bookmark]) -> BTreeSet<(String, String)> {
    copper_cloud::sync::merge::flatten(roots)
        .into_iter()
        .map(|n| (n.id, n.title))
        .collect()
}

fn space_ids(b: &MemoryBrowser) -> BTreeSet<String> {
    b.snapshot()
        .spaces
        .iter()
        .map(|s| s.id.to_uppercase())
        .collect()
}

fn history_keys(b: &MemoryBrowser) -> BTreeSet<String> {
    b.snapshot().visits.iter().map(|v| v.key.clone()).collect()
}

fn kept_urls(b: &MemoryBrowser) -> BTreeSet<(String, String)> {
    b.snapshot()
        .kept
        .iter()
        .map(|t| (t.space.to_uppercase(), t.url.clone()))
        .collect()
}

/// The engine's status once it is no longer `Syncing`. `turn_on` /
/// `sync_now` return when their own pass is done, but work the event stream
/// started meanwhile (a "ready" → pull of every doc, a "history" → history
/// pull) can still be in flight; that work settles the state when it ends.
/// Bounded: after 30 s the status is returned as it is (and the caller's
/// `Idle` assertion fails with it).
async fn settled(engine: &SyncEngine) -> copper_cloud::sync::SyncStatus {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let st = engine.status();
        if !matches!(st.state, copper_cloud::sync::SyncState::Syncing)
            || tokio::time::Instant::now() >= deadline
        {
            return st;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A site that answers `/r` with a 302 to another host name — `127.0.0.1` →
/// `localhost`, `/landed?from=r` — the way a site behind a sign-in sends a
/// tab that wakes without the session to its identity provider.
struct Redirector {
    port: u16,
    _task: tokio::task::JoinHandle<()>,
}

impl Redirector {
    async fn start() -> Redirector {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let reply = if path == "/r" {
                        format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}/landed?from=r\r\n\
                             Content-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                    } else {
                        let body = "<!doctype html><title>Landed</title>landed";
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
                             Connection: close\r\n\r\n{body}",
                            body.len()
                        )
                    };
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Redirector { port, _task: task }
    }

    fn kept_at(&self) -> String {
        format!("http://127.0.0.1:{}/r", self.port)
    }

    /// What a tab at `url` commits when it wakes: the address it ends on
    /// after following the server's redirects.
    async fn wake(&self, url: &str) -> String {
        let answer = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("the redirecting site answers");
        assert!(answer.status().is_success(), "{}", answer.status());
        answer.url().to_string()
    }
}

/// The server's spaces document now.
async fn server_spaces(client: &CloudClient) -> (i64, SpacesDoc) {
    let doc: Value = client
        .request_json("GET", "/v1/sync/docs/spaces", None, &[])
        .await
        .expect("GET spaces");
    let version = doc["version"].as_i64().unwrap();
    let bytes = decode_base64(doc["payload"].as_str().unwrap()).unwrap();
    (version, serde_json::from_slice(&bytes).unwrap())
}

fn entries(doc: &SpacesDoc) -> BTreeSet<(String, String, String)> {
    doc.tabs
        .iter()
        .map(|t| (t.key.clone(), t.space.to_uppercase(), t.url.clone()))
        .collect()
}

/// The same device, quit and launched again: the tabs come back from its
/// session with new ids (and the keys they carry), at the address each had
/// at the quit; a new engine over the same client and files, whose memory of
/// this run is gone. As the app does, it keys the restored tabs before any
/// page loads.
fn relaunch(
    client: &CloudClient,
    engine: SyncEngine,
    browser: &MemoryBrowser,
    run: u32,
) -> (SyncEngine, Arc<MemoryBrowser>) {
    drop(engine);
    let mut state = browser.snapshot();
    for t in &mut state.kept {
        t.tab_id = format!("{}@run{run}", t.tab_id);
    }
    let browser = Arc::new(MemoryBrowser::new(state));
    let engine = SyncEngine::new(client.clone(), browser.clone());
    engine.remember_kept_tabs();
    engine.start();
    (engine, browser)
}

/// A kept tab that redirects to another host when it wakes, quit and
/// launched again (twice), never changes the spaces document: not here, not
/// on the other device. Browsing inside a kept tab doesn't either — and what
/// the person then does to that tab still syncs as that tab.
async fn kept_tabs_across_relaunch(
    a: &CloudClient,
    b: &CloudClient,
    (engine_a, browser_a): (&SyncEngine, &Arc<MemoryBrowser>),
    (engine_b, browser_b): (SyncEngine, Arc<MemoryBrowser>),
) -> (SyncEngine, Arc<MemoryBrowser>) {
    let site = Redirector::start().await;
    let kept_at = site.kept_at();
    // A keeps it in Home; B gets it from the cloud, asleep, at /r.
    browser_a.edit(|s| s.kept.push(kept("a-r", HOME_A, &kept_at, "Hyper-V", None)));
    engine_a.sync_now().await;
    engine_b.sync_now().await;
    assert!(
        kept_urls(&browser_b).contains(&(HOME_A.into(), kept_at.clone())),
        "{:?}",
        kept_urls(&browser_b)
    );
    let (version, doc) = server_spaces(a).await;
    let entry = doc
        .tabs
        .iter()
        .find(|t| t.url == kept_at)
        .expect("the kept tab's entry")
        .clone();
    eprintln!("spaces v{version}: {} at {}", entry.key, entry.url);

    // B selects it: it wakes, and the site sends it to another host.
    let landed = site.wake(&kept_at).await;
    assert!(landed.starts_with("http://localhost:"), "{landed}");
    let woke = |b: &MemoryBrowser| {
        b.edit(|s| {
            for t in &mut s.kept {
                if t.url == kept_at || t.url == landed {
                    t.url = landed.clone();
                    t.title = "Landed".into();
                }
            }
        })
    };
    woke(&browser_b);
    engine_b.sync_now().await;
    assert_eq!(server_spaces(a).await, (version, doc.clone()));

    // Quit, launch again: restored at the redirect's address, it wakes and
    // lands there again. Twice. The document is the same version, byte for byte.
    let (mut engine_b, mut browser_b) = (engine_b, browser_b);
    for run in [2, 3] {
        (engine_b, browser_b) = relaunch(b, engine_b, &browser_b, run);
        woke(&browser_b);
        engine_b.sync_now().await;
        let st = settled(&engine_b).await;
        assert!(
            matches!(st.state, copper_cloud::sync::SyncState::Idle),
            "{:?}\n{:#?}",
            st.state,
            engine_b.log()
        );
        assert_eq!(st.versions.get("spaces"), Some(&version));
        assert_eq!(
            server_spaces(a).await,
            (version, doc.clone()),
            "run {run}: the redirect rewrote the spaces document\n{:#?}",
            engine_b.log()
        );
        assert!(
            !engine_b
                .log()
                .iter()
                .any(|(_, l)| l.starts_with("Pushed spaces")),
            "{:#?}",
            engine_b.log()
        );
        assert!(
            browser_b
                .snapshot()
                .kept
                .iter()
                .any(|t| t.url == landed && t.key.as_deref() == Some(entry.key.as_str())),
            "the restored tab carries its key"
        );
    }
    // A still has the tab at /r, and nothing from the redirect.
    engine_a.sync_now().await;
    assert!(kept_urls(browser_a).contains(&(HOME_A.into(), kept_at.clone())));
    assert!(
        !kept_urls(browser_a)
            .iter()
            .any(|(_, u)| u.contains("landed"))
    );

    // Browsing: on B the person takes the Mail pin to another site, quits,
    // and launches again. Still the same pin, at the address it was kept with.
    let mail = doc
        .tabs
        .iter()
        .find(|t| t.url == "https://mail.example.com/inbox")
        .expect("the Mail pin")
        .clone();
    browser_b.edit(|s| {
        for t in &mut s.kept {
            if t.url == mail.url {
                t.url = "https://news.example.org/today".into();
                t.title = "News".into();
            }
        }
    });
    engine_b.sync_now().await;
    (engine_b, browser_b) = relaunch(b, engine_b, &browser_b, 4);
    engine_b.sync_now().await;
    assert_eq!(server_spaces(a).await, (version, doc.clone()));

    // What the person then does to that tab still syncs, as that tab: B closes
    // the pin, the entry goes (and only it), and A's Mail pin closes.
    browser_b.edit(|s| s.kept.retain(|t| t.url != "https://news.example.org/today"));
    engine_b.sync_now().await;
    let (after, now) = server_spaces(a).await;
    assert_eq!(after, version + 1);
    let mut want = entries(&doc);
    want.retain(|(k, _, _)| *k != mail.key);
    assert_eq!(entries(&now), want);
    engine_a.sync_now().await;
    assert!(
        !kept_urls(browser_a)
            .iter()
            .any(|(_, u)| u == "https://mail.example.com/inbox"),
        "{:?}",
        kept_urls(browser_a)
    );
    assert!(kept_urls(browser_a).contains(&(HOME_A.into(), kept_at.clone())));
    (engine_b, browser_b)
}

const HOME_A: &str = "A0000000-0000-4000-8000-00000000000A";
const WORK_A: &str = "A0000000-0000-4000-8000-0000000000AB";
const HOME_B: &str = "B0000000-0000-4000-8000-00000000000B";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_server_end_to_end() {
    let Some(server) = start_server().await else {
        return;
    };
    let result = tokio::time::timeout(Duration::from_secs(600), scenario(&server)).await;
    match result {
        Ok(()) => {}
        Err(_) => panic!(
            "the live scenario timed out; server log:\n{}",
            server.log_tail()
        ),
    }
}

async fn scenario(server: &Server) {
    let tmp = tempfile::tempdir().unwrap();
    let link = parse_link_code(&server.link_code).unwrap();
    let real_fp = link
        .fingerprint
        .clone()
        .expect("a self-signed instance pins");
    eprintln!(
        "server at {} fp {}",
        link.host(),
        link.fingerprint_pairs().unwrap()
    );

    // --- Pinning: only the pinned certificate is trusted, never the system's.
    let flipped = if real_fp.starts_with('a') { 'b' } else { 'a' };
    let wrong = normalize_fingerprint(&format!("{flipped}{}", &real_fp[1..])).unwrap();
    assert_ne!(wrong, real_fp);
    let err = CloudClient::probe_health(&link.endpoint, Some(&wrong))
        .await
        .unwrap_err();
    assert_eq!(err.code, "fingerprint", "{err:?}");
    let mut wrong_link = link.clone();
    wrong_link.fingerprint = Some(wrong.clone());
    let refused = CloudClient::open(store(tmp.path(), "refused")).unwrap();
    let err = refused.connect(wrong_link.clone(), None).await.unwrap_err();
    assert_eq!(err.code, "fingerprint", "{err:?}");
    assert!(!refused.is_linked(), "a refused link is not kept");
    // A code without a fingerprint: never system trust; the fingerprint shown is offered.
    let mut bare = link.clone();
    bare.fingerprint = None;
    let err = refused.connect(bare.clone(), None).await.unwrap_err();
    assert_eq!(err.code, "untrusted");
    assert_eq!(err.seen.as_deref(), Some(real_fp.as_str()));
    // …and pinned to it once a person confirms it.
    refused.connect(bare, Some(&real_fp)).await.unwrap();
    assert_eq!(
        refused.link().unwrap().fingerprint.as_deref(),
        Some(real_fp.as_str())
    );
    refused.disconnect();
    // A wrong key is refused by the gate.
    let mut bad_key = link.clone();
    bad_key.key = "x".repeat(43);
    let err = refused.connect(bad_key, None).await.unwrap_err();
    assert_eq!(err.code, "instance_key");

    // --- Device A: link code, create an account.
    let a_store = store(tmp.path(), "a");
    let a = CloudClient::open(a_store.clone()).unwrap();
    a.start();
    match a.connect_code(&server.link_code, None).await.unwrap() {
        Code::Link(_) => {}
        other => panic!("{other:?}"),
    }
    assert!(a.status().linked && a.status().reachable);
    let info = a.load_info(true).await.expect("/v1/info");
    eprintln!(
        "server version {:?} features {:?}",
        info.version, info.features
    );
    let email = format!("linux-{}@example.test", uuid::Uuid::new_v4().simple());
    let password = "correct horse battery staple";
    assert_eq!(
        a.sign_up(&email, "short", "Ann").await.unwrap_err().code,
        "password"
    );
    let account = a.sign_up(&email, password, "Ann Linux").await.unwrap();
    assert_eq!(account.email, email);
    assert!(a.is_signed_in());
    let cloud_json = a_store.path(DataFile::Cloud);
    assert_private(&cloud_json);
    let saved: Value = serde_json::from_slice(&std::fs::read(&cloud_json).unwrap()).unwrap();
    assert!(saved["token"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(saved["link"]["fingerprint"], real_fp.as_str());
    assert_eq!(saved["account"]["email"], email.as_str());
    // Signing in again on the same device works too.
    a.sign_in(&email, password).await.unwrap();
    assert_eq!(
        a.sign_in(&email, "wrong password!")
            .await
            .unwrap_err()
            .message,
        "Wrong email or password"
    );
    // A wrong password doesn't sign the good session out.
    assert!(a.is_signed_in());
    a.me().await.unwrap();
    a.rename_device("Linux A").await.unwrap();
    let devices = a.devices().await.unwrap();
    assert!(
        devices
            .iter()
            .any(|d| d.name.as_deref() == Some("Linux A") && d.current),
        "{devices:?}"
    );

    // --- Device B: a pairing code from A links and signs in at once.
    let minted = a.mint_pairing_code(Some("Linux B")).await.unwrap();
    assert!(
        minted.link.contains(&format!("fp={real_fp}")),
        "{}",
        minted.link
    );
    assert_eq!(a.pairing_codes().await.unwrap().len(), 1);
    let b_store = store(tmp.path(), "b");
    let b = CloudClient::open(b_store.clone()).unwrap();
    b.start();
    match parse_code(&minted.link, None) {
        Some(Code::Pairing(_)) => {}
        other => panic!("{other:?}"),
    }
    b.connect_code(&minted.link, None).await.unwrap();
    assert!(b.is_signed_in());
    let b_account = b.account().unwrap();
    assert_eq!(b_account.user_id, a.account().unwrap().user_id);
    assert_ne!(b_account.device_id, a.account().unwrap().device_id);
    // Used once: it doesn't work again.
    let c = CloudClient::open(store(tmp.path(), "c")).unwrap();
    let err = c.connect_code(&minted.link, None).await.unwrap_err();
    assert_eq!(err.code, "pairing_code", "{err:?}");
    // A wrong pin on a pairing code is refused before anything is sent.
    let other = a.mint_pairing_code(None).await.unwrap();
    let tampered = other.link.replace(&real_fp, &wrong);
    let err = c.connect_code(&tampered, None).await.unwrap_err();
    assert_eq!(err.code, "fingerprint");
    a.revoke_pairing_code(&other.id).await.unwrap();
    assert!(a.pairing_codes().await.unwrap().is_empty());

    // --- Sync: two devices with different starting states.
    let now = copper_core::date::now_unix();
    let browser_a = Arc::new(MemoryBrowser::new(MemoryState {
        spaces: vec![space(HOME_A, "Home"), space(WORK_A, "Work")],
        kept: vec![
            kept(
                "a1",
                HOME_A,
                "https://mail.example.com/inbox",
                "Mail",
                Some("M"),
            ),
            kept("a2", WORK_A, "https://docs.example.com/", "Docs", None),
        ],
        settings: BTreeMap::from([
            ("look".into(), "dark".into()),
            ("shield".into(), "true".into()),
        ]),
        bookmarks: vec![bookmark(
            "BA000000-0000-4000-8000-000000000001",
            "Folder A",
            None,
            Some(vec![bookmark(
                "BA000000-0000-4000-8000-000000000002",
                "Rust",
                Some("https://www.rust-lang.org/"),
                None,
            )]),
        )],
        open: vec![OpenTab {
            url: "https://news.example.com/".into(),
            title: "News".into(),
            space: Some("Home".into()),
            active: Some(true),
            extra: Default::default(),
        }],
        visits: vec![
            visit("https://a-only.test/page", "A only", now - 300.0),
            visit("https://delete-me.test/x", "Delete me", now - 200.0),
        ],
        next_id: 0,
    }));
    let browser_b = Arc::new(MemoryBrowser::new(MemoryState {
        spaces: vec![space(HOME_B, "home")],
        kept: vec![kept(
            "b1",
            HOME_B,
            "https://calendar.example.com/",
            "Calendar",
            Some("C"),
        )],
        settings: BTreeMap::from([("look".into(), "light".into())]),
        bookmarks: vec![bookmark(
            "BB000000-0000-4000-8000-000000000001",
            "B's bookmark",
            Some("https://b.example.org/"),
            None,
        )],
        open: vec![OpenTab {
            url: "https://b-open.example.com/".into(),
            title: "B open".into(),
            space: Some("home".into()),
            active: None,
            extra: Default::default(),
        }],
        visits: vec![visit("https://b-only.test/", "B only", now - 100.0)],
        next_id: 0,
    }));
    let engine_a = SyncEngine::new(a.clone(), browser_a.clone());
    engine_a.start();
    let engine_b = SyncEngine::new(b.clone(), browser_b.clone());
    engine_b.start();
    let all = [
        Domain::Spaces,
        Domain::Settings,
        Domain::Bookmarks,
        Domain::Tabs,
        Domain::History,
        Domain::Canvas,
    ];

    engine_a.turn_on(&all).await;
    let st = settled(&engine_a).await;
    assert!(st.on, "{st:?}");
    assert!(
        matches!(st.state, copper_cloud::sync::SyncState::Idle),
        "{:?}\n{:#?}",
        st.state,
        engine_a.log()
    );
    for d in ["spaces", "settings", "bookmarks"] {
        assert!(
            st.versions.contains_key(d),
            "{d} pushed: {st:?}\n{:#?}",
            engine_a.log()
        );
    }
    assert!(st.history_pushed_through.is_some());

    let mut events_a = a.subscribe();
    engine_b.turn_on(&all).await;
    let st_b = settled(&engine_b).await;
    assert!(
        matches!(st_b.state, copper_cloud::sync::SyncState::Idle),
        "{st_b:?}\n{:#?}",
        engine_b.log()
    );
    // First sync on B merged without loss: B's "home" became A's Home (same name),
    // A's Work arrived, both kept tabs are there, both bookmarks, the cloud's settings.
    assert_eq!(
        space_ids(&browser_b),
        BTreeSet::from([HOME_A.to_string(), WORK_A.to_string()])
    );
    assert!(
        kept_urls(&browser_b).contains(&(HOME_A.into(), "https://calendar.example.com/".into()))
    );
    assert!(
        kept_urls(&browser_b).contains(&(HOME_A.into(), "https://mail.example.com/inbox".into()))
    );
    assert!(kept_urls(&browser_b).contains(&(WORK_A.into(), "https://docs.example.com/".into())));
    let b_marks = flat(&browser_b.snapshot().bookmarks);
    assert!(b_marks.iter().any(|(_, t)| t == "Rust"));
    assert!(b_marks.iter().any(|(_, t)| t == "B's bookmark"));
    assert_eq!(
        browser_b
            .snapshot()
            .settings
            .get("look")
            .map(String::as_str),
        Some("dark")
    );
    assert!(
        history_keys(&browser_b).contains("a-only.test/page"),
        "{:?}",
        history_keys(&browser_b)
    );

    // A hears about B's writes on the event stream.
    let heard = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match events_a.recv().await {
                Ok(e) if e.kind == "doc" => return e,
                Ok(_) => continue,
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .expect("a doc event reached A");
    eprintln!("A heard: {}", heard.data);

    engine_a.sync_now().await;
    assert_eq!(space_ids(&browser_a), space_ids(&browser_b));
    assert_eq!(kept_urls(&browser_a), kept_urls(&browser_b));
    assert_eq!(
        flat(&browser_a.snapshot().bookmarks),
        flat(&browser_b.snapshot().bookmarks)
    );
    assert!(history_keys(&browser_a).contains("b-only.test"));
    assert_eq!(history_keys(&browser_a), history_keys(&browser_b));
    // Other devices' open tabs.
    let others = engine_a.other_devices();
    assert_eq!(others.len(), 1, "{others:?}");
    assert_eq!(others[0].tabs[0].url, "https://b-open.example.com/");
    assert!(engine_b.other_devices().iter().any(|d| d.name == "Linux A"));

    // --- Edits flow both ways after the first sync.
    browser_b.edit(|s| {
        for sp in &mut s.spaces {
            if sp.id.eq_ignore_ascii_case(WORK_A) {
                sp.name = "Work (renamed on B)".into();
            }
        }
        s.bookmarks.retain(|b| b.title != "B's bookmark");
        s.settings.insert("look".into(), "system".into());
    });
    engine_b.sync_now().await;
    engine_a.sync_now().await;
    let a_now = browser_a.snapshot();
    assert!(a_now.spaces.iter().any(|s| s.name == "Work (renamed on B)"));
    assert!(
        !flat(&a_now.bookmarks)
            .iter()
            .any(|(_, t)| t == "B's bookmark")
    );
    assert_eq!(
        a_now.settings.get("look").map(String::as_str),
        Some("system")
    );
    // A closes a kept tab; B loses it.
    browser_a.edit(|s| s.kept.retain(|t| t.url != "https://docs.example.com/"));
    engine_a.sync_now().await;
    engine_b.sync_now().await;
    assert!(
        !kept_urls(&browser_b)
            .iter()
            .any(|(_, u)| u == "https://docs.example.com/")
    );

    // --- Kept tabs across a relaunch: a 302 to another host, and browsing.
    let (engine_b, browser_b) =
        kept_tabs_across_relaunch(&a, &b, (&engine_a, &browser_a), (engine_b, browser_b)).await;

    // --- History: the 16 KiB entry limit.
    let long_title = "t".repeat(20_000);
    let huge_url = format!("https://huge.test/?q={}", "a".repeat(20_000));
    browser_a.edit(|s| {
        s.visits
            .push(visit("https://long-title.test/", &long_title, now - 50.0));
        s.visits.push(visit(&huge_url, "Huge", now - 40.0));
        s.visits
            .push(visit("https://after.test/", "After", now - 30.0));
    });
    engine_a.push_history(true).await;
    assert!(
        engine_a
            .log()
            .iter()
            .any(|(_, l)| l.contains("left out a visit to huge.test")),
        "{:#?}",
        engine_a.log()
    );
    assert!(
        engine_a
            .log()
            .iter()
            .any(|(_, l)| l.contains("shortened 1 long title")),
        "{:#?}",
        engine_a.log()
    );
    engine_b.pull_history().await;
    let b_visits = browser_b.snapshot().visits;
    let shortened = b_visits
        .iter()
        .find(|v| v.key == "long-title.test")
        .expect("the long title went up");
    assert!(shortened.title.ends_with('…') && shortened.title.len() < 16_384);
    assert!(
        b_visits.iter().any(|v| v.key == "after.test"),
        "a visit after the refused one went up"
    );
    assert!(
        !b_visits
            .iter()
            .any(|v| v.url.starts_with("https://huge.test/"))
    );

    // --- Deleting history on the cloud.
    assert_eq!(a.deletes_history(), Some(true));
    let outcome = engine_a
        .delete_history(&Selector {
            hosts: vec!["delete-me.test".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(outcome.deleted, 1, "{outcome:?}");
    let rows: Value = a
        .request_json(
            "GET",
            "/v1/sync/history",
            None,
            &[("since", "0"), ("limit", "500")],
        )
        .await
        .unwrap();
    let urls: Vec<&str> = rows["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["payload"]["url"].as_str())
        .collect();
    assert!(
        !urls.iter().any(|u| u.contains("delete-me.test")),
        "{urls:?}"
    );
    assert!(urls.iter().any(|u| u.contains("b-only.test")));
    // A push after the delete doesn't send it back.
    engine_a.push_history(true).await;
    let rows: Value = a
        .request_json(
            "GET",
            "/v1/sync/history",
            None,
            &[("since", "0"), ("limit", "500")],
        )
        .await
        .unwrap();
    assert!(!rows.to_string().contains("delete-me.test"));

    // --- Intelligence keys from the cloud.
    let intel = CloudIntelligence::new(a.clone());
    assert_eq!(intel.fetch().await, None);
    assert_eq!(intel.last_outcome(), "ok (nothing provided)");
    let key_file = tmp.path().join("jev.key");
    std::fs::write(&key_file, "jev_test_key_0123456789").unwrap();
    let out = server
        .cli(&[
            "intelligence",
            "set",
            "--jev-key-file",
            key_file.to_str().unwrap(),
            "--agent-max-turns",
            "42",
        ])
        .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let provided = intel.fetch().await.expect("keys provided");
    assert_eq!(
        provided.jev.as_ref().unwrap().key.as_deref(),
        Some("jev_test_key_0123456789")
    );
    assert_eq!(provided.agent_max_turns(), Some(42));
    let cache = a_store.path(DataFile::CloudIntelligence);
    assert_private(&cache);
    assert_eq!(
        CloudIntelligence::new(a.clone())
            .provided()
            .unwrap()
            .agent_max_turns(),
        Some(42)
    );

    // --- Canvas: REST, the live room, presence, ops, chat.
    let api_a = CanvasApi::new(a.clone());
    let api_b = CanvasApi::new(b.clone());
    let list = api_a.list().await.unwrap();
    assert!(list.iter().any(|c| c.kind == "personal"), "{list:?}");
    let canvas = api_a.create("Linux test board").await.unwrap();
    assert_eq!(canvas.kind, "shared");
    let me_a = Me {
        id: a.account().unwrap().user_id.to_lowercase(),
        name: "Ann (A)".into(),
        color: "#ff7a00".into(),
    };
    let me_b = Me {
        id: me_a.id.clone(),
        name: "Ann (B)".into(),
        color: "#0070f3".into(),
    };
    let live_a = LiveCanvas::connect(a.clone(), &canvas.id, me_a, CanvasDoc::new());
    let live_b = LiveCanvas::connect(b.clone(), &canvas.id, me_b, CanvasDoc::new());
    assert!(
        live_a.synced(Duration::from_secs(15)).await,
        "{:?}",
        live_a.status()
    );
    assert!(
        live_b.synced(Duration::from_secs(15)).await,
        "{:?}",
        live_b.status()
    );
    assert_eq!(live_b.doc().meta()["name"], "Linux test board");
    // Ops through the server reach both documents.
    let result = api_b
        .apply_ops(
            &canvas.id,
            &[json!({"op": "add", "shape": {"id": "goal", "type": "sticky", "text": "Ship Linux", "color": "green"}})],
            Some(&Author {
                id: None,
                name: "Linux test".into(),
                color: None,
            }),
            false,
        )
        .await
        .unwrap();
    assert_eq!(result.applied, 1, "{result:?}");
    assert!(
        live_a
            .wait_for(Duration::from_secs(10), |s| s.remote_updates >= 2)
            .await
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let shapes = live_a.doc().shapes();
    assert_eq!(shapes["goal"]["text"], "Ship Linux", "{shapes:?}");
    // Presence.
    live_a.set_presence(serde_json::Map::from_iter([(
        "cursor".into(),
        json!({"x": 10, "y": 20}),
    )]));
    assert!(
        live_b
            .wait_for(Duration::from_secs(10), |s| s.peers >= 1)
            .await
    );
    let peers = live_b.peers();
    assert_eq!(peers[0].state["user"]["name"], "Ann (A)");
    assert_eq!(peers[0].state["cursor"]["x"], 10);
    // Chat, written into the document like the page does.
    let message = live_a.chat("hello from Linux A", &[]).unwrap();
    let mut got = false;
    for _ in 0..40 {
        if live_b
            .doc()
            .chat()
            .iter()
            .any(|m| m["id"] == message.as_str())
        {
            got = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(got, "B saw A's chat");
    let read = api_a.read(&canvas.id, true, &[], &[]).await.unwrap();
    assert!(read.to_string().contains("hello from Linux A"), "{read}");
    // Leaving presence.
    live_a.clear_presence();
    assert!(
        live_b
            .wait_for(Duration::from_secs(10), |s| s.peers == 0)
            .await
    );
    // Rename shows up live; delete closes the room with 4404.
    api_a.rename(&canvas.id, "Renamed board").await.unwrap();
    let mut renamed = false;
    for _ in 0..40 {
        if live_b.doc().meta().get("name") == Some(&json!("Renamed board")) {
            renamed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(renamed);
    api_a.delete(&canvas.id).await.unwrap();
    assert!(
        live_b
            .wait_for(Duration::from_secs(10), |s| s.gone == Some(4404))
            .await,
        "{:?}",
        live_b.status()
    );

    // --- Sign out: B's session ends on the server too.
    engine_b.sign_out().await;
    assert!(!b.is_signed_in());
    assert!(!engine_b.is_on());
    assert!(a.me().await.is_ok());
    // B's open tabs were cleared from the server before it left.
    engine_a.sync_now().await;
    assert!(
        engine_a.other_devices().is_empty(),
        "{:?}",
        engine_a.other_devices()
    );
    let err: Failure = b.me().await.unwrap_err();
    assert_eq!(err.code, "not_signed_in");

    a.disconnect();
    assert!(!a.is_linked());
    let saved: Value = serde_json::from_slice(&std::fs::read(&cloud_json).unwrap()).unwrap();
    assert!(saved.get("token").is_none() && saved.get("link").is_none());
}
