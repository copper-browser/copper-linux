//! End-to-end test of the WebKitGTK engine against a local HTTP server.
//!
//! GTK may only be used from the thread that initialised it, so this test
//! has its own `main` (`harness = false`): it runs every check in order on
//! the GTK main loop, prints `PASS`/`FAIL` per check and exits non-zero on
//! any failure. libtest flags such as `--test-threads=1` are ignored. Needs
//! a display (headless weston via `bin/dbx`).

mod backstage;
mod server;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use copper_engine_webkitgtk::copper_engine::{
    ConsoleLevel, ContentFilter, Cookie, CrashReason, DownloadEvent, DownloadId, DownloadState,
    Engine, EngineError, EnginePaths, EventStream, FindOptions, InjectionTime, InputEvent, JsonMap,
    JsonValue, LoadState, Modifiers, MouseButton, PageEvent, PageOptions, PageView, PermissionKind,
    Profile, ProfileId, SnapshotRegion, StyleLevel, Trust, UserScript, UserStyleSheet,
    WebsiteDataKinds, WindowDisposition, World,
};
use copper_engine_webkitgtk::{WebKitEngine, WebKitPage, WebKitProfile};
use gdk4::prelude::*;
use gtk4::prelude::*;

/// Per-check timeout.
const STEP: Duration = Duration::from_secs(30);
/// Whole-process watchdog.
const WATCHDOG: Duration = Duration::from_secs(600);

const CHECKS: &[&str] = &[
    "profiles",
    "load-events",
    "favicon",
    "eval",
    "isolated-world",
    "call-async",
    "script-message",
    "console",
    "snapshot",
    "find-zoom",
    "user-content",
    "history",
    "cookies",
    "website-data",
    "content-filter",
    "input",
    "new-window",
    "popup-handler",
    "download",
    "download-cancel",
    "hard-reload",
    "permission",
    "crash",
    // T-002 / D-006: trusted input and the backstage.
    "self-check",
    "backstage-hold",
    "backstage-raf",
    "backstage-snapshot",
    "trusted-click",
    "trusted-type",
    "enter-submits",
    "backstage-popups",
    "page-focus",
    "handoff",
    "stalled-clock",
    "script-fallback",
    "release",
    "gtk-warnings",
];

/// Three-rule WebKit content-blocker list (the JSON shape Shield.swift emits).
const FILTER_RULES: &str = r#"[
  {"trigger": {"url-filter": "blocked\\.js"}, "action": {"type": "block"}},
  {"trigger": {"url-filter": "tracker\\.gif"}, "action": {"type": "block"}},
  {"trigger": {"url-filter": ".*"}, "action": {"type": "css-display-none", "selector": ".ad"}}
]"#;

type Res = Result<String, String>;

struct Tab {
    page: WebKitPage,
    ev: EventStream<PageEvent>,
}

impl Tab {
    fn new(page: WebKitPage) -> Self {
        let ev = page.events();
        Tab { page, ev }
    }

    fn drain(&mut self) {
        while self.ev.try_next().is_some() {}
    }

    /// Wait for an event `f` maps to `Some`.
    async fn wait<T>(&mut self, mut f: impl FnMut(&PageEvent) -> Option<T>) -> Result<T, String> {
        loop {
            let e = self.ev.next().await.ok_or("page event stream ended")?;
            if let Some(t) = f(&e) {
                return Ok(t);
            }
        }
    }

    /// Wait for the current load to finish; `Err` if it failed (not cancelled).
    async fn finished(&mut self) -> Result<(), String> {
        let mut failed = None;
        self.wait(|e| match e {
            PageEvent::Load(LoadState::Failed {
                error, cancelled, ..
            }) => {
                if !cancelled {
                    failed = Some(error.clone());
                }
                None
            }
            PageEvent::Load(LoadState::Finished { .. }) => Some(()),
            _ => None,
        })
        .await?;
        failed.map_or(Ok(()), |e| Err(format!("load failed: {e}")))
    }

    async fn load(&mut self, url: &str) -> Result<(), String> {
        self.drain();
        self.page.load(url);
        self.finished().await
    }

    async fn js(&self, code: &str) -> Result<JsonValue, String> {
        self.page
            .eval(code, &World::Page)
            .await
            .map_err(|e| e.to_string())
    }

    async fn js_in(&self, world: &World, code: &str) -> Result<JsonValue, String> {
        self.page.eval(code, world).await.map_err(|e| e.to_string())
    }

    async fn input(&self, ev: InputEvent) -> Result<Trust, String> {
        self.page.input(ev).await.map_err(|e| e.to_string())
    }
}

struct Ctx {
    srv: server::Server,
    root: PathBuf,
    engine: WebKitEngine,
    alpha: WebKitProfile,
    beta: WebKitProfile,
    private: WebKitProfile,
    a: Tab,
    b: Tab,
    e: Tab,
    saw_favicon: bool,
    bench: Option<backstage::Bench>,
}

