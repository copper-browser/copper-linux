//! The Shield list through WebKitGTK's own content-extension compiler, then
//! enforced on real page loads from a local HTTP server.
//!
//! Third-party requests need a host other than the page's. Every tracker is
//! requested as `http://<tracker domain>@127.0.0.1:<port>/t/<name>.js` from a
//! page served as `http://localhost:<port>/…`: the URL string starts with the
//! tracker's domain (what `url-filter` matches), the request goes to this
//! server, and its host (127.0.0.1) differs from the page's (localhost), so
//! WebKit classes it third-party exactly as it would a real ad server.
//!
//! `harness = false`: GTK must own the main thread. Needs a display
//! (headless weston via `bin/dbx`).

use std::collections::BTreeSet;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use copper_engine_webkitgtk::WebKitEngine;
use copper_engine_webkitgtk::WebKitPage;
use copper_engine_webkitgtk::copper_engine::{
    ContentFilter, Engine, EnginePaths, EventStream, JsonValue, LoadState, PageEvent, PageOptions,
    PageView, ProfileId, World,
};
use copper_shield::{List, Shield, UNWANTED, compiled};
use gtk4::prelude::*;

const STEP: Duration = Duration::from_secs(40);
const WATCHDOG: Duration = Duration::from_secs(400);

/// Ten of the Mac's unwanted domains, each one tracker on the fixture page.
const TEN: [&str; 10] = [
    "doubleclick.net",
    "googlesyndication.com",
    "google-analytics.com",
    "googletagmanager.com",
    "amazon-adsystem.com",
    "criteo.com",
    "taboola.com",
    "hotjar.com",
    "connect.facebook.net",
    "scorecardresearch.com",
];

type Res = Result<String, String>;

// ---------------------------------------------------------------- server

#[derive(Clone)]
struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = log.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                    loop {
                        let mut h = String::new();
                        if reader.read_line(&mut h).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    log.lock().unwrap().push(path.clone());
                    let (kind, body) = respond(&path, port);
                    let mut s = stream;
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        Server { port, seen }
    }

    fn clear(&self) {
        self.seen.lock().unwrap().clear();
    }

    fn trackers_seen(&self) -> BTreeSet<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.strip_prefix("/t/"))
            .map(|p| p.trim_end_matches(".js").to_string())
            .collect()
    }
}

/// `/page/<host>` — the fixture page, its trackers addressed through `host`.
fn respond(path: &str, port: u16) -> (&'static str, String) {
    if let Some(name) = path.strip_prefix("/t/") {
        let name = name.trim_end_matches(".js");
        return (
            "text/javascript",
            format!("window.ran = (window.ran || []).concat([{name:?}]);"),
        );
    }
    let host = path.strip_prefix("/page/").unwrap_or("127.0.0.1");
    let mut scripts = String::new();
    for domain in TEN {
        scripts.push_str(&format!(
            "<script src=\"http://{domain}@{host}:{port}/t/{domain}.js\"></script>\n"
        ));
    }
    scripts.push_str(&format!(
        "<script src=\"http://ok.example@{host}:{port}/t/control.js\"></script>\n"
    ));
    (
        "text/html",
        format!(
            "<!doctype html><title>shield</title>\
             <ins class=\"adsbygoogle\" id=\"ins\">ad</ins>\
             <div id=\"google_ads_iframe_1\">ad</div>\
             <div id=\"div-gpt-ad-123\">ad</div>\
             <div class=\"article\" id=\"keep\">text</div>\n{scripts}"
        ),
    )
}

// ---------------------------------------------------------------- page

struct Tab {
    page: WebKitPage,
    ev: EventStream<PageEvent>,
}

impl Tab {
    async fn load(&mut self, url: &str) -> Result<(), String> {
        while self.ev.try_next().is_some() {}
        self.page.load(url);
        loop {
            match self.ev.next().await.ok_or("event stream ended")? {
                PageEvent::Load(LoadState::Finished { .. }) => return Ok(()),
                PageEvent::Load(LoadState::Failed {
                    error, cancelled, ..
                }) if !cancelled => return Err(format!("load failed: {error}")),
                _ => {}
            }
        }
    }

    async fn look(&self) -> Result<JsonValue, String> {
        self.page
            .eval(
                "({ran: window.ran || [], \
                   ins: getComputedStyle(document.getElementById('ins')).display, \
                   gads: getComputedStyle(document.getElementById('google_ads_iframe_1')).display, \
                   gpt: getComputedStyle(document.getElementById('div-gpt-ad-123')).display, \
                   keep: getComputedStyle(document.getElementById('keep')).display})",
                &World::Page,
            )
            .await
            .map_err(|e| e.to_string())
    }
}

struct Ctx {
    srv: Server,
    engine: WebKitEngine,
    tab: Tab,
    shield: Shield<WebKitPage>,
}

