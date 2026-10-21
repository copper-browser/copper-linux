//! BotPass against a mock server: plain loopback HTTP and real self-signed
//! TLS (rcgen), on a tokio `TcpListener` of its own thread. Pairing writes a
//! private file; accounts, passwords and codes map through [`BotPass`];
//! 401 / 403 / 423 read as words; a wrong fingerprint is refused; no token
//! or secret shows up in `Debug` or in what the CLI prints.

use copper_botpass::cli::run_with;
use copper_botpass::{BotPass, BpState, Error, Server};
use copper_creds::credential::{AgentHint, CredentialId, Source};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const TOKEN: &str = "bpd_dev1_TOKENSECRETBODY";
const PASSWORD: &str = "pw-SECRET-123";
const CODE: &str = "424242";
const PAIR_CODE: &str = "GOOD-CODE";

struct Mock {
    url: String,
    fingerprint: Option<String>,
    locked: Arc<AtomicBool>,
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn host_of(origin: &str) -> String {
    url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

fn answer(
    locked: bool,
    method: &str,
    target: &str,
    auth: Option<&str>,
    body: &[u8],
) -> (u16, Value) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let authed = auth == Some(&format!("Bearer {TOKEN}"));
    match (method, path) {
        ("GET", "/v1/status") => (
            200,
            json!({"version": "mock", "setup": true, "locked": locked}),
        ),
        ("POST", "/v1/devices/pair") => {
            let req: Value = serde_json::from_slice(body).unwrap_or_default();
            if locked {
                return (423, json!({"error": "BotPass is locked"}));
            }
            if req["code"] != PAIR_CODE || req["kind"] != "copper" {
                return (401, json!({"error": "pairing code is wrong or expired"}));
            }
            (
                200,
                json!({
                    "device": {"id": "dev1", "name": req["name"], "kind": "copper", "bot_id": "b1"},
                    "token": TOKEN,
                    "bot": {"id": "b1", "name": "copper-bot"},
                }),
            )
        }
        ("GET", "/v1/fill/accounts") => {
            if locked {
                return (423, json!({"error": "BotPass is locked"}));
            }
            if !authed {
                return (401, json!({"error": "not signed in"}));
            }
            let origin = url::form_urlencoded::parse(query.as_bytes())
                .find(|(k, _)| k == "origin")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default();
            let host = host_of(&origin);
            let accounts = if host == "github.com" {
                json!([{"id": "it1", "title": "GitHub", "username": "octo", "sites": ["https://github.com"], "has_totp": true, "vault": "Agents"}])
            } else {
                json!([])
            };
            (200, json!({"origin": host, "accounts": accounts}))
        }
        ("POST", "/v1/fill") => {
            if locked {
                return (423, json!({"error": "BotPass is locked"}));
            }
            if !authed {
                return (401, json!({"error": "not signed in"}));
            }
            let req: Value = serde_json::from_slice(body).unwrap_or_default();
            let host = host_of(req["origin"].as_str().unwrap_or(""));
            if req["id"] != "it1" {
                return (404, json!({"error": "account not found"}));
            }
            if host != "github.com" {
                return (
                    403,
                    json!({"error": format!("destination {host} is not allowed for this credential")}),
                );
            }
            match req["what"].as_str() {
                Some("otp") => (
                    200,
                    json!({"id": "it1", "code": CODE, "code_expires_in": 17}),
                ),
                _ => (
                    200,
                    json!({"id": "it1", "username": "octo", "password": PASSWORD}),
                ),
            }
        }
        _ => (404, json!({"error": "no such endpoint"})),
    }
}

async fn serve_one<S: AsyncRead + AsyncWrite + Unpin>(mut s: S, locked: bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = match s.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.lines();
    let first: Vec<&str> = lines.next().unwrap_or("").split_whitespace().collect();
    let (method, target) = (
        first.first().copied().unwrap_or(""),
        first.get(1).copied().unwrap_or("/"),
    );
    let mut len = 0usize;
    let mut auth = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "authorization" => auth = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    while buf.len() < head_end + len {
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = &buf[head_end..(head_end + len).min(buf.len())];
    let (status, value) = answer(locked, method, target, auth.as_deref(), body);
    let text = value.to_string();
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = s.write_all(reply.as_bytes()).await;
    let _ = s.shutdown().await;
}

/// A mock BotPass on its own thread and runtime (so the CLI, which makes
/// its own runtime, can be run against it from the test thread).
fn start(tls: bool) -> Mock {
    let locked = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let flag = locked.clone();
    std::thread::spawn(move || {
        rt().block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let acceptor = if tls {
                let ck = rcgen::generate_simple_self_signed(vec![
                    "127.0.0.1".to_string(),
                    "localhost".to_string(),
                ])
                .unwrap();
                let der = ck.cert.der().clone();
                let fp = copper_cloud::tls::fingerprint(der.as_ref());
                let key = rustls::pki_types::PrivatePkcs8KeyDer::from(ck.key_pair.serialize_der());
                let config = rustls::ServerConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![der], key.into())
                .unwrap();
                tx.send((format!("https://127.0.0.1:{port}"), Some(fp)))
                    .unwrap();
                Some(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
            } else {
                tx.send((format!("http://127.0.0.1:{port}"), None)).unwrap();
                None
            };
            while let Ok((sock, _)) = listener.accept().await {
                let locked = flag.load(Ordering::SeqCst);
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    match acceptor {
                        Some(a) => {
                            if let Ok(s) = a.accept(sock).await {
                                serve_one(s, locked).await;
                            }
                        }
                        None => serve_one(sock, locked).await,
                    }
                });
            }
        });
    });
    let (url, fingerprint) = rx.recv().unwrap();
    Mock {
        url,
        fingerprint,
        locked,
    }
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn pairing_over_pinned_tls_writes_a_private_file() {
    let m = start(true);
    let dir = tempfile::tempdir().unwrap();
    let bp = BotPass::new(dir.path());
    let fp = m.fingerprint.clone().unwrap();
    let p = rt()
        .block_on(bp.pair(&m.url, Some(&fp.to_uppercase()), PAIR_CODE, "Copper test"))
        .unwrap();
    assert_eq!(p.token, TOKEN);
    assert_eq!(p.device_id, "dev1");
    assert_eq!(p.bot.as_deref(), Some("copper-bot"));
    assert_eq!(p.fingerprint.as_deref(), Some(fp.as_str()));
    assert!(!format!("{p:?}").contains("TOKENSECRET"));
    let file = bp.file();
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["token"], TOKEN);
    assert_eq!(saved["url"], m.url);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let snap = bp.snapshot();
    assert!(snap.paired());
    assert_eq!(snap.bot.as_deref(), Some("copper-bot"));
    assert!(!format!("{snap:?} {bp:?}").contains("TOKENSECRET"));
}