fn main() {
    backstage::count_gtk_warnings();
    gtk4::init().expect("gtk init (is WAYLAND_DISPLAY/DISPLAY set?)");
    std::thread::spawn(|| {
        std::thread::sleep(WATCHDOG);
        eprintln!("integration: watchdog fired after {WATCHDOG:?}; aborting");
        std::process::exit(3);
    });

    let srv = server::start(make_icon()).expect("start local http server");
    let root = std::env::temp_dir().join(format!("copper-engine-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let engine = WebKitEngine::new(EnginePaths::under(&root));

    let id = |s: &str| ProfileId::new(s).unwrap();
    let alpha = engine.profile(&id("alpha"), false).expect("alpha");
    let beta = engine.profile(&id("beta"), false).expect("beta");
    let private = engine.profile(&id("private"), true).expect("private");

    let a = engine.new_page(
        &alpha,
        PageOptions {
            capture_console: true,
            allow_script_popups: true,
            developer_extras: true,
            ..PageOptions::default()
        },
    );
    let b = engine.new_page(&beta, PageOptions::default());
    let e = engine.new_page(&private, PageOptions::default());

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    for (page, h) in [(&a, 400), (&b, 220), (&e, 220)] {
        let w = page.widget();
        w.set_size_request(-1, h);
        w.set_vexpand(false);
        vbox.append(&w);
    }
    let window = gtk4::Window::builder()
        .title("copper-engine integration")
        .default_width(1280)
        .default_height(860)
        .child(&vbox)
        .build();
    window.present();

    let mut ctx = Ctx {
        srv,
        root: root.clone(),
        engine,
        alpha,
        beta,
        private,
        a: Tab::new(a),
        b: Tab::new(b),
        e: Tab::new(e),
        saw_favicon: false,
        bench: None,
    };
    let fails = glib::MainContext::default().block_on(async move {
        let mut fails = 0;
        for &name in CHECKS {
            let res = match glib::future_with_timeout(STEP, run_check(&mut ctx, name)).await {
                Ok(r) => r,
                Err(_) => Err(format!("timed out after {STEP:?}")),
            };
            match res {
                Ok(detail) => println!("PASS {name}: {detail}"),
                Err(why) => {
                    fails += 1;
                    println!("FAIL {name}: {why}");
                }
            }
        }
        println!(
            "integration: {}/{} checks passed",
            CHECKS.len() - fails,
            CHECKS.len()
        );
        fails
    });
    let _ = std::fs::remove_dir_all(&root);
    std::process::exit(if fails == 0 { 0 } else { 1 });
}

fn run_check<'a>(ctx: &'a mut Ctx, name: &str) -> Pin<Box<dyn Future<Output = Res> + 'a>> {
    match name {
        "profiles" => Box::pin(check_profiles(ctx)),
        "load-events" => Box::pin(check_load_events(ctx)),
        "favicon" => Box::pin(check_favicon(ctx)),
        "eval" => Box::pin(check_eval(ctx)),
        "isolated-world" => Box::pin(check_isolated(ctx)),
        "call-async" => Box::pin(check_call_async(ctx)),
        "script-message" => Box::pin(check_message(ctx)),
        "console" => Box::pin(check_console(ctx)),
        "snapshot" => Box::pin(check_snapshot(ctx)),
        "find-zoom" => Box::pin(check_find_zoom(ctx)),
        "user-content" => Box::pin(check_user_content(ctx)),
        "history" => Box::pin(check_history(ctx)),
        "cookies" => Box::pin(check_cookies(ctx)),
        "website-data" => Box::pin(check_website_data(ctx)),
        "content-filter" => Box::pin(check_filter(ctx)),
        "input" => Box::pin(check_input(ctx)),
        "new-window" => Box::pin(check_new_window(ctx)),
        "popup-handler" => Box::pin(check_popup_handler(ctx)),
        "download" => Box::pin(check_download(ctx)),
        "download-cancel" => Box::pin(check_download_cancel(ctx)),
        "hard-reload" => Box::pin(check_hard_reload(ctx)),
        "permission" => Box::pin(check_permission(ctx)),
        "crash" => Box::pin(check_crash(ctx)),
        other => backstage::run(ctx, other),
    }
}

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

/// Poll `code` in the page world until it returns `want`.
async fn poll_js(tab: &Tab, code: &str, want: &JsonValue) -> Result<(), String> {
    loop {
        if tab.js(code).await? == *want {
            return Ok(());
        }
        sleep(25).await;
    }
}

/// A 16x16 opaque PNG for the favicon.
fn make_icon() -> Vec<u8> {
    let mut px = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16u8 {
        for x in 0..16u8 {
            px.extend_from_slice(&[0xe8, x * 16, y * 16, 0xff]);
        }
    }
    let tex = gdk4::MemoryTexture::new(
        16,
        16,
        gdk4::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from_owned(px),
        16 * 4,
    );
    tex.save_to_png_bytes().to_vec()
}

/// Count distinct pixel values (capped) in a PNG to tell a page from a blank.
fn distinct_colors(png: &[u8]) -> Result<usize, String> {
    let tex = gdk4::Texture::from_bytes(&glib::Bytes::from(png)).map_err(|e| e.to_string())?;
    let (w, h) = (tex.width() as usize, tex.height() as usize);
    let stride = w * 4;
    let mut buf = vec![0u8; stride * h];
    tex.download(&mut buf, stride);
    let mut seen = std::collections::HashSet::new();
    for px in buf.as_chunks::<4>().0.iter().step_by(7) {
        seen.insert(*px);
        if seen.len() > 1000 {
            break;
        }
    }
    Ok(seen.len())
}

async fn check_profiles(ctx: &mut Ctx) -> Res {
    let dir = ctx
        .alpha
        .data_dir()
        .ok_or("alpha has no data dir")?
        .to_path_buf();
    ensure(
        dir == ctx.root.join("data/profiles/alpha") && dir.is_dir(),
        || format!("alpha data dir {}", dir.display()),
    )?;
    ensure(
        ctx.private.is_ephemeral() && ctx.private.data_dir().is_none(),
        || "private profile is not ephemeral".into(),
    )?;
    ensure(!ctx.beta.is_ephemeral(), || "beta is ephemeral".into())?;
    let again = ctx
        .engine
        .profile(&ProfileId::new("alpha").unwrap(), false)
        .map_err(|e| e.to_string())?;
    ensure(again.data_dir() == ctx.alpha.data_dir(), || {
        "profile cache miss".into()
    })?;
    ensure(ProfileId::new("../evil").is_err(), || {
        "bad id accepted".into()
    })?;
    Ok(format!(
        "alpha at {}, beta persistent, private ephemeral",
        dir.display()
    ))
}