impl Ctx {
    fn url(&self, page_host: &str, tracker_host: &str) -> String {
        format!("http://{page_host}:{}/page/{tracker_host}", self.srv.port)
    }
}

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

// ---------------------------------------------------------------- checks

/// No list: every tracker runs (proves the page and the trick work at all).
async fn check_baseline(ctx: &mut Ctx) -> Res {
    ctx.srv.clear();
    let url = ctx.url("localhost", "127.0.0.1");
    ctx.tab.load(&url).await?;
    sleep(200).await;
    let seen = ctx.srv.trackers_seen();
    let v = ctx.tab.look().await?;
    ensure(seen.len() == 11 && v["ins"] != "none", || {
        format!("without the list: server saw {seen:?}; page {v}")
    })?;
    Ok(format!(
        "{} requests reach the server unfiltered",
        seen.len()
    ))
}

/// Compile through WebKit's compiler, then a second call loads it.
async fn check_compile(ctx: &mut Ctx) -> Res {
    let list = List::standard();
    // A tab opened before the list exists waits for it (Shield.waiting).
    ctx.shield.protect(&ctx.tab.page);
    ensure(ctx.shield.waiting() == 1, || "page not waiting".into())?;
    let filter = compiled(&ctx.engine, &list, None)
        .await
        .map_err(|e| format!("WebKit refused the Mac list: {e}"))?;
    ensure(filter.id() == list.id, || format!("id {}", filter.id()))?;
    let stored = ctx
        .engine
        .load_content_filter(&list.id)
        .await
        .map_err(|e| e.to_string())?;
    ensure(stored.is_some(), || "not stored under its hash id".into())?;
    let again = compiled(&ctx.engine, &list, Some(&list.id))
        .await
        .map_err(|e| e.to_string())?;
    ensure(again.id() == list.id, || "reload id".into())?;
    ctx.shield.ready(Ok(filter));
    ensure(
        ctx.shield.waiting() == 0 && ctx.shield.list().is_some(),
        || "waiting page not handed the list".into(),
    )?;
    Ok(format!("compiled and stored as {}", list.id))
}

/// The ten trackers are blocked before they load; the control and the
/// article are untouched; the ad slots are display:none.
async fn check_blocks(ctx: &mut Ctx) -> Res {
    ctx.srv.clear();
    let url = ctx.url("localhost", "127.0.0.1");
    ctx.tab.load(&url).await?;
    sleep(300).await;
    let seen = ctx.srv.trackers_seen();
    let v = ctx.tab.look().await?;
    let blocked: Vec<&str> = TEN.iter().copied().filter(|d| !seen.contains(*d)).collect();
    ensure(
        blocked.len() == 10
            && seen.contains("control")
            && v["ins"] == "none"
            && v["gads"] == "none"
            && v["gpt"] == "none"
            && v["keep"] == "block"
            && v["ran"] == serde_json::json!(["control"]),
        || format!("blocked {blocked:?}; server saw {seen:?}; page {v}"),
    )?;
    Ok(format!(
        "10/10 trackers blocked, control loaded, 3 slots hidden: {v}"
    ))
}

/// Same-site requests are the site: `load-type: third-party` leaves them.
async fn check_first_party(ctx: &mut Ctx) -> Res {
    ctx.srv.clear();
    let url = ctx.url("127.0.0.1", "127.0.0.1");
    ctx.tab.load(&url).await?;
    sleep(300).await;
    let seen = ctx.srv.trackers_seen();
    ensure(seen.len() == 11, || format!("first-party saw {seen:?}"))?;
    Ok("first-party requests untouched".into())
}

/// Off for one site (the Mac's tune at navigation), then back on.
async fn check_paused(ctx: &mut Ctx) -> Res {
    let paused = ctx.shield.pause("localhost", true);
    ensure(paused == ["localhost"], || format!("paused {paused:?}"))?;
    ctx.shield.tune(&ctx.tab.page, Some("localhost"));
    ctx.srv.clear();
    let url = ctx.url("localhost", "127.0.0.1");
    ctx.tab.load(&url).await?;
    sleep(300).await;
    let off = ctx.srv.trackers_seen().len();
    let v = ctx.tab.look().await?;
    ensure(off == 11 && v["ins"] != "none", || {
        format!("paused site still filtered: saw {off}; page {v}")
    })?;
    ctx.shield.pause("localhost", false);
    ctx.shield.tune(&ctx.tab.page, Some("localhost"));
    ctx.srv.clear();
    ctx.tab.load(&url).await?;
    sleep(300).await;
    let on = ctx.srv.trackers_seen().len();
    ensure(on == 1, || format!("resumed site: saw {on}"))?;
    // Switched off everywhere, then on again (Shield.apply).
    ctx.shield.enabled = false;
    ctx.shield.apply([&ctx.tab.page]);
    ctx.srv.clear();
    ctx.tab.load(&url).await?;
    sleep(300).await;
    let disabled = ctx.srv.trackers_seen().len();
    ctx.shield.enabled = true;
    ctx.shield.apply([&ctx.tab.page]);
    ensure(disabled == 11, || format!("disabled: saw {disabled}"))?;
    Ok("paused: 11 requests; resumed: 1; disabled: 11".into())
}

