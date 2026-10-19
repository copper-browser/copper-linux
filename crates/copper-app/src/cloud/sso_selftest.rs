//! Single sign-on's self-test: `COPPER_SELFTEST=cloud-sso copper [LINK]`
//! in a throwaway `COPPER_WORLD`. Screenshots (`linux-sso-*.png`) go to
//! `COPPER_SELFTEST_OUT` (default `/tmp/copper-cloud-sso-selftest`).
//!
//! A fake copper-cloud on loopback (plain HTTP; `COPPER_SELFTEST_SSO_PORT`,
//! or any free port) offers Google for `@exowatt.com`, answers the
//! exchange (`good-code` signs in, `gmail-code` is refused with
//! `sso_domain`) and enough of sync to turn it on. The system browser is
//! played in-process: it notes the address and comes back with the code the
//! step asks for — or never, until Cancel.
//!
//! * a `copper://cloud/join` link on the command line opens Settings ›
//!   Cloud with it filled in and the sign-in started (when `LINK` is one);
//! * Connect: a join link is read (host, fingerprint, "Continue with
//!   Google"), the row becomes the spinner + "Finish signing in in your
//!   browser…" + Cancel, Cancel stops it, `sso_domain` reads "Use your
//!   @exowatt.com account", a good code signs in and sync turns on with
//!   every switch — light and dark;
//! * an instance with a certificate nobody vouches for and no pin gets the
//!   trust card (`COPPER_SELFTEST_SSO_TLS=127.0.0.1:PORT`, e.g. an
//!   `openssl s_server`), when set;
//! * a join link while signed in there says so; one for another instance
//!   shows "This link is for HOST" and changes nothing (GApplication.Open
//!   and the address field's Go);
//! * an instance linked with a link code shows "Continue with Google" on
//!   the Account step, its password form folded behind "Sign in with a
//!   password instead", and signs in from there.
//!
//! Prints `PASS`/`FAIL` lines and exits non-zero on any failure.

use super::{get, page, runtime};
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_cloud::Domain;
use copper_shell_core::cloud::sso::{Opener, sign_in};
use gtk4::prelude::*;
use libadwaita::prelude::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What the played browser does with the next sign-in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Browser {
    /// Never comes back (until Cancel).
    Hold,
    /// The provider sends this code back.
    Code(&'static str),
}

struct Played {
    next: Mutex<Browser>,
    opened: Mutex<Vec<String>>,
}

/// Start when `COPPER_SELFTEST=cloud-sso`: the fake instance and the
/// played browser now (a link on the command line uses them at once), the
/// steps once the window is up.
pub fn maybe_start(shell: &Shell) {
    if std::env::var("COPPER_SELFTEST").ok().as_deref() != Some("cloud-sso") {
        return;
    }
    let port = std::env::var("COPPER_SELFTEST_SSO_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(0);
    let port = match runtime().block_on(fake_instance(port)) {
        Ok(p) => p,
        Err(e) => {
            println!("FAIL the fake copper-cloud: {e}");
            crate::app::set_exit(1);
            return;
        }
    };
    println!("cloud-sso selftest: fake copper-cloud on 127.0.0.1:{port}");
    let played = Arc::new(Played {
        next: Mutex::new(Browser::Hold),
        opened: Mutex::new(Vec::new()),
    });
    super::set_fake_browser(Some(browser(played.clone())));
    let shell = shell.clone();
    glib::timeout_add_local_once(Duration::from_millis(1200), move || {
        glib::spawn_future_local(async move {
            let failures = run(&shell, port, played).await;
            super::set_fake_browser(None);
            println!("cloud-sso selftest: {} failure(s)", failures.len());
            for f in &failures {
                println!("  - {f}");
            }
            crate::app::set_exit(if failures.is_empty() { 0 } else { 1 });
            shell.shutdown();
            shell.app().quit();
        });
    });
}

/// The played system browser.
fn browser(played: Arc<Played>) -> Opener {
    Arc::new(move |url: &str| {
        played.opened.lock().expect("opened").push(url.to_string());
        let next = *played.next.lock().expect("next");
        let Browser::Code(code) = next else {
            return Ok(());
        };
        let u = url::Url::parse(url).map_err(|e| e.to_string())?;
        let q: BTreeMap<String, String> = u.query_pairs().into_owned().collect();
        let back = format!(
            "{}?code={code}&state={}",
            q.get("redirect_uri").cloned().unwrap_or_default(),
            q.get("state").cloned().unwrap_or_default()
        );
        runtime().spawn(async move {
            // Long enough to see the waiting row.
            tokio::time::sleep(Duration::from_millis(700)).await;
            let _ = get_url(&back).await;
        });
        Ok(())
    })
}

/// A bare GET (the provider's redirect to the loopback).
async fn get_url(url: &str) -> Option<String> {
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

// MARK: the fake instance

async fn fake_instance(port: u16) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            tokio::spawn(serve(stream, port));
        }
    });
    Ok(port)
}