async fn check_load_events(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/");
    let tab = &mut ctx.a;
    tab.drain();
    tab.page.load(&url);
    let mut seen = Vec::new();
    let mut raw = Vec::new();
    let mut saw_favicon = false;
    tab.wait(|e| {
        raw.push(format!("{e:?}"));
        match e {
            PageEvent::Load(LoadState::Started { .. }) => seen.push("started"),
            PageEvent::Load(LoadState::Committed { .. }) => seen.push("committed"),
            PageEvent::Title(Some(_)) => seen.push("title"),
            PageEvent::Url(Some(_)) => seen.push("url"),
            PageEvent::Progress(p) if *p >= 1.0 => seen.push("progress=1"),
            PageEvent::Favicon(Some(_)) => saw_favicon = true,
            _ => {}
        }
        matches!(e, PageEvent::Load(LoadState::Finished { .. })).then_some(())
    })
    .await?;
    ctx.saw_favicon = saw_favicon;
    // WebKit may deliver the title just after the load finishes.
    if !seen.contains(&"title") {
        tab.wait(|e| matches!(e, PageEvent::Title(Some(_))).then_some(()))
            .await?;
        seen.push("title(after finished)");
    }
    let title = tab.page.title().unwrap_or_default();
    let cur = tab.page.url().unwrap_or_default();
    for want in ["started", "committed", "url"] {
        ensure(seen.contains(&want), || {
            format!("no {want} event; saw {seen:?} raw {raw:?}")
        })?;
    }
    ensure(title == "Copper engine test" && cur == url, || {
        format!("title={title:?} url={cur}")
    })?;
    ensure(!tab.page.is_loading(), || "still loading".into())?;
    Ok(format!("events {seen:?}; title={title:?}"))
}

async fn check_favicon(ctx: &mut Ctx) -> Res {
    if !ctx.saw_favicon {
        ctx.a
            .wait(|e| matches!(e, PageEvent::Favicon(Some(_))).then_some(()))
            .await?;
    }
    Ok("favicon event with an image".into())
}

async fn check_eval(ctx: &mut Ctx) -> Res {
    let tab = &ctx.a;
    let v = tab
        .js("({sum: [1,2,3].reduce((a,b)=>a+b), title: document.title, nested: {ok: true}})")
        .await?;
    ensure(
        v["sum"] == 6 && v["title"] == "Copper engine test" && v["nested"]["ok"] == true,
        || format!("unexpected {v}"),
    )?;
    let undef = tab.js("undefined").await?;
    ensure(undef.is_null(), || format!("undefined → {undef}"))?;
    let s = tab.js("'a\"b'").await?;
    ensure(s == "a\"b", || format!("string → {s}"))?;
    let err = tab.page.eval("throw new Error('boom')", &World::Page).await;
    ensure(
        matches!(&err, Err(EngineError::JavaScript(m)) if m.contains("boom")),
        || format!("throw → {err:?}"),
    )?;
    Ok(format!("{v}; undefined→null; throw→JavaScript error"))
}

async fn check_isolated(ctx: &mut Ctx) -> Res {
    let tab = &ctx.a;
    let iso = World::isolated("copper");
    tab.js("window.pageSecret = 42; true").await?;
    let seen = tab.js_in(&iso, "typeof window.pageSecret").await?;
    let title = tab.js_in(&iso, "document.title").await?;
    tab.js_in(&iso, "window.isoOnly = 1; true").await?;
    let back = tab.js("typeof window.isoOnly").await?;
    let kept = tab.js_in(&iso, "window.isoOnly").await?;
    ensure(kept == 1, || {
        format!("isolated global not kept between calls: {kept}")
    })?;
    ensure(
        seen == "undefined" && back == "undefined" && title == "Copper engine test",
        || format!("isolated saw pageSecret as {seen}, page saw isoOnly as {back}, title {title}"),
    )?;
    Ok("globals isolated both ways, DOM shared".into())
}

async fn check_call_async(ctx: &mut Ctx) -> Res {
    let tab = &ctx.a;
    let mut args = JsonMap::new();
    args.insert("a".into(), 1.into());
    args.insert("b".into(), serde_json::json!({"n": 2}));
    args.insert("s".into(), "quote\" and \u{2028}".into());
    args.insert("list".into(), serde_json::json!([1, 2, 3]));
    args.insert("z".into(), JsonValue::Null);
    let body = "await new Promise(r => setTimeout(r, 5));\n\
                return {sum: a + b.n, s, len: list.length, nil: z === null, path: location.pathname};";
    let v = tab
        .page
        .call_async(body, &args, &World::isolated("copper"))
        .await
        .map_err(|e| e.to_string())?;
    ensure(
        v["sum"] == 3
            && v["s"] == "quote\" and \u{2028}"
            && v["len"] == 3
            && v["nil"] == true
            && v["path"] == "/",
        || format!("unexpected {v}"),
    )?;
    let none = tab
        .page
        .call_async("return 7;", &JsonMap::new(), &World::Page)
        .await
        .map_err(|e| e.to_string())?;
    ensure(none == 7, || format!("no-arg call → {none}"))?;
    let mut bad = JsonMap::new();
    bad.insert("a;alert(1)".into(), 1.into());
    let r = tab.page.call_async("return 1;", &bad, &World::Page).await;
    ensure(matches!(r, Err(EngineError::InvalidArgument(_))), || {
        format!("bad arg name → {r:?}")
    })?;
    let rejected = tab
        .page
        .call_async("throw new Error('nope');", &JsonMap::new(), &World::Page)
        .await;
    ensure(matches!(rejected, Err(EngineError::JavaScript(_))), || {
        format!("rejection → {rejected:?}")
    })?;
    Ok(format!("{v}"))
}

