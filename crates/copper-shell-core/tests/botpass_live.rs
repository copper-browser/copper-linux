//! End to end against a real BotPass server, through the forms lane's
//! [`Vault`]: pair with `copper botpass pair` (the CLI `bp connect copper`
//! runs), then list, fill a password and a one-time code for the granted
//! origin, and show a look-alike origin gets nothing. Ignored by default;
//! run it with a BotPass server and a fresh pairing code:
//!
//! ```text
//! BOTPASS_E2E_URL=https://127.0.0.1:17491 BOTPASS_E2E_FINGERPRINT=<hex> \
//! BOTPASS_E2E_CODE=ABCD-EFGH BOTPASS_E2E_ORIGIN=https://github.com \
//! BOTPASS_E2E_USER=octo BOTPASS_E2E_PASSWORD=<the item's password> \
//!   cargo test -p copper-shell-core --test botpass_live -- --ignored --nocapture
//! ```

use copper_creds::credential::{CredentialId, Source};
use copper_shell_core::forms::vault::{Keyring, Vault};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set (see the file's docs)"))
}

/// No Secret Service in this test: the keyring is never opened.
fn no_keyring(_start: bool) -> Keyring {
    Keyring::Unavailable("not in this test".into())
}

#[test]
#[ignore = "needs a running BotPass server (BOTPASS_E2E_*)"]
fn pair_list_fill_and_refuse_phishing() {
    let (url, code, origin) = (
        env("BOTPASS_E2E_URL"),
        env("BOTPASS_E2E_CODE"),
        env("BOTPASS_E2E_ORIGIN"),
    );
    let fingerprint = std::env::var("BOTPASS_E2E_FINGERPRINT").unwrap_or_default();
    let user = env("BOTPASS_E2E_USER");
    let expected_password = env("BOTPASS_E2E_PASSWORD");
    let data = tempfile::tempdir().unwrap();

    // 1. Pair the way `bp connect copper` does.
    let mut args: Vec<String> = [
        "botpass", "pair", "--server", &url, "--code", &code, "--json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if !fingerprint.is_empty() {
        args.extend(["--fingerprint".to_string(), fingerprint.clone()]);
    }
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let exit = copper_botpass::cli::run_with(&args, data.path(), &mut out, &mut err);
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(&err)
    );
    println!("copper botpass pair → exit {exit}\n{printed}");
    assert_eq!(exit, 0);
    assert!(!printed.contains("bpd_"), "the token was printed");
    let status: Vec<String> = ["botpass", "status", "--json"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let exit = copper_botpass::cli::run_with(&status, data.path(), &mut out, &mut err);
    let st: serde_json::Value = serde_json::from_slice(&out).unwrap();
    println!("copper botpass status → exit {exit}, state {}", st["state"]);
    assert_eq!((exit, st["state"].as_str()), (0, Some("unlocked")));
    assert!(!String::from_utf8_lossy(&out).contains("bpd_"));
    let file = data.path().join("botpass.json");
    assert!(file.is_file(), "pairing file missing: {}", file.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        println!("{} is {mode:o}", file.display());
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let vault = Vault::new(data.path(), false, false, no_keyring);
    let host = url::Url::parse(&origin)
        .unwrap()
        .host_str()
        .unwrap()
        .to_string();

    rt.block_on(async {
        // 2. The accounts the bot may use here: usernames only.
        let rows = vault.candidates(&host, Some(&origin)).await;
        let bp: Vec<_> = rows
            .iter()
            .filter(|c| c.source == Source::BotPass)
            .collect();
        println!(
            "candidates on {origin}: {:?}",
            bp.iter()
                .map(|c| (c.id.to_string(), c.user.clone(), c.has_totp))
                .collect::<Vec<_>>()
        );
        let c = bp
            .iter()
            .find(|c| c.user == user)
            .expect("the granted account is offered");
        assert!(matches!(c.id, CredentialId::BotPass(_)));

        // 3. Its password and one-time code, fetched for this origin now.
        let password = vault.secret_at(&c.id, Some(&origin)).await.unwrap();
        assert_eq!(password, expected_password);
        println!(
            "password for {}: <{} chars, matches>",
            c.user,
            password.len()
        );
        if c.has_totp {
            let code = vault.totp_at(&c.id, Some(&origin)).await.unwrap();
            assert!(code.len() >= 6 && code.chars().all(|ch| ch.is_ascii_digit()));
            println!("one-time code: <{} digits>", code.len());
        }
        // Without an origin BotPass gives nothing.
        assert!(vault.secret(&c.id).await.is_err());

        // 4. A phishing origin is offered nothing and given nothing.
        let evil = format!("https://{host}.evil.example");
        let offered = vault
            .candidates(&format!("{host}.evil.example"), Some(&evil))
            .await;
        assert!(offered.iter().all(|c| c.source != Source::BotPass));
        let refused = vault.secret_at(&c.id, Some(&evil)).await.unwrap_err();
        println!(
            "on {evil}: offered {} BotPass accounts; fill → {refused}",
            offered.len()
        );
        assert!(!refused.contains(&expected_password));

        println!("BotPass in the snapshot: {:?}", vault.snapshot().bp);

        // 5. Optional: lock BotPass (an owner session) and see the words an
        // agent gets, then unlock it again (the master password).
        if let (Ok(session), Ok(master)) = (
            std::env::var("BOTPASS_E2E_SESSION"),
            std::env::var("BOTPASS_E2E_MASTER"),
        ) {
            let trust = copper_cloud::tls::Trust::new(
                Some(&fingerprint)
                    .filter(|f| !f.is_empty())
                    .map(String::as_str),
            );
            let http = trust.http_client();
            let base = url.trim_end_matches('/');
            let r = http
                .post(format!("{base}/v1/lock"))
                .bearer_auth(&session)
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success(), "lock: {}", r.status());
            let locked = vault.secret_at(&c.id, Some(&origin)).await.unwrap_err();
            println!("locked: fill → {locked}");
            assert_eq!(locked, "BotPass is locked — run bp unlock");
            assert!(vault.snapshot().bp_locked());
            let r = http
                .post(format!("{base}/v1/unlock"))
                .json(&serde_json::json!({"password": master}))
                .send()
                .await
                .unwrap();
            assert!(r.status().is_success(), "unlock: {}", r.status());
            assert_eq!(
                vault.secret_at(&c.id, Some(&origin)).await.unwrap(),
                expected_password
            );
            println!("unlocked again: fill works");
        }
    });
}