/// The baked-in exemption: the same list plus ignore-previous-rules for one
/// page domain compiles, and lets that site alone while still blocking on
/// another.
async fn check_exempting(ctx: &mut Ctx) -> Res {
    let list = List::exempting(["localhost"]);
    let filter = compiled(&ctx.engine, &list, None)
        .await
        .map_err(|e| format!("WebKit refused the exempting list: {e}"))?;
    ctx.tab.page.set_content_filter(Some(&filter));
    ctx.srv.clear();
    ctx.tab.load(&ctx.url("localhost", "127.0.0.1")).await?;
    sleep(300).await;
    let exempt = ctx.srv.trackers_seen().len();
    let v = ctx.tab.look().await?;
    ctx.srv.clear();
    ctx.tab.load(&ctx.url("127.0.0.1", "localhost")).await?;
    sleep(300).await;
    let other = ctx.srv.trackers_seen().len();
    ctx.tab.page.set_content_filter(None);
    let _ = ctx.engine.remove_content_filter(&list.id).await;
    ensure(exempt == 11 && v["ins"] != "none" && other == 1, || {
        format!("exempt site saw {exempt} (page {v}); other site saw {other}")
    })?;
    Ok("exempt site: 11 requests, slots shown; other site: 1".into())
}

/// A list WebKit can't compile is reported, not swallowed.
async fn check_trouble(ctx: &mut Ctx) -> Res {
    let bad = List {
        json: "[{\"trigger\": {}}]".into(),
        id: "office-shield-bad".into(),
    };
    let result = compiled(&ctx.engine, &bad, None).await;
    let mut shield: Shield<WebKitPage> = Shield::new(true, Vec::new());
    shield.ready(result.map_err(|e| e.to_string()));
    ensure(
        shield.trouble().is_some() && shield.list().is_none(),
        || "bad list accepted".into(),
    )?;
    Ok(format!("refused: {}", shield.trouble().unwrap_or_default()))
}

const CHECKS: &[&str] = &[
    "baseline",
    "compile",
    "blocks",
    "first-party",
    "paused",
    "exempting",
    "trouble",
];

fn run<'a>(ctx: &'a mut Ctx, name: &str) -> Pin<Box<dyn Future<Output = Res> + 'a>> {
    match name {
        "baseline" => Box::pin(check_baseline(ctx)),
        "compile" => Box::pin(check_compile(ctx)),
        "blocks" => Box::pin(check_blocks(ctx)),
        "first-party" => Box::pin(check_first_party(ctx)),
        "paused" => Box::pin(check_paused(ctx)),
        "exempting" => Box::pin(check_exempting(ctx)),
        "trouble" => Box::pin(check_trouble(ctx)),
        _ => unreachable!(),
    }
}

fn main() {
    // libtest flags (e.g. --list from `cargo test -- --list`) don't apply.
    if std::env::args().any(|a| a == "--list") {
        println!("webkit: test");
        return;
    }
    gtk4::init().expect("gtk init (is WAYLAND_DISPLAY/DISPLAY set?)");
    std::thread::spawn(|| {
        std::thread::sleep(WATCHDOG);
        eprintln!("shield webkit: watchdog fired");
        std::process::exit(3);
    });
    assert_eq!(UNWANTED.len(), 44);

    let srv = Server::start();
    let root = std::env::temp_dir().join(format!("copper-shield-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let engine = WebKitEngine::new(EnginePaths::under(&root));
    let profile = engine
        .profile(&ProfileId::new("shield").unwrap(), true)
        .expect("profile");
    let page = engine.new_page(&profile, PageOptions::default());
    let widget = page.widget();
    let window = gtk4::Window::builder()
        .default_width(900)
        .default_height(600)
        .child(&widget)
        .build();
    window.present();

    let ev = page.events();
    let mut ctx = Ctx {
        srv,
        engine,
        tab: Tab { page, ev },
        shield: Shield::new(true, Vec::new()),
    };
    let fails = glib::MainContext::default().block_on(async move {
        let mut fails = 0;
        for &name in CHECKS {
            let res = match glib::future_with_timeout(STEP, run(&mut ctx, name)).await {
                Ok(r) => r,
                Err(_) => Err(format!("timed out after {STEP:?}")),
            };
            match res {
                Ok(d) => println!("PASS {name}: {d}"),
                Err(e) => {
                    fails += 1;
                    println!("FAIL {name}: {e}");
                }
            }
        }
        println!(
            "shield webkit: {}/{} passed",
            CHECKS.len() - fails,
            CHECKS.len()
        );
        fails
    });
    let _ = std::fs::remove_dir_all(&root);
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