async fn check_message(ctx: &mut Ctx) -> Res {
    let tab = &mut ctx.a;
    let iso = World::isolated("copper");
    ensure(tab.page.add_message_handler("copper", &iso), || {
        "add failed".into()
    })?;
    ensure(!tab.page.add_message_handler("copper", &iso), || {
        "duplicate add returned true".into()
    })?;
    ensure(
        tab.page.add_message_handler("pagemsg", &World::Page),
        || "add pagemsg failed".into(),
    )?;
    tab.drain();
    tab.js_in(
        &iso,
        "window.webkit.messageHandlers.copper.postMessage({kind: 'hello', n: 42}); true",
    )
    .await?;
    let body = tab
        .wait(|e| match e {
            PageEvent::ScriptMessage { name, body } if name == "copper" => Some(body.clone()),
            _ => None,
        })
        .await?;
    ensure(body["n"] == 42 && body["kind"] == "hello", || {
        format!("body {body}")
    })?;
    let visible = tab
        .js("!!(window.webkit.messageHandlers && window.webkit.messageHandlers.copper)")
        .await?;
    ensure(visible == false, || {
        "isolated handler visible to page world".into()
    })?;
    tab.js("window.webkit.messageHandlers.pagemsg.postMessage([1, 'two']); true")
        .await?;
    let body2 = tab
        .wait(|e| match e {
            PageEvent::ScriptMessage { name, body } if name == "pagemsg" => Some(body.clone()),
            _ => None,
        })
        .await?;
    ensure(body2 == serde_json::json!([1, "two"]), || {
        format!("pagemsg {body2}")
    })?;
    tab.page.remove_message_handler("pagemsg", &World::Page);
    let gone = tab
        .js("typeof (window.webkit.messageHandlers.pagemsg)")
        .await?;
    ensure(gone == "undefined", || {
        format!("pagemsg still there: {gone}")
    })?;
    Ok(format!("isolated {body}, page {body2}"))
}

async fn check_console(ctx: &mut Ctx) -> Res {
    let tab = &mut ctx.a;
    tab.drain();
    tab.js("console.log('hello', {a: 1}); console.warn('careful'); true")
        .await?;
    let mut got = Vec::new();
    tab.wait(|e| {
        if let PageEvent::Console(m) = e {
            got.push(m.clone());
        }
        (got.len() >= 2).then_some(())
    })
    .await?;
    ensure(
        got[0].level == ConsoleLevel::Log
            && got[0].message == r#"hello {"a":1}"#
            && got[1].level == ConsoleLevel::Warn
            && got[1].message == "careful",
        || format!("{got:?}"),
    )?;
    Ok(format!("{got:?}"))
}

async fn check_snapshot(ctx: &mut Ctx) -> Res {
    let tab = &ctx.a;
    sleep(200).await;
    let vis = tab
        .page
        .snapshot(SnapshotRegion::Visible)
        .await
        .map_err(|e| e.to_string())?;
    let full = tab
        .page
        .snapshot(SnapshotRegion::FullDocument)
        .await
        .map_err(|e| e.to_string())?;
    let cv = distinct_colors(&vis.png)?;
    let cf = distinct_colors(&full.png)?;
    ensure(cv >= 4 && cf >= 4, || {
        format!("blank? visible {cv} colors, full {cf}")
    })?;
    ensure(full.height > vis.height, || {
        format!(
            "full {}x{} not taller than visible {}x{}",
            full.width, full.height, vis.width, vis.height
        )
    })?;
    ensure(vis.png.starts_with(b"\x89PNG"), || "not a PNG".into())?;
    let out = std::env::temp_dir().join("copper-engine-snapshot.png");
    let _ = std::fs::write(&out, &vis.png);
    Ok(format!(
        "visible {}x{} ({cv} colors), full {}x{} ({cf} colors), saved {}",
        vis.width,
        vis.height,
        full.width,
        full.height,
        out.display()
    ))
}

