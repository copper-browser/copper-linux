//! Single sign-on in the shell, against a fake instance on loopback: the
//! phases a page draws, the outcome, sync turned on through the window's own
//! engine, refusals in words, Cancel, and what `copper://cloud/join` does.

use super::*;
use crate::cloud::{Cloud, runtime};
use copper_cloud::sync::adapter::{MemoryBrowser, MemoryState};
use copper_cloud::{CanvasApi, CloudClient, CloudIntelligence, Domain, SyncEngine, parse_sso_link};
use copper_core::{Paths, Store};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// MARK: pure

#[test]
fn hints_and_instances() {
    assert_eq!(
        domain_hint(&["exowatt.com".into()]).as_deref(),
        Some("Use your @exowatt.com account")
    );
    assert_eq!(
        domain_hint(&["a.com".into(), "@B.com".into(), "c.com".into()]).as_deref(),
        Some("Use your @a.com, @b.com or @c.com account")
    );
    assert_eq!(domain_hint(&[" ".into()]), None);
    let a = parse_sso_link("copper-cloud://Copper.Dev.Exowatt.com/#sso=google").unwrap();
    let b = parse_sso_link("copper-cloud://copper.dev.exowatt.com:443/#sso=okta").unwrap();
    let c = parse_sso_link("copper-cloud://copper.dev.exowatt.com:8443/#sso=google").unwrap();
    assert!(same_instance(&a.endpoint, &b.endpoint));
    assert!(!same_instance(&a.endpoint, &c.endpoint));
    assert!(SsoTarget::Join(a.clone()).reaches(&b.endpoint));
}

#[test]
fn what_the_page_draws() {
    assert_eq!(
        SsoPhase::Waiting("Google".into()).waiting_text(),
        Some("Finish signing in in your browser…")
    );
    assert!(SsoPhase::Starting.cancellable());
    assert!(!SsoPhase::Finishing.cancellable());
    assert!(SsoPhase::Finishing.busy());
    assert!(!SsoPhase::Idle.busy());
    let link = parse_sso_link("copper-cloud://h.test:8443/#sso=google").unwrap();
    let elsewhere = parse_sso_link("copper-cloud://other.test/#sso=google").unwrap();
    let mut s = SsoState {
        phase: SsoPhase::Failed {
            code: "sso_domain".into(),
            message: "Use your @exowatt.com account".into(),
        },
        target: Some(SsoTarget::Join(link.clone())),
        ..Default::default()
    };
    assert_eq!(
        s.problem_for(&link.endpoint),
        Some("Use your @exowatt.com account")
    );
    assert_eq!(s.problem_for(&elsewhere.endpoint), None);
    assert!(!s.running_for(&link.endpoint));
    // A certificate to compare is a card, not a problem line.
    s.phase = SsoPhase::Failed {
        code: "untrusted".into(),
        message: "compare it".into(),
    };
    s.seen = Some("ab".repeat(32));
    assert_eq!(s.problem_for(&link.endpoint), None);
    assert_eq!(s.seen_for(&link.endpoint), Some("ab".repeat(32).as_str()));
    s.phase = SsoPhase::Waiting("Google".into());
    assert!(s.running_for(&link.endpoint));
    assert!(!s.running_for(&elsewhere.endpoint));
    assert_eq!(
        SsoDone {
            host: "h.test:8443".into(),
            email: "a@b.c".into(),
            provider: "Google".into(),
            created: true
        }
        .text(),
        "Signed in to h.test:8443 as a@b.c (new account)"
    );
}

// MARK: against a fake instance

