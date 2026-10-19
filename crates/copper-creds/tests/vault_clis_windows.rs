//! Bitwarden through `bw.cmd` on Windows — the shape npm installs — against
//! the Python mock in tests/fixtures/bw-mock (`bw.cmd` runs `python bw`).
//! What it proves: a batch file spawns (no console window, inside a job),
//! arguments with `cmd.exe` metacharacters arrive intact, secrets travel by
//! environment and stdin only, and the session file persists.
#![cfg(windows)]

use std::path::{Path, PathBuf};

use copper_creds::bitwarden::{Bitwarden, BwConfig, BwState};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests\\fixtures")
        .join(name)
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("copper-creds-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn bw(dir: &Path, stay: bool) -> Bitwarden {
    Bitwarden::new(
        BwConfig {
            path: Some(fixture("bw-mock\\bw.cmd")),
            app_data: dir.join("appdata"),
            env: vec![(
                "BW_MOCK_STATE".into(),
                dir.join("mock\\state.json").to_string_lossy().into_owned(),
            )],
        },
        stay,
    )
}

#[tokio::test]
async fn bitwarden_through_a_batch_file() {
    let dir = scratch("bw-cmd");
    let mut b = bw(&dir, true);
    b.start().await;
    assert_eq!(*b.state(), BwState::Unauthenticated);
    assert_eq!(b.version().await.as_deref(), Some("2026.9.0-mock"));

    // `&`, `%`, `^`, `|`, `<`, `>` and spaces reach the program unchanged.
    let server = "https://vault.example.com/a b?x=1&y=%PATH%&z=^|<>!";
    b.configure(server).await.unwrap();
    b.refresh_status().await;
    assert_eq!(b.cli_server(), Some(server));
    // What a command line can't carry is refused before anything runs.
    assert!(b.configure("https://x.example/\"quoted\"").await.is_err());

    b.login("ann@example.com", "correct horse", None, None)
        .await
        .unwrap();
    assert!(b.is_unlocked(), "{:?}", b.state());
    assert!(dir.join("appdata\\session").is_file());
    let mut again = bw(&dir, true);
    again.start().await;
    assert!(again.is_unlocked(), "resume: {:?}", again.state());

    assert_eq!(b.password("i-example").await.unwrap(), "example-pw");
    // Save goes through `bw encode` + stdin.
    let id = b.create("new.example", "zoe", "zoe-pw").await.unwrap();
    assert_eq!(b.password(&id).await.unwrap(), "zoe-pw");

    b.lock().await;
    assert!(matches!(b.state(), BwState::Locked { .. }));
    assert!(!dir.join("appdata\\session").exists());

    let log = std::fs::read_to_string(dir.join("mock\\calls.log")).unwrap_or_default();
    for secret in ["correct horse", "example-pw", "zoe-pw"] {
        assert!(!log.contains(secret), "{secret} reached argv");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
