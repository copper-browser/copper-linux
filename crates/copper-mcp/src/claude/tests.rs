//! The account against a mock of claude.ai's token, profile and Messages
//! endpoints on loopback: the authorize address, the callback and the
//! exchange, the paste fallback, refresh (once for many askers, signed out
//! when turned away) and the Messages call with its headers.

use super::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Mutex;

/// One request the mock saw.
#[derive(Clone, Debug)]
struct Seen {
    path: String,
    headers: HashMap<String, String>,
    body: Value,
}

/// Answers by path from a script, one `(status, body)` per request; past
/// the end it repeats the last answer.
struct Mock {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn start(script: Vec<(&str, u16, Value)>) -> Mock {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let mut answers: HashMap<String, Vec<(u16, Value)>> = HashMap::new();
        for (path, status, body) in script {
            answers.entry(path.into()).or_default().push((status, body));
        }
        std::thread::spawn(move || {
            let mut n: HashMap<String, usize> = HashMap::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).unwrap_or(0) == 0 {
                    continue;
                }
                let path = first.split(' ').nth(1).unwrap_or("").to_string();
                let mut headers = HashMap::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                    }
                }
                let length = headers
                    .get("content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                log.lock().unwrap().push(Seen {
                    path: path.clone(),
                    headers,
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                });
                let (status, answer) = match answers.get(&path) {
                    Some(list) => {
                        let i = n.entry(path.clone()).or_default();
                        let a = list[(*i).min(list.len() - 1)].clone();
                        *i += 1;
                        a
                    }
                    None => (404, json!({})),
                };
                let text = answer.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        Mock { base, seen }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            authorize: AUTHORIZE_URL.into(),
            token: format!("{}/v1/oauth/token", self.base),
            profile: format!("{}/api/oauth/profile", self.base),
            messages: format!("{}/v1/messages", self.base),
        }
    }

    fn seen(&self, path: &str) -> Vec<Seen> {
        let all = self.seen.lock().unwrap();
        all.iter().filter(|s| s.path == path).cloned().collect()
    }
}

fn token(access: &str, refresh: &str) -> Value {
    json!({ "access_token": access, "refresh_token": refresh, "expires_in": 3600 })
}

fn profile() -> Value {
    json!({
        "account": { "email": "nick@example.com", "display_name": "Nick", "full_name": "Nick N" },
        "organization": { "name": "Exowatt" },
    })
}

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    rt: tokio::runtime::Runtime,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(copper_core::Paths::new(dir.path(), dir.path()));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    World {
        _dir: dir,
        store,
        rt,
    }
}

impl World {
    fn account(&self, mock: &Mock) -> Arc<Account> {
        Account::with_endpoints(
            self.store.clone(),
            self.rt.handle().clone(),
            mock.endpoints(),
        )
    }

    /// Wait (up to 5 s) for the status to say so.
    fn until(&self, account: &Account, ok: impl Fn(&Status) -> bool) -> Status {
        let mut rx = account.watch();
        self.rt
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|s| ok(s)))
                    .await
                    .map(|s| s.map(|s| s.clone()))
            })
            .expect("in time")
            .expect("the account lives")
    }

    /// A credential already on disk, running out in `seconds`.
    fn signed_in(&self, seconds: f64) {
        let c = ClaudeCredential {
            access: "old-access".into(),
            refresh: "old-refresh".into(),
            expires: SwiftDate::from_unix(now_unix() + seconds),
            email: Some("nick@example.com".into()),
            ..ClaudeCredential::default()
        };
        self.store.save(&c).unwrap();
    }
}

fn query(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// GET `target` on the loopback; the status and body.
fn get(port: u16, target: &str) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split(' ').nth(1).unwrap_or("0").parse().unwrap_or(0);
    (status, out)
}

fn port_of(redirect: &str) -> u16 {
    url::Url::parse(redirect).unwrap().port().unwrap()
}