/// A copper-cloud that knows `/healthz`, `GET /v1/auth/sso`,
/// `POST /v1/auth/sso/google` and `/v1/auth/me`, on plain loopback HTTP.
/// `old`: it predates single sign-on.
async fn fake_instance(old: bool) -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                continue;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, mut body) = loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (
                            String::from_utf8_lossy(&buf[..at]).to_string(),
                            buf[at + 4..].to_vec(),
                        );
                    }
                };
                let length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while body.len() < length {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                let line = head.lines().next().unwrap_or("").to_string();
                let (status, answer) = route(&line, &body, port, old);
                let text = answer.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

fn user() -> Value {
    json!({"id": "7d1c3a8e-0000-4000-8000-000000000001", "email": "farce@exowatt.com",
           "display_name": "Felipe Arce", "created_at": "2026-10-19T00:00:00Z"})
}

fn route(line: &str, body: &[u8], port: u16, old: bool) -> (u16, Value) {
    if line.starts_with("GET /healthz ") {
        return (200, json!({"ok": true}));
    }
    if line.starts_with("GET /v1/auth/sso ") {
        if old {
            return (404, json!({"error": "not_found", "message": "Not Found"}));
        }
        return (
            200,
            json!({"enabled": true, "password_signin": false, "providers": [{
                "id": "google", "kind": "google", "name": "Google Workspace",
                "client_id": "123-abc.apps.googleusercontent.com",
                "authorization_endpoint": format!("http://127.0.0.1:{port}/authorize"),
                "scopes": ["openid", "email", "profile"],
                "domains": ["exowatt.com"], "hd": "exowatt.com"}]}),
        );
    }
    if line.starts_with("GET /v1/auth/me ") {
        return (
            200,
            json!({"user": user(), "device": {"id": "0b6e8f8a-1111-4111-8111-111111111111", "name": "Test box"}}),
        );
    }
    if line.starts_with("POST /v1/auth/sso/google ") {
        let v: Value = serde_json::from_slice(body).unwrap_or_default();
        return match v["code"].as_str() {
            Some("good-code") => (
                200,
                json!({
                    "token": "tok_secret_sso",
                    "user": user(),
                    "device": {"id": "0b6e8f8a-1111-4111-8111-111111111111", "name": "Test box"},
                    "gate_key": "ck_minted_for_sso",
                    "created": false
                }),
            ),
            Some("gmail-code") => (
                403,
                json!({"error": "sso_domain", "message": "Only exowatt.com accounts"}),
            ),
            _ => (401, json!({"error": "sso_failed", "message": "bad code"})),
        };
    }
    (404, json!({"error": "not_found", "message": "Not Found"}))
}

/// A cloud of its own (not the app's), on a throwaway profile.
fn cloud(dir: &std::path::Path) -> &'static Cloud {
    let store = Store::new(Paths::new(dir.join("data"), dir.join("config")));
    let client = CloudClient::open(store).unwrap();
    let browser: Arc<dyn copper_cloud::BrowserState> =
        Arc::new(MemoryBrowser::new(MemoryState::default()));
    let _rt = runtime().enter();
    Box::leak(Box::new(Cloud {
        sync: SyncEngine::new(client.clone(), browser),
        intelligence: CloudIntelligence::new(client.clone()),
        canvas: CanvasApi::new(client.clone()),
        client,
        started: AtomicBool::new(false),
    }))
}

fn center() -> &'static SignIn {
    Box::leak(Box::new(SignIn::new()))
}

/// The system browser, played: after `delay`, the provider "redirects"
/// back to the loopback with `code` (`None`: never comes back).
fn browser(code: Option<&'static str>, delay: Duration) -> (Opener, Arc<Mutex<Option<String>>>) {
    let opened = Arc::new(Mutex::new(None::<String>));
    let seen = opened.clone();
    let open: Opener = Arc::new(move |url: &str| {
        *seen.lock().unwrap() = Some(url.to_string());
        let Some(code) = code else { return Ok(()) };
        let u = url::Url::parse(url).map_err(|e| e.to_string())?;
        let q: BTreeMap<String, String> = u.query_pairs().into_owned().collect();
        let redirect = q["redirect_uri"].clone();
        let state = q["state"].clone();
        runtime().spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = fetch(&format!("{redirect}?code={code}&state={state}")).await;
        });
        Ok(())
    });
    (open, opened)
}

/// A bare GET over a TCP socket (the test needs no HTTP client of its own).
async fn fetch(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let mut s = tokio::net::TcpStream::connect((u.host_str()?, u.port()?))
        .await
        .ok()?;
    let target = match u.query() {
        Some(q) => format!("{}?{q}", u.path()),
        None => u.path().to_string(),
    };
    s.write_all(
        format!("GET {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).await.ok()?;
    Some(out)
}

/// Wait (at most 10 s) until the state says so.
fn wait_for(rx: &mut watch::Receiver<SsoState>, what: impl Fn(&SsoState) -> bool) -> SsoState {
    runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|s| what(s)))
            .await
            .expect("in time")
            .expect("open")
            .clone()
    })
}

fn link(port: u16) -> SsoLink {
    parse_sso_link(&format!("http://127.0.0.1:{port}/#sso=google")).unwrap()
}

