//! Bitwarden and 1Password through their CLIs, against the mocks in
//! tests/fixtures (op-mock is copper-ref's docs/fixtures/op-mock/op).
//! Unix only: the mocks are shell/Python scripts run through their shebang,
//! and the tests check 0600/0700 modes.
#![cfg(unix)]

use std::path::PathBuf;

use copper_creds::bitwarden::{Bitwarden, BwConfig, BwState};
use copper_creds::credential::{AgentHint, CredentialId, combine};
use copper_creds::onepassword::{Mode, OnePassword, OpConfig, OpState, Problem, Remembered};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("copper-creds-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn calls(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

fn mode_of(p: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

// ------------------------------------------------------------- Bitwarden

fn bw(dir: &std::path::Path, stay: bool) -> Bitwarden {
    Bitwarden::new(
        BwConfig {
            path: Some(fixture("bw-mock/bw")),
            app_data: dir.join("appdata"),
            env: vec![(
                "BW_MOCK_STATE".into(),
                dir.join("mock/state.json").to_string_lossy().into_owned(),
            )],
        },
        stay,
    )
}

#[tokio::test]
async fn bitwarden_end_to_end() {
    let dir = scratch("bw");
    let mut b = bw(&dir, true);
    b.start().await;
    assert_eq!(*b.state(), BwState::Unauthenticated);
    assert_eq!(b.version().await.as_deref(), Some("2026.9.0-mock"));

    let wrong = b
        .login("ann@example.com", "nope", None, None)
        .await
        .unwrap_err();
    assert_eq!(wrong.0, "Wrong email or master password");
    b.login("ann@example.com", "correct horse", None, None)
        .await
        .unwrap();
    assert!(b.is_unlocked(), "{:?}", b.state());
    assert_eq!(b.counts(), (3, 1, 1, 1));

    // Session kept 0600 for "stay unlocked"; a fresh instance resumes it.
    let session = dir.join("appdata/session");
    assert_eq!(mode_of(&session), 0o600);
    assert_eq!(mode_of(&dir.join("appdata")), 0o700);
    let mut again = bw(&dir, true);
    again.start().await;
    assert!(again.is_unlocked(), "resume: {:?}", again.state());

    // Matching, rows, agent hints.
    let rows = b.credentials_for("www.example.com");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].id, CredentialId::Bitwarden("i-example".into()));
    assert!(rows[0].has_totp);
    let exact = b.credentials_for("accounts.example.com");
    let agent = exact.iter().find(|c| c.user == "bot").expect("host match");
    assert_eq!(agent.agent_hint, AgentHint::Allow);
    assert_eq!(agent.folder.as_deref(), Some("Agents"));
    let dev = b.credentials_for("localhost");
    assert_eq!(dev.len(), 1);
    assert_eq!(dev[0].agent_hint, AgentHint::Deny);
    // Any item's hint by its id: a login's, and none for a card outside Agents.
    assert_eq!(b.agent_hint("i-agent"), Some(AgentHint::Allow));
    assert_eq!(b.agent_hint("i-local"), Some(AgentHint::Deny));
    assert_eq!(b.agent_hint("i-card"), Some(AgentHint::None));
    assert_eq!(b.agent_hint("nope"), None);

    // Secrets from the in-memory cache, TOTP computed locally (RFC seed).
    assert_eq!(b.password("i-example").await.unwrap(), "example-pw");
    let code = b.totp("i-example").await.unwrap();
    assert_eq!(code.len(), 6);
    assert_ne!(code, "123456", "computed here, not asked of bw");
    assert_eq!(b.field_value("i-example", "PIN").as_deref(), Some("9876"));
    assert_eq!(
        b.field_value("i-example", "Account").as_deref(),
        Some("AC-7")
    );
    assert_eq!(
        b.field_value("i-example", "Remember").as_deref(),
        Some("true")
    );
    assert_eq!(b.notes("i-note").as_deref(), Some("the wifi is hunter2"));
    assert_eq!(b.cards()[0].last4(), "1111");
    assert_eq!(b.cards()[0].exp_month, "12");
    assert_eq!(b.identities()[0].city, "Springfield");

    // Save: update and create go through `bw encode` + stdin.
    b.update("i-example", "example-pw-2").await.unwrap();
    assert_eq!(b.password("i-example").await.unwrap(), "example-pw-2");
    let id = b
        .create("new.example", "zoe", "zoe-pw", None)
        .await
        .unwrap();
    assert_eq!(b.password(&id).await.unwrap(), "zoe-pw");
    assert_eq!(b.credentials_for("new.example")[0].user, "zoe");
    assert_eq!(
        b.credentials_for("new.example")[0].sites,
        ["https://new.example"]
    );
    // Saved from a page: its scheme and port are the item's address.
    b.create(
        "dev.example",
        "yan",
        "yan-pw",
        Some("http://dev.example:8080"),
    )
    .await
    .unwrap();
    assert_eq!(
        b.credentials_for("dev.example")[0].sites,
        ["http://dev.example:8080"]
    );

    // Merge with other sources.
    let merged = combine(vec![], b.credentials_for("example.com"), vec![]);
    assert!(!merged.is_empty());

    // Lock wipes the cache and the session file.
    b.lock().await;
    assert!(matches!(b.state(), BwState::Locked { .. }));
    assert!(b.credentials_for("example.com").is_empty());
    assert!(b.password("i-example").await.is_err());
    assert!(!session.exists());
    let bad = b.unlock("wrong").await.unwrap_err();
    assert_eq!(bad.0, "Wrong master password — try again");
    b.unlock("correct horse").await.unwrap();
    assert!(b.is_unlocked());

    // API key login after a logout.
    b.logout().await;
    assert_eq!(*b.state(), BwState::Unauthenticated);
    b.login_api_key("user.mock", "mock-secret").await.unwrap();
    assert!(
        matches!(b.state(), BwState::Locked { .. }),
        "{:?}",
        b.state()
    );

    // No secret ever on argv; BITWARDENCLI_APPDATA_DIR always Copper's.
    let log = calls(&dir.join("mock"));
    for secret in [
        "correct horse",
        "example-pw",
        "zoe-pw",
        "mock-secret",
        "9876",
    ] {
        assert!(!log.contains(secret), "{secret} reached argv");
    }
    let appdata = dir.join("appdata").to_string_lossy().into_owned();
    assert!(log.lines().all(|l| l.contains(&appdata)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `copper bitwarden save -|note -` / browser_vault_* against the mock
/// (BitwardenSave.swift): create, update in place only when shared, the
/// Agents folder made in folder mode, notes, a locked vault, and errors
/// with every value cut out.
#[tokio::test]
async fn bitwarden_vault_write() {
    use serde_json::{Value, json};
    let dir = scratch("bw-save");
    let mut b = bw(&dir, false);
    b.start().await;
    b.login("ann@example.com", "correct horse", None, None)
        .await
        .unwrap();
    let args = |v: Value| v.as_object().cloned().unwrap();
    let none: &[String] = &[];
    let state = || -> Value {
        serde_json::from_str(&std::fs::read_to_string(dir.join("mock/state.json")).unwrap())
            .unwrap()
    };
    let item = |id: &str| -> Value {
        state()["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id)
            .cloned()
            .unwrap()
    };

    // Folder mode, no Agents folder yet: it is made and the login lands there.
    let mut st = state();
    st["folders"] = json!([{"id": "f-work", "name": "Work"}]);
    std::fs::write(dir.join("mock/state.json"), st.to_string()).unwrap();
    let made = b
        .vault_write(
            "save",
            &args(
                json!({"url": "https://www.pubadhub.com/login?token=abc", "username": "me@x.com",
                         "password": "s3cret-pw", "totp": "jbsw y3dp ehpk 3pxp"}),
            ),
            false,
            none,
        )
        .await;
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["created"], true);
    assert_eq!(made["updated"], false);
    assert_eq!(made["name"], "pubadhub.com");
    assert_eq!(made["folder"], "Agents");
    let id = made["id"].as_str().unwrap().to_string();
    let saved = item(&id);
    assert_eq!(saved["login"]["password"], "s3cret-pw");
    assert_eq!(saved["login"]["totp"], "JBSWY3DPEHPK3PXP");
    assert_eq!(saved["login"]["uris"][0]["uri"], "https://www.pubadhub.com");
    assert_eq!(
        saved["notes"],
        Value::Null,
        "the template's sample notes are dropped"
    );
    let agents = state()["folders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Agents")
        .cloned()
        .expect("Agents folder made");
    assert_eq!(saved["folderId"], agents["id"]);
    assert!(!made.to_string().contains("s3cret-pw"));
    // Usable at once: the cache knows it, an Agents-folder login.
    assert_eq!(b.password(&id).await.unwrap(), "s3cret-pw");

    // The same site and user (any case): updated in place, it is shared.
    let again = b
        .vault_write(
            "save",
            &args(
                json!({"host": "pubadhub.com", "username": "ME@x.com", "password": "n3w-pw",
                         "notes": "recovery: ask Ann"}),
            ),
            false,
            none,
        )
        .await;
    assert_eq!(
        again,
        json!({"ok": true, "id": id, "created": false, "updated": true,
               "name": "pubadhub.com", "folder": "Agents"})
    );
    let saved = item(&id);
    assert_eq!(saved["login"]["password"], "n3w-pw");
    assert_eq!(saved["login"]["totp"], "JBSWY3DPEHPK3PXP", "kept");
    assert_eq!(saved["notes"], "recovery: ask Ann");

    // A login the user kept back is refused, unless shared.
    let ask =
        args(json!({"url": "example.com", "username": "ann@example.com", "password": "hijack-pw"}));
    let refused = b.vault_write("save", &ask, false, none).await;
    assert_eq!(refused["ok"], false);
    assert_eq!(
        refused["error"],
        "a login for ann@example.com on example.com is already saved and not shared with agents — share it in Settings › Passwords › Agent access, or ask the user to change it"
    );
    assert_eq!(item("i-example")["login"]["password"], "example-pw");
    let shared = b
        .vault_write("save", &ask, false, &["bw:i-example".to_string()])
        .await;
    assert_eq!(shared["updated"], true, "{shared}");
    assert_eq!(shared["folder"], Value::Null);
    assert_eq!(item("i-example")["login"]["password"], "hijack-pw");
    // `copper-agent: deny` wins even over share-all.
    let denied = b
        .vault_write(
            "save",
            &args(json!({"url": "http://localhost:8765", "username": "dev", "password": "x-pw"})),
            true,
            none,
        )
        .await;
    assert_eq!(denied["ok"], false, "{denied}");

    // Share-all: a new item goes where asked (here: nowhere), no folder made.
    let free = b
        .vault_write(
            "save",
            &args(json!({"url": "new.example.org", "username": "zed", "password": "zed-pw"})),
            true,
            none,
        )
        .await;
    assert_eq!(free["folder"], Value::Null, "{free}");
    let work = b
        .vault_write(
            "save",
            &args(json!({"url": "w.example.org", "username": "w", "password": "w-pw", "folder": "work"})),
            true,
            none,
        )
        .await;
    assert_eq!(work["folder"], "Work", "any case finds the folder");

    // Notes: created in Agents, then replaced by name; the user's is refused.
    let note = args(json!({"name": "Google backup codes", "text": "1111 2222\n3333 4444\n"}));
    let made = b.vault_write("note", &note, false, none).await;
    assert_eq!(made["created"], true, "{made}");
    assert_eq!(made["folder"], "Agents");
    let nid = made["id"].as_str().unwrap().to_string();
    assert_eq!(item(&nid)["type"], 2);
    assert_eq!(item(&nid)["secureNote"]["type"], 0);
    assert_eq!(b.notes(&nid).as_deref(), Some("1111 2222\n3333 4444\n"));
    let replaced = b
        .vault_write(
            "note",
            &args(json!({"name": "Google backup codes", "text": "5555 6666"})),
            false,
            none,
        )
        .await;
    assert_eq!(
        replaced,
        json!({"ok": true, "id": nid, "created": false, "updated": true,
               "name": "Google backup codes", "folder": "Agents"})
    );
    assert_eq!(item(&nid)["notes"], "5555 6666");
    let wifi = b
        .vault_write(
            "note",
            &args(json!({"name": "Wifi", "text": "open"})),
            false,
            none,
        )
        .await;
    assert!(
        wifi["error"]
            .as_str()
            .unwrap()
            .starts_with("a note named Wifi is already saved and not shared")
    );

    // A bw error quoting what it was sent comes back with every value cut.
    let quoted = b
        .vault_write(
            "save",
            &args(
                json!({"url": "q.example.org", "username": "q", "password": "leak-me-pw",
                         "notes": "line one secret\nline two secret", "name": "mock-refuse"}),
            ),
            true,
            none,
        )
        .await;
    let said = quoted["error"].as_str().unwrap();
    assert!(said.starts_with("Invalid item:"), "{said}");
    for secret in ["leak-me-pw", "line one secret", "line two secret"] {
        assert!(!said.contains(secret), "{said}");
    }

    // Bad input and a locked vault: plain errors, never the values.
    let bad = b
        .vault_write(
            "save",
            &args(json!({"url": "x.com", "username": "u"})),
            false,
            none,
        )
        .await;
    assert_eq!(bad, json!({"ok": false, "error": "password is required"}));
    b.lock().await;
    let locked = b.vault_write("note", &note, false, none).await;
    assert_eq!(locked, json!({"ok": false, "error": "Bitwarden is locked"}));

    // No value ever on argv.
    let log = calls(&dir.join("mock"));
    for secret in [
        "s3cret-pw",
        "n3w-pw",
        "JBSWY3DP",
        "1111 2222",
        "hijack-pw",
        "leak-me-pw",
    ] {
        assert!(!log.contains(secret), "{secret} reached argv");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn bitwarden_missing() {
    let dir = scratch("bw-missing");
    let mut b = Bitwarden::new(
        BwConfig {
            path: Some(dir.join("no-such-bw")),
            app_data: dir.join("appdata"),
            env: vec![],
        },
        false,
    );
    // An explicit path that isn't there: start() says missing… unless a real
    // bw is installed on PATH — the explicit path wins, so it is missing.
    b.start().await;
    assert!(b.unlock("x").await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------- 1Password

fn op(dir: &std::path::Path) -> OnePassword {
    OnePassword::new(
        OpConfig {
            path: Some(fixture("op-mock/op")),
            folder: dir.join("1password"),
            config_dir: None,
            env: vec![(
                "OP_MOCK_STATE".into(),
                dir.join("mock/state.json").to_string_lossy().into_owned(),
            )],
        },
        true,
    )
}

fn mock(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(fixture("op-mock/op"))
        .args(args)
        .env("OP_MOCK_STATE", dir.join("mock/state.json"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
async fn onepassword_password_mode() {
    let dir = scratch("op-pw");
    let mut o = op(&dir);
    o.start(Remembered::default()).await;
    assert_eq!(o.state, OpState::SignedOut);
    assert_eq!(o.accounts().len(), 1);
    assert_eq!(o.version().await.as_deref(), Some("2.39.0-mock"));

    let wrong = o.sign_in("nope", None).await.unwrap_err();
    assert_eq!(wrong.problem, Problem::WrongPassword);
    o.sign_in("correct horse battery staple", None)
        .await
        .unwrap();
    assert!(o.is_unlocked());
    assert_eq!(o.mode(), Some(Mode::Password));
    assert_eq!(o.account().unwrap().email, "alice@example.com");
    assert_eq!(mode_of(&dir.join("1password/session")), 0o600);

    // Details read in batches: secrets, hidden fields, TOTP, identity, card.
    let rows = o.credentials_for("localhost");
    let users: Vec<&str> = rows.iter().map(|c| c.user.as_str()).collect();
    assert_eq!(rows.len(), 2, "{users:?}");
    let alice = rows.iter().find(|c| c.user == "alice@example.com").unwrap();
    assert!(alice.has_totp);
    assert_eq!(alice.agent_hint, AgentHint::None);
    let CredentialId::OnePassword(alice_id) = alice.id.clone() else {
        panic!()
    };
    assert_eq!(o.password(&alice_id).await.unwrap(), "alice-localhost-pw");
    assert_eq!(o.totp(&alice_id).await.unwrap().len(), 6);
    assert_eq!(o.field_value(&alice_id, "PIN").as_deref(), Some("4321"));
    assert_eq!(
        o.field_value(&alice_id, "Account number").as_deref(),
        Some("AC-1001")
    );
    assert_eq!(o.credentials_for("example.org").len(), 1);
    let who = &o.identities()[0];
    assert_eq!(
        (
            who.first_name.as_str(),
            who.city.as_str(),
            who.country.as_str()
        ),
        ("Ada", "Springfield", "US")
    );
    let card = &o.cards()[0];
    assert_eq!(
        (
            card.brand.as_str(),
            card.exp_month.as_str(),
            card.exp_year.as_str()
        ),
        ("Visa", "12", "2028")
    );
    assert_eq!(card.last4(), "1111");
    assert_eq!(o.save_vault().unwrap().name, "Private");

    // Update (whole item through stdin) and create.
    o.update(&alice_id, "alice-new-pw").await.unwrap();
    assert_eq!(
        mock(
            &dir,
            &[
                "mock",
                "check",
                "Localhost",
                "alice@example.com",
                "alice-new-pw"
            ]
        ),
        "true"
    );
    assert_eq!(o.password(&alice_id).await.unwrap(), "alice-new-pw");
    let made = o
        .create("new.example", "zed", "zed-pw", None)
        .await
        .unwrap();
    assert_eq!(
        mock(&dir, &["mock", "check", "new.example", "zed", "zed-pw"]),
        "true"
    );
    assert_eq!(o.password(&made).await.unwrap(), "zed-pw");
    assert_eq!(o.credentials_for("new.example").len(), 1);

    // A resumed launch reads the kept session and is unlocked at once.
    let remembered = o.remembered();
    let mut again = op(&dir);
    again.start(remembered.clone()).await;
    assert!(again.is_unlocked(), "{:?}", again.problem());
    assert!(!again.items().is_empty());

    // Expiry on 1Password's side locks and wipes.
    mock(&dir, &["mock", "expire"]);
    assert!(again.sync().await.is_err());
    assert_eq!(again.state, OpState::Locked);
    assert!(again.items().is_empty());

    o.lock().await;
    assert_eq!(o.state, OpState::Locked);
    assert!(o.credentials_for("localhost").is_empty());
    assert!(!dir.join("1password/session").exists());

    let log = calls(&dir.join("mock"));
    for secret in [
        "correct horse battery staple",
        "alice-new-pw",
        "zed-pw",
        "4321",
    ] {
        assert!(!log.contains(secret), "{secret} reached argv");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn onepassword_app_and_service() {
    let dir = scratch("op-app");
    let mut o = op(&dir);
    o.start(Remembered::default()).await;
    // The app integration: no token, the app's approval.
    mock(&dir, &["mock", "set", "app_integration", "on"]);
    mock(&dir, &["mock", "set", "app_approve", "off"]);
    let dismissed = o.unlock_with_app(None).await.unwrap_err();
    assert_eq!(dismissed.problem, Problem::Dismissed);
    mock(&dir, &["mock", "set", "app_approve", "on"]);
    o.unlock_with_app(None).await.unwrap();
    assert_eq!(o.mode(), Some(Mode::App));
    assert!(o.is_unlocked());
    assert_eq!(o.credentials_for("localhost").len(), 2);
    assert!(
        !dir.join("1password/session").exists(),
        "app mode keeps no token"
    );
    o.sign_out().await;
    assert_eq!(o.state, OpState::SignedOut);

    // A read-only service account: reads, but can't take a save.
    mock(&dir, &["mock", "set", "app_integration", "off"]);
    let bad = o.connect("not-a-token").await.unwrap_err();
    assert_eq!(bad.problem, Problem::BadToken);
    o.connect("ops_mockServiceTokenReadOnly0001").await.unwrap();
    assert_eq!(o.mode(), Some(Mode::Service));
    assert_eq!(mode_of(&dir.join("1password/service")), 0o600);
    let rows = o.credentials_for("localhost");
    assert_eq!(rows.len(), 1, "only the Shared vault: {rows:?}");
    assert!(o.save_vault().is_none());
    assert!(o.create("x.example", "u", "p", None).await.is_err());
    let CredentialId::OnePassword(bob) = rows[0].id.clone() else {
        panic!()
    };
    assert_eq!(o.password(&bob).await.unwrap(), "bob-localhost-pw");
    let log = calls(&dir.join("mock"));
    assert!(!log.contains("ops_mockServiceTokenReadOnly0001"));
    let _ = std::fs::remove_dir_all(&dir);
}