async fn serve(mut stream: tokio::net::TcpStream, port: u16) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
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
        if buf.len() > 1 << 20 {
            return;
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
    let (status, answer) = route(&line, &body, port);
    let text = answer.to_string();
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn user() -> Value {
    json!({"id": "7d1c3a8e-0000-4000-8000-000000000001", "email": "farce@exowatt.com",
           "display_name": "Felipe Arce", "created_at": "2026-10-19T00:00:00Z"})
}

fn device() -> Value {
    json!({"id": "0b6e8f8a-1111-4111-8111-111111111111", "name": "Linux self-test"})
}

fn route(line: &str, body: &[u8], port: u16) -> (u16, Value) {
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let path = target.split('?').next().unwrap_or("");
    match (method, path) {
        ("GET", "/healthz") => (200, json!({"ok": true})),
        ("GET", "/v1/auth/sso") => (
            200,
            json!({"enabled": true, "password_signin": false, "providers": [{
                "id": "google", "kind": "google", "name": "Google",
                "client_id": "123-selftest.apps.googleusercontent.com",
                "authorization_endpoint": format!("http://127.0.0.1:{port}/authorize"),
                "scopes": ["openid", "email", "profile"],
                "domains": ["exowatt.com"], "hd": "exowatt.com"}]}),
        ),
        ("POST", "/v1/auth/sso/google") => {
            let v: Value = serde_json::from_slice(body).unwrap_or_default();
            match v["code"].as_str() {
                Some("good-code") => (
                    200,
                    json!({"token": "tok_selftest_sso", "user": user(), "device": device(),
                           "gate_key": "ck_selftest_minted", "created": false}),
                ),
                Some("gmail-code") => (
                    403,
                    json!({"error": "sso_domain", "message": "Only exowatt.com accounts"}),
                ),
                _ => (401, json!({"error": "sso_failed", "message": "bad code"})),
            }
        }
        ("GET", "/v1/auth/me") => (200, json!({"user": user(), "device": device()})),
        ("POST", "/v1/auth/logout") => (200, json!({})),
        ("GET", "/v1/info") => (
            200,
            json!({"name": "copper-cloud", "version": "0.9.0-selftest"}),
        ),
        ("GET", "/v1/sync/docs") => (200, json!([])),
        ("PUT", p) if p.starts_with("/v1/sync/docs/") => (200, json!({"version": 1})),
        ("GET", p) if p.starts_with("/v1/sync/docs/") => (
            404,
            json!({"error": "not_found", "message": "No such document"}),
        ),
        ("DELETE", _) => (200, json!({})),
        ("POST", "/v1/sync/history") => (200, json!({"rejected": []})),
        ("GET", "/v1/sync/history") => (200, json!({"entries": [], "next": 0, "more": false})),
        _ => (404, json!({"error": "not_found", "message": "Not Found"})),
    }
}

// MARK: the steps

struct Run {
    shell: Shell,
    out: PathBuf,
    failures: RefCell<Vec<String>>,
}

impl Run {
    fn check(&self, ok: bool, what: &str) -> bool {
        println!("{} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.borrow_mut().push(what.into());
        }
        ok
    }

    async fn shot(&self, name: &str) {
        crate::app::settle(&self.shell.downgrade(), Duration::from_secs(5)).await;
        glib::timeout_future(Duration::from_millis(400)).await;
        let path = self.out.join(format!("linux-sso-{name}.png"));
        match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
            Ok(()) => println!("SHOT {}", path.display()),
            Err(e) => {
                self.check(false, &format!("screenshot {name}: {e}"));
            }
        }
    }

    /// Light and dark.
    async fn shots(&self, name: &str) {
        self.shot(&format!("{name}-light")).await;
        self.shell.set_look(copper_core::settings::Look::Dark);
        glib::timeout_future(Duration::from_millis(300)).await;
        self.shot(&format!("{name}-dark")).await;
        self.shell.set_look(copper_core::settings::Look::Light);
        glib::timeout_future(Duration::from_millis(300)).await;
    }

    fn page(&self) -> Option<Rc<page::CloudPage>> {
        page::current()
    }

    /// Settings › Cloud, freshly opened.
    async fn open(&self) -> Option<Rc<page::CloudPage>> {
        if let Some(d) = self.shell.window().visible_dialog() {
            d.close();
            glib::timeout_future(Duration::from_millis(200)).await;
        }
        super::open_settings(&self.shell);
        glib::timeout_future(Duration::from_millis(400)).await;
        self.page()
    }

    /// Wait (at most `secs`) until `what` holds of the page on screen.
    async fn until(&self, secs: u64, what: impl Fn(&page::CloudPage) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(secs) {
            if self.page().is_some_and(|p| what(&p)) {
                return true;
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
        false
    }
}

fn deep(code: &str) -> String {
    format!(
        "copper://cloud/join?code={}",
        url::form_urlencoded::byte_serialize(code.as_bytes()).collect::<String>()
    )
}

async fn run(shell: &Shell, port: u16, played: Arc<Played>) -> Vec<String> {
    let out = std::env::var("COPPER_SELFTEST_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/copper-cloud-sso-selftest"));
    let _ = std::fs::create_dir_all(&out);
    let r = Rc::new(Run {
        shell: shell.clone(),
        out,
        failures: RefCell::default(),
    });
    shell.set_look(copper_core::settings::Look::Light);
    let Some(cloud) = get() else {
        r.check(false, "Copper Cloud is on");
        return r.failures.borrow().clone();
    };
    let here = format!("127.0.0.1:{port}");
    let local = format!("copper-cloud://{here}/#sso=google&tls=off");
    let set = |b: Browser| *played.next.lock().expect("next") = b;

    // A join link on the command line: the page, filled in, signing in.
    let cold = std::env::args().any(|a| a.starts_with("copper://cloud/join"));
    if cold {
        let ok = r
            .until(10, |p| {
                p.code_field().is_some_and(|c| c.contains(&here))
                    && p.sso_waiting()
                        .is_some_and(|t| t.contains("Finish signing in in your browser"))
            })
            .await;
        r.check(
            ok,
            "copper copper://cloud/join?code=… opens Settings › Cloud with the code, the sign-in started",
        );
        r.shot("deep-link-launch").await;
        if let Some(p) = r.page() {
            p.press_cancel_sso();
        }
        let ok = r.until(5, |p| p.sso_waiting().is_none()).await;
        r.check(ok, "Cancel stops the sign-in a launch link started");
    }

    let Some(page) = r.open().await else {
        r.check(false, "Settings › Cloud opens");
        return r.failures.borrow().clone();
    };
    r.check(
        page.phase() == page::Phase::Unlinked,
        "unlinked to begin with",
    );

    // A pinned join link is read before anything is sent.
    let fp = "577920b797ef884344c9be3576103c6b1c0ca7776d72dff1f17a905435ffa1c4";
    page.type_code(&format!(
        "copper-cloud://cloud.example.test/#sso=google&fp={fp}"
    ));
    glib::timeout_future(Duration::from_millis(300)).await;
    let said = page.said_text().unwrap_or_default();
    r.check(
        said.contains("Join link")
            && said.contains("cloud.example.test")
            && said.contains("57:79:20"),
        &format!("a join link is read: host and fingerprint ({said})"),
    );
    r.check(
        page.sso_button_label().as_deref() == Some("Continue with Google"),
        "its button says Continue with Google",
    );
    r.shots("join-link").await;

    // The trust step, for a certificate nobody vouches for and no pin.
    if let Ok(tls) = std::env::var("COPPER_SELFTEST_SSO_TLS")
        && !tls.trim().is_empty()
    {
        page.type_code(&format!("copper-cloud://{}/#sso=google", tls.trim()));
        glib::timeout_future(Duration::from_millis(200)).await;
        page.press_sso();
        let ok = r.until(10, |p| p.trust_shown().is_some()).await;
        r.check(
            ok,
            "no pin and an unknown certificate: the fingerprint to compare, as for pairing",
        );
        r.shot("trust-light").await;
    }

    // The local instance: discovered, then the browser.
    page.type_code(&local);
    let ok = r
        .until(5, |p| {
            p.said_text().is_some_and(|s| s.contains("Join link"))
                && sign_in()
                    .known(&copper_cloud::parse_sso_link(&local).unwrap().endpoint)
                    .is_some()
        })
        .await;
    r.check(ok, "the instance says what it offers (GET /v1/auth/sso)");
    set(Browser::Hold);
    page.press_sso();
    let ok = r
        .until(5, |p| {
            p.sso_waiting()
                .is_some_and(|t| t.starts_with("Finish signing in in your browser…"))
        })
        .await;
    r.check(
        ok,
        &format!(
            "Continue with Google: the row says Finish signing in in your browser… ({:?})",
            page.sso_waiting()
        ),
    );
    r.check(
        played
            .opened
            .lock()
            .expect("opened")
            .last()
            .is_some_and(|u| u.contains("/authorize?") && u.contains("hd=exowatt.com")),
        "the system browser is opened at the provider",
    );
    r.check(
        page.sso_button_label().is_none(),
        "the button gives way to the waiting row",
    );
    r.shots("waiting").await;
    page.press_cancel_sso();
    let ok = r
        .until(5, |p| p.sso_waiting().is_none() && p.error_text().is_none())
        .await;
    r.check(ok, "Cancel: back to the button, no problem shown");

    // The wrong account.
    set(Browser::Code("gmail-code"));
    page.press_sso();
    let ok = r
        .until(10, |p| {
            p.error_text()
                .is_some_and(|e| e == "Use your @exowatt.com account")
        })
        .await;
    r.check(
        ok,
        &format!(
            "sso_domain reads Use your @exowatt.com account ({:?})",
            page.error_text()
        ),
    );
    r.check(!cloud.client.is_linked(), "a refusal keeps nothing");
    r.shots("domain-error").await;

    // `connect_code` with a join link answers `sso_link`; the page sends it
    // to the browser instead (above).
    let client = cloud.client.clone();
    let code = local.clone();
    let refused = runtime()
        .spawn(async move { client.connect_code(&code, None).await.map(|_| ()) })
        .await;
    r.check(
        matches!(&refused, Ok(Err(f)) if f.code == "sso_link"),
        "connect_code refuses a join link with sso_link",
    );

    // Signed in.
    set(Browser::Code("good-code"));
    page.press_sso();
    let ok = r.until(15, |p| p.phase() == page::Phase::Syncing).await;
    r.check(ok, "a good code signs in and turns sync on");
    r.check(
        cloud.client.is_signed_in()
            && cloud
                .client
                .account()
                .is_some_and(|a| a.email == "farce@exowatt.com"),
        "kept like a pairing (cloud.json: link, account, token)",
    );
    r.check(
        cloud
            .client
            .link()
            .is_some_and(|l| l.key == "ck_selftest_minted"),
        "the instance credential the exchange minted is the link's key",
    );
    r.check(
        cloud.sync.is_on() && cloud.sync.enabled() == Domain::ALL.into_iter().collect(),
        "sync on with every switch, through the window's engine",
    );
    glib::timeout_future(Duration::from_millis(1500)).await;
    r.shots("signed-in").await;

    // A join link for the instance signed in to already (GApplication.Open).
    let again = deep(&local);
    shell.app().open(&[gio::File::for_uri(&again)], "");
    let ok = r
        .until(5, |p| {
            p.notice_text()
                .is_some_and(|t| t == format!("Already signed in to {here} as farce@exowatt.com"))
        })
        .await;
    r.check(
        ok,
        &format!(
            "a join link while signed in there says so ({:?})",
            r.page().and_then(|p| p.notice_text())
        ),
    );
    r.shot("already-signed-in-light").await;

    // One for another instance, typed into the address field: nothing
    // changes without a press.
    let other = deep("copper-cloud://cloud.other.test/#sso=google");
    match copper_shell_core::cloud::join::palette_offers(&other) {
        Some(offers) => {
            r.check(
                offers.rows[0].title == "Join Copper Cloud — cloud.other.test",
                "the address field offers Join Copper Cloud — cloud.other.test",
            );
            crate::palette::perform(shell, &offers.rows[0], crate::palette::Mode::Edit, false);
        }
        None => {
            r.check(false, "the address field offers to join");
        }
    }
    let ok = r
        .until(5, |p| {
            p.notice_text()
                .is_some_and(|t| t == "This link is for cloud.other.test")
        })
        .await;
    r.check(
        ok,
        "another instance's link: This link is for cloud.other.test",
    );
    r.check(
        cloud.client.link().is_some_and(|l| l.host() == here) && cloud.client.is_signed_in(),
        "still linked and signed in where it was",
    );
    r.check(
        shell
            .browser()
            .active()
            .is_none_or(|t| !t.url.starts_with("copper://cloud")),
        "no tab opened for it",
    );
    r.shots("other-instance").await;
    if let Some(p) = r.page() {
        p.press_dismiss();
    }
    let ok = r.until(3, |p| p.notice_text().is_none()).await;
    r.check(ok, "Dismiss puts it away");

    // Linked with a link code: the Account step offers Google too.
    if let Some(p) = r.page() {
        p.disconnect();
    }
    let ok = r.until(10, |p| p.phase() == page::Phase::Unlinked).await;
    r.check(ok, "Disconnect");
    let Some(page) = r.open().await else {
        r.check(false, "Settings › Cloud opens again");
        return r.failures.borrow().clone();
    };
    page.type_code(&format!("copper-cloud://{here}/#k=ck_instance_key&tls=off"));
    glib::timeout_future(Duration::from_millis(200)).await;
    page.press_connect();
    let ok = r
        .until(10, |p| {
            p.phase() == page::Phase::Linked
                && p.sso_button_label().as_deref() == Some("Continue with Google")
        })
        .await;
    r.check(
        ok,
        &format!(
            "linked with a link code: Continue with Google on the Account step ({:?})",
            r.page().and_then(|p| p.sso_button_label())
        ),
    );
    r.check(
        r.page().is_some_and(|p| p.password_folded()),
        "password_signin false: the password form waits behind a button",
    );
    r.shots("account-step").await;
    if let Some(p) = r.page() {
        p.press_password_instead();
    }
    let ok = r.until(3, |p| !p.password_folded()).await;
    r.check(ok, "Sign in with a password instead shows the form");
    r.shot("password-instead-light").await;
    set(Browser::Code("good-code"));
    if let Some(p) = r.page() {
        p.press_sso();
    }
    let ok = r.until(15, |p| p.phase() == page::Phase::Syncing).await;
    r.check(ok, "Continue with Google on the Account step signs in");

    // Leave the world as found.
    if let Some(p) = r.page() {
        p.disconnect();
    }
    let _ = r.until(10, |p| p.phase() == page::Phase::Unlinked).await;
    if let Some(d) = shell.window().visible_dialog() {
        d.close();
    }
    r.failures.borrow().clone()
}