#[test]
fn signs_in_and_turns_sync_on_through_the_window_engine() {
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let cloud = cloud(tmp.path());
    let me = center();
    let mut rx = me.watch();
    // The link that brought it, kept for the page until it is done.
    let offer = me.offer(&link(port).link(), copper_cloud::JoinCode::Sso(link(port)));
    assert_eq!(offer.host(), format!("127.0.0.1:{port}"));
    let (open, opened) = browser(Some("good-code"), Duration::from_millis(300));
    let run = me.start(cloud, SsoTarget::Join(link(port)), None, open);
    assert_eq!(me.state().phase, SsoPhase::Starting);
    let waiting = wait_for(&mut rx, |s| matches!(s.phase, SsoPhase::Waiting(_)));
    // The name the instance gives the provider, learned on the way.
    assert_eq!(waiting.phase, SsoPhase::Waiting("Google Workspace".into()));
    assert!(waiting.running_for(&link(port).endpoint));
    assert!(waiting.discovered > 0);
    assert_eq!(
        me.provider_name(&link(port).endpoint, "google"),
        "Google Workspace"
    );
    let outcome = runtime()
        .block_on(run.done)
        .expect("joined")
        .expect("signed in");
    assert_eq!(outcome.account.email, "farce@exowatt.com");
    let done = me.state();
    assert_eq!(done.phase, SsoPhase::Idle);
    assert_eq!(
        done.signed_in.as_ref().map(SsoDone::text).as_deref(),
        Some(format!("Signed in to 127.0.0.1:{port} as farce@exowatt.com").as_str())
    );
    assert_eq!(done.offer, None, "the link has done its work");
    assert!(
        opened
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|u| u.contains("/authorize?") && u.contains("hd=exowatt.com"))
    );
    // Kept like a pairing, and sync on with every switch, by this engine.
    assert!(cloud.client.is_signed_in());
    assert_eq!(cloud.client.link().unwrap().key, "ck_minted_for_sso");
    assert!(cloud.is_started());
    assert!(cloud.sync.is_on());
    assert_eq!(
        cloud.sync.enabled(),
        Domain::ALL
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
    );
}

#[test]
fn a_refusal_is_shown_in_words_and_nothing_is_kept() {
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let cloud = cloud(tmp.path());
    let me = center();
    let (open, _) = browser(Some("gmail-code"), Duration::from_millis(10));
    let run = me.start(cloud, SsoTarget::Join(link(port)), None, open);
    let err = runtime().block_on(run.done).unwrap().unwrap_err();
    assert_eq!(err.code, "sso_domain");
    let s = me.state();
    assert_eq!(
        s.problem_for(&link(port).endpoint),
        Some("Use your @exowatt.com account")
    );
    assert!(!cloud.client.is_linked());
    assert!(!cloud.sync.is_on());
    // Editing the code clears it.
    me.clear_problem();
    assert_eq!(me.state().phase, SsoPhase::Idle);
}

#[test]
fn cancel_stops_waiting() {
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let cloud = cloud(tmp.path());
    let me = center();
    let mut rx = me.watch();
    let (open, _) = browser(None, Duration::ZERO);
    let run = me.start(cloud, SsoTarget::Join(link(port)), None, open);
    wait_for(&mut rx, |s| matches!(s.phase, SsoPhase::Waiting(_)));
    me.cancel();
    assert_eq!(me.state().phase, SsoPhase::Idle);
    let err = runtime()
        .block_on(async { tokio::time::timeout(Duration::from_secs(10), run.done).await })
        .expect("stopped promptly")
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, "sso_cancelled");
    assert_eq!(me.state().phase, SsoPhase::Idle, "no problem shown");
    assert!(!cloud.client.is_signed_in());
}

#[test]
fn the_opener_failing_is_a_problem_in_words() {
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let cloud = cloud(tmp.path());
    let me = center();
    let open: Opener = Arc::new(|_: &str| Err("no display".into()));
    let run = me.start(cloud, SsoTarget::Join(link(port)), None, open);
    let err = runtime().block_on(run.done).unwrap().unwrap_err();
    assert_eq!(err.code, "browser");
    assert!(
        me.state()
            .problem_for(&link(port).endpoint)
            .is_some_and(|p| p.contains("couldn't open the browser"))
    );
}

#[test]
fn a_second_start_takes_over() {
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let cloud = cloud(tmp.path());
    let me = center();
    let mut rx = me.watch();
    let (never, _) = browser(None, Duration::ZERO);
    let first = me.start(cloud, SsoTarget::Join(link(port)), None, never);
    wait_for(&mut rx, |s| matches!(s.phase, SsoPhase::Waiting(_)));
    let (open, _) = browser(Some("good-code"), Duration::from_millis(50));
    let second = me.start(cloud, SsoTarget::Join(link(port)), None, open);
    let err = runtime().block_on(first.done).unwrap().unwrap_err();
    assert_eq!(err.code, "sso_cancelled");
    runtime().block_on(second.done).unwrap().expect("signed in");
    assert_eq!(me.state().phase, SsoPhase::Idle);
    assert!(me.state().signed_in.is_some());
}