#[test]
fn accounts_passwords_and_codes_map_and_phishing_gets_nothing() {
    let m = start(true);
    let dir = tempfile::tempdir().unwrap();
    let bp = BotPass::new(dir.path());
    let rt = rt();
    rt.block_on(bp.pair(&m.url, m.fingerprint.as_deref(), PAIR_CODE, "c"))
        .unwrap();

    let rows = rt
        .block_on(bp.credentials_for("https://github.com"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    let c = &rows[0];
    assert_eq!(c.id, CredentialId::BotPass("it1".into()));
    assert_eq!(c.source, Source::BotPass);
    assert_eq!((c.host.as_str(), c.user.as_str()), ("github.com", "octo"));
    assert!(c.has_totp);
    assert_eq!(c.agent_hint, AgentHint::Allow);
    assert_eq!(bp.state(), BpState::Ready);

    assert_eq!(
        rt.block_on(bp.password("it1", "https://github.com"))
            .unwrap(),
        PASSWORD
    );
    assert_eq!(
        rt.block_on(bp.totp("it1", "https://github.com")).unwrap(),
        CODE
    );

    // A look-alike origin is offered nothing and given nothing.
    let evil = "https://github.com.evil.example";
    assert!(rt.block_on(bp.credentials_for(evil)).unwrap().is_empty());
    let denied = rt.block_on(bp.password("it1", evil)).unwrap_err();
    assert!(matches!(denied, Error::Denied(_)), "{denied:?}");
    assert!(denied.to_string().contains("not allowed"));
    assert!(!denied.to_string().contains(PASSWORD));
    assert!(matches!(
        rt.block_on(bp.password("nope", "https://github.com")),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn locked_and_revoked_read_as_words() {
    let m = start(false);
    let dir = tempfile::tempdir().unwrap();
    let bp = BotPass::new(dir.path());
    let rt = rt();
    // Plain http is fine on loopback (no pin).
    rt.block_on(bp.pair(&m.url, None, PAIR_CODE, "c")).unwrap();

    m.locked.store(true, Ordering::SeqCst);
    let e = rt
        .block_on(bp.credentials_for("https://github.com"))
        .unwrap_err();
    assert_eq!(e, Error::Locked);
    assert_eq!(e.to_string(), "BotPass is locked — run bp unlock");
    assert!(bp.snapshot().locked());
    assert_eq!(
        rt.block_on(bp.totp("it1", "https://github.com")),
        Err(Error::Locked)
    );
    m.locked.store(false, Ordering::SeqCst);
    rt.block_on(bp.refresh());
    assert_eq!(bp.state(), BpState::Ready);

    // A token BotPass no longer knows (the device was removed there).
    let mut p = bp.pairing().unwrap();
    p.token = "bpd_gone_x".into();
    copper_botpass::pairing::save(dir.path(), &p).unwrap();
    let bp = BotPass::new(dir.path());
    let e = rt
        .block_on(bp.password("it1", "https://github.com"))
        .unwrap_err();
    assert_eq!(e, Error::Unpaired);
    assert!(e.to_string().contains("bp connect copper"));
    assert_eq!(bp.state(), BpState::Revoked);

    // A wrong pairing code.
    let other = tempfile::tempdir().unwrap();
    let e = rt
        .block_on(BotPass::new(other.path()).pair(&m.url, None, "BAD0-CODE", "c"))
        .unwrap_err();
    assert!(matches!(e, Error::BadCode(_)), "{e:?}");
    assert!(!copper_botpass::pairing::path(other.path()).exists());
}

#[test]
fn a_wrong_fingerprint_is_refused_and_nothing_is_kept() {
    let m = start(true);
    let rt = rt();
    let real = m.fingerprint.clone().unwrap();
    let wrong = "00".repeat(32);
    let e = rt
        .block_on(Server::new(&m.url, Some(&wrong)).unwrap().status())
        .unwrap_err();
    assert_eq!(
        e,
        Error::Fingerprint {
            seen: Some(real.clone())
        }
    );
    let dir = tempfile::tempdir().unwrap();
    let e = rt
        .block_on(BotPass::new(dir.path()).pair(&m.url, Some(&wrong), PAIR_CODE, "c"))
        .unwrap_err();
    assert!(matches!(e, Error::Fingerprint { .. }), "{e:?}");
    assert!(!copper_botpass::pairing::path(dir.path()).exists());

    // No pin: the public authorities don't know a self-signed certificate,
    // and what it showed is reported for comparison.
    let e = rt
        .block_on(Server::new(&m.url, None).unwrap().status())
        .unwrap_err();
    assert_eq!(e, Error::Untrusted { seen: Some(real) });
}

#[test]
fn the_cli_pairs_reports_and_unpairs_without_printing_the_token() {
    let m = start(true);
    let dir = tempfile::tempdir().unwrap();
    let fp = m.fingerprint.clone().unwrap();
    let run = |a: Vec<String>| {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run_with(&a, dir.path(), &mut out, &mut err);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        );
        assert!(!text.contains("TOKENSECRET"), "{text}");
        (code, String::from_utf8_lossy(&out).to_string(), text)
    };

    let (code, _, text) = run(args(&[
        "botpass",
        "pair",
        "--server",
        &m.url,
        "--code",
        "BAD0-CODE",
        "--fingerprint",
        &fp,
    ]));
    assert_eq!(code, 1, "{text}");

    let (code, out, text) = run(args(&[
        "botpass",
        "pair",
        "--server",
        &m.url,
        "--fingerprint",
        &fp,
        "--code",
        PAIR_CODE,
        "--name",
        "Copper on test",
        "--json",
    ]));
    assert_eq!(code, 0, "{text}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["ok"], json!(true));
    assert_eq!(v["device_name"], "Copper on test");
    assert_eq!(v["bot"], "copper-bot");
    assert!(v["path"].as_str().unwrap().ends_with("botpass.json"));

    let (code, out, text) = run(args(&["botpass", "status", "--json"]));
    assert_eq!(code, 0, "{text}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["state"], "unlocked");

    m.locked.store(true, Ordering::SeqCst);
    let (code, out, _) = run(args(&["botpass", "status"]));
    assert_eq!(code, 0);
    assert!(out.contains("locked") && out.contains("bp unlock"), "{out}");

    let (code, out, _) = run(args(&["botpass", "unpair"]));
    assert_eq!(code, 0);
    assert!(out.contains("dev1"), "{out}");
    assert!(!copper_botpass::pairing::path(dir.path()).exists());
    let (code, _, _) = run(args(&["botpass", "status"]));
    assert_eq!(code, 1);
}