#[test]
fn signing_in_reaches_the_authorize_page_and_comes_back_signed_in() {
    let w = world();
    let mock = Mock::start(vec![
        ("/v1/oauth/token", 200, token("new-access", "new-refresh")),
        ("/api/oauth/profile", 200, profile()),
    ]);
    let account = w.account(&mock);
    assert!(!account.signed_in());

    let url = account.sign_in().unwrap();
    account.opened("tab-1".into());
    assert!(
        url.starts_with("https://claude.ai/oauth/authorize?"),
        "{url}"
    );
    let q = query(&url);
    assert_eq!(q["code"], "true");
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["scope"], SCOPES);
    assert_eq!(q["code_challenge_method"], "S256");
    let verifier = &q["state"];
    assert_eq!(verifier.len(), 43);
    assert_eq!(
        q["code_challenge"],
        copper_cloud::sso::challenge_for(verifier)
    );
    let redirect = &q["redirect_uri"];
    assert!(redirect.starts_with("http://localhost:") && redirect.ends_with("/callback"));
    assert_eq!(account.status().phase, Phase::Waiting);

    // Anything but the callback is nothing; a stranger's state is refused.
    let port = port_of(redirect);
    assert_eq!(get(port, "/favicon.ico").0, 404);
    let (status, page) = get(port, "/callback?code=abc&state=someone-else");
    assert_eq!(status, 400);
    assert!(page.contains("Copper could not sign in."));
    assert_eq!(
        account.status().phase,
        Phase::Failed("The Claude sign-in callback was not valid".into())
    );
    assert!(
        account.take_closing().is_empty(),
        "a failed tab is the person's"
    );

    // The code claude.ai showed can still be pasted: the verifier stayed.
    account.paste(&format!("pasted-code#{verifier}"));
    let st = w.until(&account, |s| s.signed_in);
    assert_eq!(st.phase, Phase::Idle);
    assert_eq!(st.who(), "Nick · nick@example.com");
    assert_eq!(st.organization, "Exowatt");
    assert_eq!(st.session, 1);

    let sent = &mock.seen("/v1/oauth/token")[0].body;
    assert_eq!(sent["grant_type"], "authorization_code");
    assert_eq!(sent["client_id"], CLIENT_ID);
    assert_eq!(sent["code"], "pasted-code");
    assert_eq!(&sent["state"], verifier.as_str());
    assert_eq!(&sent["code_verifier"], verifier.as_str());
    assert_eq!(&sent["redirect_uri"], redirect.as_str());
    let asked = &mock.seen("/api/oauth/profile")[0].headers;
    assert_eq!(asked["authorization"], "Bearer new-access");
    assert_eq!(asked["anthropic-beta"], "oauth-2025-04-20");

    // The tokens are on disk (0600), the lane is the account's, and no
    // status says a token.
    let saved = w
        .store
        .load_opt::<ClaudeCredential>()
        .unwrap()
        .value
        .unwrap();
    assert_eq!(saved.access, "new-access");
    assert_eq!(saved.refresh, "new-refresh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(w.store.path(DataFile::Claude))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let lane = w.store.load::<IntelligenceKeys>().unwrap().value.lane;
    assert_eq!(lane.as_deref(), Some("claude"));
    let shown = st.json().to_string();
    assert!(!shown.contains("new-access") && !shown.contains("new-refresh"));
    assert_eq!(st.json()["signedIn"], true);
    assert_eq!(st.json()["phase"], "idle");

    // Signing out forgets it.
    account.sign_out();
    assert!(!account.signed_in());
    assert!(!w.store.path(DataFile::Claude).exists());
    assert_eq!(account.status().session, 2);
}

#[test]
fn the_callback_signs_in_and_hands_its_tab_back() {
    let w = world();
    let mock = Mock::start(vec![
        ("/v1/oauth/token", 200, token("a", "r")),
        ("/api/oauth/profile", 500, json!({})),
    ]);
    let account = w.account(&mock);
    let q = query(&account.sign_in().unwrap());
    account.opened("tab-7".into());
    let port = port_of(&q["redirect_uri"]);
    let (status, page) = get(port, &format!("/callback?code=c0de&state={}", q["state"]));
    assert_eq!(status, 200);
    assert!(page.contains("Copper is signed in with your Claude account."));
    let st = w.until(&account, |s| s.signed_in);
    assert_eq!(st.who(), "Signed in", "no profile: still signed in");
    assert_eq!(account.take_closing(), vec!["tab-7".to_string()]);
    assert_eq!(mock.seen("/v1/oauth/token")[0].body["code"], "c0de");
    // The listener is gone with the attempt.
    std::thread::sleep(Duration::from_millis(100));
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
}