async fn check_find_zoom(ctx: &mut Ctx) -> Res {
    let tab = &ctx.a;
    let n = tab
        .page
        .find("needle", FindOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    let none = tab
        .page
        .find("zzzz-not-here", FindOptions::default())
        .await
        .map_err(|e| e.to_string())?;
    tab.page.find_finish();
    ensure(n == 3 && none == 0, || format!("needle={n} missing={none}"))?;
    tab.page.set_zoom(1.5);
    let z = tab.page.zoom();
    tab.page.set_zoom(1.0);
    ensure((z - 1.5).abs() < 1e-9, || format!("zoom {z}"))?;
    Ok(format!(
        "find needle={n}, missing={none}; zoom 1.5 round-trips"
    ))
}

async fn check_user_content(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/");
    let tab = &mut ctx.b;
    let iso = World::isolated("copper");
    tab.page.add_message_handler("early", &iso);
    let s1 = tab.page.add_user_script(
        &UserScript::new(
            "window.webkit.messageHandlers.early.postMessage({path: location.pathname, ready: document.readyState});",
        )
        .at(InjectionTime::DocumentStart)
        .world(iso.clone()),
    );
    let s2 = tab.page.add_user_script(
        &UserScript::new("window.__early = document.readyState;").at(InjectionTime::DocumentStart),
    );
    let s3 = tab
        .page
        .add_user_script(&UserScript::new("window.__late = document.readyState;"));
    let css = tab.page.add_user_style_sheet(
        &UserStyleSheet::new("h1 { color: rgb(1, 2, 3) !important; }").level(StyleLevel::User),
    );
    tab.drain();
    tab.page.load(&url);
    let msg = tab
        .wait(|e| match e {
            PageEvent::ScriptMessage { name, body } if name == "early" => Some(body.clone()),
            _ => None,
        })
        .await?;
    tab.finished().await?;
    let early = tab.js("window.__early").await?;
    let late = tab.js("window.__late").await?;
    let color = tab
        .js("getComputedStyle(document.querySelector('h1')).color")
        .await?;
    ensure(msg["ready"] == "loading" && msg["path"] == "/", || {
        format!("message {msg}")
    })?;
    ensure(
        early == "loading" && late != "loading" && !late.is_null(),
        || format!("early={early} late={late}"),
    )?;
    ensure(color == "rgb(1, 2, 3)", || format!("h1 color {color}"))?;
    ensure(
        tab.page.remove_user_script(s1)
            && tab.page.remove_user_script(s2)
            && tab.page.remove_user_script(s3)
            && tab.page.remove_user_style_sheet(css)
            && !tab.page.remove_user_script(s1),
        || "remove returned the wrong value".into(),
    )?;
    tab.load(&url).await?;
    let color2 = tab
        .js("getComputedStyle(document.querySelector('h1')).color")
        .await?;
    let early2 = tab.js("window.__early === undefined").await?;
    ensure(color2 != "rgb(1, 2, 3)" && early2 == true, || {
        format!("after removal color={color2} early-undefined={early2}")
    })?;
    tab.page.remove_message_handler("early", &iso);
    Ok(format!(
        "document-start isolated script posted {msg}; page early={early} late={late}; css applied then removed"
    ))
}

async fn check_history(ctx: &mut Ctx) -> Res {
    let (u1, u2) = (ctx.srv.url("/"), ctx.srv.url("/page2"));
    let tab = &mut ctx.a;
    tab.load(&u1).await?;
    tab.load(&u2).await?;
    ensure(tab.page.can_go_back() && !tab.page.can_go_forward(), || {
        "history flags wrong after two loads".into()
    })?;
    tab.page.back();
    while tab.page.url().as_deref() != Some(u1.as_str()) || tab.page.is_loading() {
        sleep(20).await;
    }
    ensure(tab.page.can_go_forward(), || {
        "cannot go forward after back".into()
    })?;
    tab.page.forward();
    while tab.page.url().as_deref() != Some(u2.as_str()) || tab.page.is_loading() {
        sleep(20).await;
    }
    tab.drain();
    tab.page.reload(true);
    tab.finished().await?;
    tab.drain();
    tab.page.reload(false);
    tab.finished().await?;
    while tab.page.title().as_deref() != Some("Page two") {
        sleep(20).await;
    }
    tab.page.load(&ctx.srv.url("/slow.bin?stop"));
    tab.page.stop();
    Ok("back, forward, reload(bypass), reload, stop".into())
}

fn names(cookies: &[Cookie]) -> Vec<String> {
    let mut v: Vec<String> = cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect();
    v.sort();
    v
}

async fn check_cookies(ctx: &mut Ctx) -> Res {
    let base = ctx.srv.url("/");
    ctx.a.load(&ctx.srv.url("/setcookie?v=alpha")).await?;
    ctx.b.load(&ctx.srv.url("/setcookie?v=beta")).await?;
    ctx.e.load(&ctx.srv.url("/setcookie?v=private")).await?;
    let ja = ctx.a.js("document.cookie").await?;
    let jb = ctx.b.js("document.cookie").await?;
    let je = ctx.e.js("document.cookie").await?;
    let ca = names(&ctx.alpha.cookies(&base).await.map_err(|e| e.to_string())?);
    let cb = names(&ctx.beta.cookies(&base).await.map_err(|e| e.to_string())?);
    let ce = names(
        &ctx.private
            .cookies(&base)
            .await
            .map_err(|e| e.to_string())?,
    );
    ensure(
        ja == "who=alpha"
            && jb == "who=beta"
            && je == "who=private"
            && ca == ["who=alpha"]
            && cb == ["who=beta"]
            && ce == ["who=private"],
        || format!("js a={ja} b={jb} e={je}; store a={ca:?} b={cb:?} e={ce:?}"),
    )?;

    let manual = Cookie::new("manual", "1", "127.0.0.1");
    ctx.beta
        .set_cookie(&manual)
        .await
        .map_err(|e| e.to_string())?;
    let jb2 = ctx.b.js("document.cookie").await?;
    let ja2 = ctx.a.js("document.cookie").await?;
    let all_b = names(&ctx.beta.all_cookies().await.map_err(|e| e.to_string())?);
    ensure(
        jb2.as_str().is_some_and(|s| s.contains("manual=1")) && ja2 == "who=alpha",
        || format!("after set_cookie: beta js={jb2} alpha js={ja2}"),
    )?;
    ensure(all_b == ["manual=1", "who=beta"], || {
        format!("beta all {all_b:?}")
    })?;
    ctx.beta
        .delete_cookie(&manual)
        .await
        .map_err(|e| e.to_string())?;
    let cb2 = names(&ctx.beta.cookies(&base).await.map_err(|e| e.to_string())?);
    ensure(cb2 == ["who=beta"], || format!("after delete {cb2:?}"))?;
    Ok(format!(
        "alpha {ca:?} / beta {cb:?} / private {ce:?}; set+delete on beta only"
    ))
}

async fn check_website_data(ctx: &mut Ctx) -> Res {
    let base = ctx.srv.url("/");
    let removed = ctx
        .beta
        .clear_website_data(Some("127.0.0.1"), WebsiteDataKinds::COOKIES)
        .await
        .map_err(|e| e.to_string())?;
    let cb = names(&ctx.beta.cookies(&base).await.map_err(|e| e.to_string())?);
    let ca = names(&ctx.alpha.cookies(&base).await.map_err(|e| e.to_string())?);
    let other = ctx
        .beta
        .clear_website_data(Some("example.org"), WebsiteDataKinds::ALL)
        .await
        .map_err(|e| e.to_string())?;
    ensure(
        cb.is_empty() && ca == ["who=alpha"] && !removed.is_empty() && other.is_empty(),
        || format!("removed {removed:?}; beta now {cb:?}; alpha {ca:?}; other {other:?}"),
    )?;
    Ok(format!(
        "cleared cookies for {removed:?} in beta; alpha untouched"
    ))
}

async fn check_filter(ctx: &mut Ctx) -> Res {
    let filter = ctx
        .engine
        .compile_content_filter("test-filter", FILTER_RULES)
        .await
        .map_err(|e| e.to_string())?;
    ensure(filter.id() == "test-filter", || "filter id".into())?;
    let loaded = ctx
        .engine
        .load_content_filter("test-filter")
        .await
        .map_err(|e| e.to_string())?;
    let missing = ctx
        .engine
        .load_content_filter("never-saved")
        .await
        .map_err(|e| e.to_string())?;
    ensure(loaded.is_some() && missing.is_none(), || {
        format!(
            "load: stored={} missing={}",
            loaded.is_some(),
            missing.is_some()
        )
    })?;
    let bad = ctx
        .engine
        .compile_content_filter("bad", "[{\"nope\": 1}]")
        .await;
    ensure(matches!(bad, Err(EngineError::ContentFilter(_))), || {
        format!("bad rules → {bad:?}")
    })?;
    ctx.b.page.set_content_filter(Some(&filter));
    ctx.srv.clear();
    ctx.b.load(&ctx.srv.url("/filter")).await?;
    sleep(300).await;
    let v = ctx
        .b
        .js(
            "({allowed: !!window.allowedRan, blocked: !!window.blockedRan, \
              ad: getComputedStyle(document.querySelector('.ad')).display})",
        )
        .await?;
    let (saw_blocked, saw_tracker, saw_allowed) = (
        ctx.srv.saw("blocked.js"),
        ctx.srv.saw("tracker.gif"),
        ctx.srv.saw("allowed.js"),
    );
    ctx.b.page.set_content_filter(None);
    ctx.engine
        .remove_content_filter("test-filter")
        .await
        .map_err(|e| e.to_string())?;
    let after = ctx
        .engine
        .load_content_filter("test-filter")
        .await
        .map_err(|e| e.to_string())?;
    ensure(
        v["allowed"] == true
            && v["blocked"] == false
            && v["ad"] == "none"
            && !saw_blocked
            && !saw_tracker
            && saw_allowed
            && after.is_none(),
        || {
            format!(
                "page {v}; server blocked={saw_blocked} tracker={saw_tracker} allowed={saw_allowed}; still stored={}",
                after.is_some()
            )
        },
    )?;
    Ok(format!(
        "blocked.js + tracker.gif never requested, .ad hidden: {v}"
    ))
}

async fn check_input(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/input");
    let tab = &mut ctx.a;
    tab.load(&url).await?;
    // Button at (20,20) 120x40.
    let trust = tab.input(InputEvent::click(80.0, 40.0)).await?;
    let log = tab.js("window.log").await?;
    ensure(
        log["clicks"] == 1 && log["trusted"] == true && trust == Trust::Trusted,
        || format!("button clicks {log}, {trust:?}"),
    )?;
    // Focus the field by clicking it, then type.
    tab.input(InputEvent::click(100.0, 95.0)).await?;
    let focused = tab.js("document.activeElement.id").await?;
    ensure(focused == "field", || format!("focused {focused}"))?;
    tab.input(InputEvent::Type("hello".into())).await?;
    tab.input(InputEvent::key("Backspace")).await?;
    let st = tab
        .js("({value: document.getElementById('field').value, inputs: log.inputs, keys: log.keys})")
        .await?;
    ensure(
        st["value"] == "hell"
            && st["inputs"] == 6
            && st["keys"] == serde_json::json!(["h", "e", "l", "l", "o", "Backspace"]),
        || format!("field {st}"),
    )?;
    // Checkbox at (20,130) 20x20.
    tab.input(InputEvent::click(30.0, 140.0)).await?;
    let checked = tab.js("document.getElementById('check').checked").await?;
    ensure(checked == true, || "checkbox not toggled".into())?;
    // Wheel-scroll inside the scroller at (300,20) 200x100.
    tab.input(InputEvent::Move {
        x: 400.0,
        y: 70.0,
        modifiers: Modifiers::NONE,
    })
    .await?;
    tab.input(InputEvent::Scroll { dx: 0.0, dy: 200.0 }).await?;
    let top = tab
        .js("document.getElementById('scroller').scrollTop")
        .await?;
    ensure(top == 200, || format!("scrollTop {top}"))?;
    // Drag across the pad at (20,170) 200x90.
    tab.js("log.drag = []; true").await?;
    tab.input(InputEvent::Drag {
        from_x: 40.0,
        from_y: 190.0,
        to_x: 150.0,
        to_y: 240.0,
        modifiers: Modifiers::NONE,
    })
    .await?;
    let drag = tab.js("log.drag").await?;
    let moves = drag
        .as_array()
        .map(|a| a.iter().filter(|e| e[0] == "move").count())
        .unwrap_or(0);
    ensure(
        drag[0] == serde_json::json!(["down", 40, 190])
            && moves >= 5
            && drag.as_array().and_then(|a| a.last()) == Some(&serde_json::json!(["up", 150, 240])),
        || format!("drag log {drag}"),
    )?;
    // Right click fires contextmenu, not click.
    tab.js("window.ctx = 0; document.addEventListener('contextmenu', e => { ctx++; e.preventDefault(); }); true")
        .await?;
    tab.input(InputEvent::Click {
        x: 80.0,
        y: 40.0,
        button: MouseButton::Right,
        count: 1,
        modifiers: Modifiers::NONE,
    })
    .await?;
    let after = tab.js("({clicks: log.clicks, ctx: window.ctx})").await?;
    ensure(after["clicks"] == 1 && after["ctx"] == 1, || {
        format!("right click {after}")
    })?;
    let miss = tab.page.input(InputEvent::click(-50.0, -50.0)).await;
    ensure(matches!(miss, Err(EngineError::NotFound(_))), || {
        format!("click outside → {miss:?}")
    })?;
    Ok(format!(
        "click, focus, type+backspace ({}), checkbox, wheel scroll {top}, drag {} moves, right-click; isTrusted={}",
        st["value"], moves, log["trusted"]
    ))
}

async fn check_new_window(ctx: &mut Ctx) -> Res {
    let tab = &mut ctx.a;
    tab.drain();
    tab.js("window.open('/page2', '_blank'); true").await?;
    let req = tab
        .wait(|e| match e {
            PageEvent::NewWindow(r) => Some(r.clone()),
            _ => None,
        })
        .await?;
    ensure(
        req.disposition == WindowDisposition::Popup
            && req.url.as_deref().is_some_and(|u| u.ends_with("/page2")),
        || format!("{req:?}"),
    )?;
    Ok(format!("{req:?}"))
}

async fn check_popup_handler(ctx: &mut Ctx) -> Res {
    use std::cell::RefCell;
    use std::rc::Rc;
    let slot: Rc<RefCell<Option<Tab>>> = Rc::default();
    let parent = ctx.a.page.clone();
    let s = slot.clone();
    ctx.a
        .page
        .set_new_window_handler(Some(Box::new(move |_req| {
            let popup = parent.new_related(PageOptions::default());
            *s.borrow_mut() = Some(Tab::new(popup.clone()));
            Some(popup)
        })));
    ctx.a.drain();
    ctx.a
        .js("window.__pop = window.open('/page2', 'pop'); true")
        .await?;
    while slot.borrow().is_none() {
        sleep(20).await;
    }
    ctx.a.page.set_new_window_handler(None);
    let mut popup = slot.borrow_mut().take().ok_or("no popup")?;
    while popup
        .page
        .url()
        .as_deref()
        .is_none_or(|u| !u.ends_with("/page2"))
        || popup.page.is_loading()
    {
        sleep(20).await;
    }
    let opener = popup.js("window.opener !== null && window.name").await?;
    let same_profile = names(
        &popup
            .page
            .profile()
            .cookies(&ctx.srv.url("/"))
            .await
            .map_err(|e| e.to_string())?,
    );
    let open = ctx.a.js("window.__pop && !window.__pop.closed").await?;
    ensure(opener == "pop" && open == true, || {
        format!("opener/name {opener}, parent sees open {open}")
    })?;
    // No NewWindow event when the handler took the popup.
    let stray =
        std::iter::from_fn(|| ctx.a.ev.try_next()).any(|e| matches!(e, PageEvent::NewWindow(_)));
    ensure(!stray, || "NewWindow emitted although handled".into())?;
    popup.drain();
    popup.js("window.close(); true").await?;
    popup
        .wait(|e| matches!(e, PageEvent::CloseRequested).then_some(()))
        .await?;
    Ok(format!(
        "related page has opener (name {opener}), shares alpha cookies {same_profile:?}, close requested"
    ))
}

/// Wait on a profile download stream until `id` finishes or fails.
async fn download_outcome(
    stream: &mut EventStream<DownloadEvent>,
    id: DownloadId,
) -> Result<Vec<DownloadState>, String> {
    let mut states = Vec::new();
    loop {
        let ev = stream.next().await.ok_or("download stream ended")?;
        if ev.id != id {
            continue;
        }
        let done = matches!(
            ev.state,
            DownloadState::Finished { .. } | DownloadState::Failed { .. }
        );
        states.push(ev.state);
        if done {
            return Ok(states);
        }
    }
}

async fn check_download(ctx: &mut Ctx) -> Res {
    let dir = ctx.root.join("downloads");
    ensure(ctx.beta.download_dir() == dir, || "download dir".into())?;
    let mut prof_stream = ctx.beta.downloads();
    ctx.b.drain();
    let id = ctx
        .b
        .page
        .download(&ctx.srv.url("/file.bin"))
        .map_err(|e| e.to_string())?;
    let states = download_outcome(&mut prof_stream, id).await?;
    let Some(DownloadState::Finished { path, bytes }) = states.last().cloned() else {
        return Err(format!("{states:?}"));
    };
    ensure(
        matches!(states.first(), Some(DownloadState::Started { .. }))
            && states
                .iter()
                .any(|s| matches!(s, DownloadState::Destination { .. })),
        || format!("states {states:?}"),
    )?;
    let body = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    ensure(
        path == dir.join("copper-test.bin")
            && body == server::DOWNLOAD_BODY
            && bytes == body.len() as u64,
        || format!("{} ({bytes} bytes)", path.display()),
    )?;
    // The page saw the same download.
    let page_saw = std::iter::from_fn(|| ctx.b.ev.try_next()).any(|e| {
        matches!(e, PageEvent::Download(DownloadEvent { id: i, state: DownloadState::Finished { .. } }) if i == id)
    });
    ensure(page_saw, || "page did not get the download events".into())?;

    // A navigation to an attachment becomes a download in the page's profile.
    let mut alpha_stream = ctx.alpha.downloads();
    ctx.a.drain();
    ctx.a.page.load(&ctx.srv.url("/file.bin"));
    let (nav_id, nav_path) = loop {
        let ev = alpha_stream.next().await.ok_or("alpha stream ended")?;
        match ev.state {
            DownloadState::Finished { path, .. } => break (ev.id, path),
            DownloadState::Failed { error, .. } => {
                return Err(format!("nav download failed: {error}"));
            }
            _ => {}
        }
    };
    ensure(nav_path == dir.join("copper-test (1).bin"), || {
        format!("second download at {}", nav_path.display())
    })?;
    let mut page_events = Vec::new();
    while let Some(e) = ctx.a.ev.try_next() {
        page_events.push(e);
    }
    let nav_cancelled = page_events.iter().any(|e| {
        matches!(
            e,
            PageEvent::Load(LoadState::Failed {
                cancelled: true,
                ..
            })
        )
    });
    let page_dl = page_events
        .iter()
        .any(|e| matches!(e, PageEvent::Download(DownloadEvent { id: i, .. }) if *i == nav_id));
    ensure(page_dl, || format!("page events {page_events:?}"))?;
    Ok(format!(
        "{} ({bytes} B) via page.download; navigation download → {} (load cancelled={nav_cancelled})",
        path.display(),
        nav_path.display()
    ))
}

async fn check_download_cancel(ctx: &mut Ctx) -> Res {
    let mut stream = ctx.alpha.downloads();
    let id = ctx
        .alpha
        .download(&ctx.srv.url("/slow.bin"))
        .map_err(|e| e.to_string())?;
    let mut states = Vec::new();
    let mut cancelled_at = None;
    loop {
        let ev = stream.next().await.ok_or("stream ended")?;
        if ev.id != id {
            continue;
        }
        match &ev.state {
            DownloadState::Progress { received, .. } if cancelled_at.is_none() => {
                cancelled_at = Some(*received);
                ensure(ctx.alpha.cancel_download(id), || {
                    "cancel returned false".into()
                })?;
            }
            DownloadState::Finished { .. } => return Err("slow download finished".into()),
            DownloadState::Failed { .. } => {
                states.push(ev.state);
                break;
            }
            _ => {}
        }
        states.push(ev.state);
    }
    let Some(DownloadState::Failed { cancelled, error }) = states.last() else {
        return Err(format!("{states:?}"));
    };
    ensure(*cancelled, || format!("failed but not cancelled: {error}"))?;
    ensure(!ctx.alpha.cancel_download(id), || {
        "cancel after end returned true".into()
    })?;
    Ok(format!(
        "cancelled after {} bytes: {error}",
        cancelled_at.unwrap_or(0)
    ))
}

async fn check_hard_reload(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/");
    ctx.a.load(&url).await?;
    ctx.a.drain();
    ctx.srv.clear();
    let cleared = ctx.a.page.hard_reload().await.map_err(|e| e.to_string())?;
    ctx.a.finished().await?;
    ensure(ctx.srv.saw("/"), || "no request reached the server".into())?;
    let cookie = ctx.a.js("document.cookie").await?;
    ensure(cookie == "who=alpha", || {
        format!("cookies touched: {cookie}")
    })?;
    Ok(format!(
        "cleared caches for {cleared:?}, reloaded from origin, cookies kept"
    ))
}

async fn check_permission(ctx: &mut Ctx) -> Res {
    let tab = &mut ctx.a;
    tab.drain();
    tab.js("window.__perm = 'pending'; Notification.requestPermission().then(p => window.__perm = p); true")
        .await?;
    let (id, kind) = tab
        .wait(|e| match e {
            PageEvent::PermissionRequest { id, kind, .. } => Some((*id, kind.clone())),
            _ => None,
        })
        .await?;
    ensure(kind == PermissionKind::Notifications, || {
        format!("kind {kind:?}")
    })?;
    ensure(tab.page.answer_permission(id, true), || {
        "answer returned false".into()
    })?;
    ensure(!tab.page.answer_permission(id, true), || {
        "second answer returned true".into()
    })?;
    poll_js(tab, "window.__perm", &"granted".into()).await?;
    Ok(format!("{kind:?} request {id} allowed → granted"))
}

async fn check_crash(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/");
    let tab = &mut ctx.e;
    tab.drain();
    tab.page.terminate_web_process();
    let reason = tab
        .wait(|e| match e {
            PageEvent::Crashed(r) => Some(*r),
            _ => None,
        })
        .await?;
    ensure(reason == CrashReason::TerminatedByApi, || {
        format!("{reason:?}")
    })?;
    tab.load(&url).await?;
    let title = tab.js("document.title").await?;
    ensure(title == "Copper engine test", || {
        format!("after reload {title}")
    })?;
    Ok(format!("{reason:?}, page reloads after"))
}