#[test]
fn the_account_step_signs_in_on_the_linked_instance() {
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let cloud = cloud(tmp.path());
    // Linked by a link code first (Connect), not signed in.
    let code = format!("http://127.0.0.1:{port}/#k=ck_instance_key");
    let parsed = copper_cloud::parse_link_code(&code).unwrap();
    runtime().block_on(async {
        // The fake answers /v1/auth/me (gated) with 200: connect takes the key.
        cloud.client.connect(parsed.clone(), None).await.unwrap();
    });
    assert!(cloud.client.is_linked() && !cloud.client.is_signed_in());
    let me = center();
    let info = runtime()
        .block_on(me.discover(&parsed.endpoint, None, true))
        .expect("offers");
    assert!(!info.password_signin);
    assert_eq!(
        me.first_provider(&parsed.endpoint).map(|p| p.button()),
        Some("Continue with Google Workspace".into())
    );
    let (open, _) = browser(Some("good-code"), Duration::from_millis(20));
    let run = me.start(
        cloud,
        SsoTarget::Linked {
            endpoint: parsed.endpoint.clone(),
            provider: "google".into(),
        },
        None,
        open,
    );
    runtime().block_on(run.done).unwrap().expect("signed in");
    assert!(cloud.client.is_signed_in());
    assert!(cloud.sync.is_on());
}

#[test]
fn an_old_instance_offers_nothing() {
    let port = runtime().block_on(fake_instance(true));
    let me = center();
    let end = link(port).endpoint;
    assert_eq!(runtime().block_on(me.discover(&end, None, false)), None);
    assert_eq!(me.known(&end), None);
    assert_eq!(me.provider_name(&end, "google"), "Google");
    assert_eq!(me.first_provider(&end), None);
}

#[test]
fn a_join_link_opened_while_running_starts_the_sign_in() {
    // The app's own cloud and sign-in (one test only sets them).
    let port = runtime().block_on(fake_instance(false));
    let tmp = tempfile::tempdir().unwrap();
    let mine = cloud(tmp.path());
    let _ = crate::cloud::CLOUD.set(Cloud {
        client: mine.client.clone(),
        sync: mine.sync.clone(),
        intelligence: mine.intelligence.clone(),
        canvas: mine.canvas.clone(),
        started: AtomicBool::new(false),
    });
    let app = crate::cloud::get().unwrap();
    let mut rx = sign_in().watch();
    let deep = format!(
        "copper://cloud/join?code={}",
        url::form_urlencoded::byte_serialize(link(port).link().as_bytes()).collect::<String>()
    );
    let (open, _) = browser(Some("good-code"), Duration::from_millis(200));
    let action = crate::cloud::join::handle(&deep, open);
    let crate::cloud::join::JoinAction::StartSso { offer, target } = &action else {
        panic!("{action:?}")
    };
    assert_eq!(
        offer.as_ref().map(|o| o.text.clone()),
        Some(link(port).link())
    );
    assert_eq!(target, &SsoTarget::Join(link(port)));
    assert!(action.opens_settings());
    let state = wait_for(&mut rx, |s| s.signed_in.is_some());
    assert_eq!(state.offer, None);
    assert!(app.client.is_signed_in());
    assert!(app.sync.is_on());
    // Again, now signed in there: it says so and starts nothing.
    let (open, opened) = browser(Some("good-code"), Duration::ZERO);
    let again = crate::cloud::join::handle(&deep, open);
    assert_eq!(
        again.message(),
        Some(format!(
            "Already signed in to 127.0.0.1:{port} as farce@exowatt.com"
        ))
    );
    assert!(opened.lock().unwrap().is_none());
    assert_eq!(sign_in().state().phase, SsoPhase::Idle);
}

#[test]
fn a_timeout_says_so() {
    let me = center();
    let target = SsoTarget::Join(parse_sso_link("copper-cloud://h.test/#sso=google").unwrap());
    *me.current.lock().unwrap() = Some((7, SsoCancel::new()));
    me.state.send_modify(|s| {
        s.phase = SsoPhase::Waiting("Google".into());
        s.target = Some(target.clone());
    });
    // What copper-cloud's flow answers after five minutes.
    let late = Failure::new(
        0,
        "sso_timeout",
        "Nothing came back from the browser in 5 minutes — try again",
    );
    me.finish(7, &target, &Err(late));
    assert_eq!(
        me.state().problem_for(target.endpoint()),
        Some("Nothing came back from the browser in 5 minutes — try again")
    );
    // A stale attempt's end changes nothing.
    me.finish(6, &target, &Err(Failure::new(0, "network", "gone")));
    assert!(matches!(
        me.state().phase,
        SsoPhase::Failed { ref code, .. } if code == "sso_timeout"
    ));
}