#[test]
fn a_refused_exchange_fails_and_cancel_closes_the_tab() {
    let w = world();
    let mock = Mock::start(vec![(
        "/v1/oauth/token",
        400,
        json!({"error": "invalid_grant", "error_description": "Invalid code"}),
    )]);
    let account = w.account(&mock);
    account.sign_in().unwrap();
    account.opened("tab-1".into());
    account.cancel();
    assert_eq!(account.status().phase, Phase::Idle);
    assert_eq!(account.take_closing(), vec!["tab-1".to_string()]);

    // A paste with no attempt goes to the fallback address.
    account.paste("  just-a-code \n");
    let st = w.until(&account, |s| matches!(s.phase, Phase::Failed(_)));
    assert_eq!(st.phase.trouble(), "Claude sign-in failed: Invalid code");
    let sent = &mock.seen("/v1/oauth/token")[0].body;
    assert_eq!(sent["code"], "just-a-code");
    assert_eq!(sent["redirect_uri"], FALLBACK_REDIRECT);

    account.paste("");
    assert_eq!(
        account.status().phase.trouble(),
        "That does not contain a Claude sign-in code"
    );
    account.sign_in().unwrap();
    account.paste("http://localhost:1/callback?code=x&state=not-this-attempt");
    assert_eq!(
        account.status().phase.trouble(),
        "That code is from a different sign-in attempt"
    );
}

#[test]
fn tokens_refresh_once_for_many_and_a_refusal_signs_out() {
    let w = world();
    w.signed_in(60.0);
    let mock = Mock::start(vec![
        (
            "/v1/oauth/token",
            200,
            token("fresh-access", "fresh-refresh"),
        ),
        ("/v1/oauth/token", 401, json!({})),
    ]);
    let account = w.account(&mock);
    assert!(account.signed_in());
    let asks: Vec<_> = (0..4).map(|_| account.token()).collect();
    let got =
        w.rt.block_on(async { futures_util::future::join_all(asks).await });
    for t in got {
        assert_eq!(t.unwrap(), "fresh-access");
    }
    let refreshes = mock.seen("/v1/oauth/token");
    assert_eq!(refreshes.len(), 1, "one refresh for four askers");
    assert_eq!(refreshes[0].body["grant_type"], "refresh_token");
    assert_eq!(refreshes[0].body["refresh_token"], "old-refresh");
    let saved = w
        .store
        .load_opt::<ClaudeCredential>()
        .unwrap()
        .value
        .unwrap();
    assert_eq!(saved.refresh, "fresh-refresh");
    assert!(saved.expires.unix() > now_unix() + 3000.0);
    assert_eq!(saved.email.as_deref(), Some("nick@example.com"));

    // Good for an hour: no refresh. Forced, and turned away: signed out.
    assert_eq!(w.rt.block_on(account.token()).unwrap(), "fresh-access");
    assert_eq!(mock.seen("/v1/oauth/token").len(), 1);
    let refused = w.rt.block_on(account.refresh_now()).unwrap_err();
    assert!(refused.text().starts_with("Claude sign-in expired"));
    assert!(!account.signed_in());
    assert!(!w.store.path(DataFile::Claude).exists());
    let none = w.rt.block_on(account.token()).unwrap_err();
    assert_eq!(none.text(), NOT_SIGNED_IN);
}

#[test]
fn asking_sends_claude_codes_request_and_retries_after_a_401() {
    let w = world();
    w.signed_in(3600.0);
    let answer = json!({
        "model": "claude-haiku-4-5-20251001",
        "content": [{"type": "text", "text": "{\"pong\": true}"}],
        "stop_reason": "end_turn",
    });
    let mock = Mock::start(vec![
        ("/v1/messages", 401, json!({})),
        ("/v1/messages", 200, answer),
        (
            "/v1/oauth/token",
            200,
            token("second-access", "second-refresh"),
        ),
    ]);
    let account = w.account(&mock);
    let (text, model) =
        w.rt.block_on(account.ask(
            "Reply with JSON only.",
            "ping",
            "claude-haiku-4-5",
            20,
            Duration::from_secs(15),
        ))
        .unwrap();
    assert_eq!(text, "{\"pong\": true}");
    assert_eq!(model, "claude-haiku-4-5-20251001");
    let sent = mock.seen("/v1/messages");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].headers["authorization"], "Bearer old-access");
    let h = &sent[1].headers;
    assert_eq!(h["authorization"], "Bearer second-access");
    assert_eq!(h["anthropic-version"], "2023-06-01");
    assert_eq!(h["anthropic-beta"], messages::BETAS);
    assert_eq!(h["user-agent"], messages::USER_AGENT);
    assert_eq!(h["x-app"], "cli");
    assert_eq!(h["anthropic-dangerous-direct-browser-access"], "true");
    let body = &sent[1].body;
    assert_eq!(body["model"], "claude-haiku-4-5");
    assert_eq!(body["max_tokens"], 4096, "an ask gets at least 4096");
    assert_eq!(body["system"][0]["text"], messages::IDENTITY);
    assert_eq!(body["system"][1]["text"], "Reply with JSON only.");
    assert_eq!(
        body["messages"][0],
        json!({"role": "user", "content": "ping"})
    );

    // A refusal comes back in words.
    let mock = Mock::start(vec![("/v1/messages", 429, json!({}))]);
    let account = Account::with_endpoints(w.store.clone(), w.rt.handle().clone(), mock.endpoints());
    let e =
        w.rt.block_on(account.ask("", "x", "claude-haiku-4-5", 10, Duration::from_secs(5)))
            .unwrap_err();
    assert!(e.text().contains("rate-limiting"), "{}", e.text());
}

#[test]
fn control_answers_with_the_status_and_never_a_token() {
    let w = world();
    w.signed_in(3600.0);
    let mock = Mock::start(vec![]);
    let account = w.account(&mock);
    let op = |op: &str| {
        let mut p = Map::new();
        p.insert("op".into(), json!(op));
        p
    };
    let st = account.control(&op("status"), || unreachable!());
    assert_eq!(st["signedIn"], true);
    assert_eq!(st["email"], "nick@example.com");
    assert!(st["expires"].as_str().unwrap().ends_with('Z'));
    assert!(!st.to_string().contains("old-access"));
    let mut opened = false;
    let st = account.control(&op("signin"), || {
        opened = true;
        Ok(())
    });
    assert!(opened);
    assert_eq!(st["phase"], "idle", "the shell's sign-in is what starts it");
    assert_eq!(
        account.control(&op("signin"), || Err("no window".into()))["error"],
        "no window"
    );
    assert_eq!(
        account.control(&op("paste"), || Ok(()))["error"],
        "paste needs code"
    );
    assert!(
        account.control(&op("nope"), || Ok(()))["error"]
            .as_str()
            .unwrap()
            .starts_with("unknown claude op nope")
    );
    let st = account.control(&op("signout"), || Ok(()));
    assert_eq!(st["signedIn"], false);
    assert_eq!(st["expires"], "");
}

#[test]
fn pasted_codes_read_every_way_claude_shows_them() {
    let both = |c: &str, s: &str| (Some(c.to_string()), Some(s.to_string()));
    assert_eq!(parse_redirect("abc#xyz"), both("abc", "xyz"));
    assert_eq!(parse_redirect(" a%2Bb#x%3Dy \n"), both("a+b", "x=y"));
    assert_eq!(
        parse_redirect("http://localhost:53692/callback?code=abc&state=xyz"),
        both("abc", "xyz")
    );
    assert_eq!(parse_redirect("?code=abc&state=xyz"), both("abc", "xyz"));
    assert_eq!(parse_redirect("code=abc"), (Some("abc".into()), None));
    assert_eq!(parse_redirect("plain"), (Some("plain".into()), None));
}
