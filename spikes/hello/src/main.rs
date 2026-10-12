//! T-001 spike: WebKitGTK 6.0 from Rust shows a page and does the risky calls.
//!
//! `hello [URL]`        opens a window showing URL (default: the built-in test page).
//! `hello --selftest`   runs the nine T-001 checks against a tiny local HTTP
//!                      server, prints one PASS/FAIL line per check and exits 0
//!                      only if all nine pass. A watchdog thread kills the
//!                      process after `WATCHDOG` so it can never hang.
//!
//! Throwaway code: it proves the API surface for T-005, nothing more.

mod server;

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gdk4::prelude::*;
use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, NetworkSession, SnapshotOptions, SnapshotRegion, UserContentFilterStore,
    UserContentManager, WebView,
};

/// Per-check timeout.
const STEP: Duration = Duration::from_secs(20);
/// Whole-process watchdog.
const WATCHDOG: Duration = Duration::from_secs(150);

const CHECKS: [&str; 9] = [
    "load-url",
    "eval-js-json",
    "script-message-handler",
    "content-filter",
    "ephemeral-session-cookies",
    "snapshot-visible",
    "snapshot-full-document",
    "download",
    "inspector-open",
];

/// Three-rule WebKit content-blocker list (same JSON shape Shield.swift emits).
const FILTER_RULES: &str = r#"[
  {"trigger": {"url-filter": "blocked\\.js"}, "action": {"type": "block"}},
  {"trigger": {"url-filter": "tracker\\.gif"}, "action": {"type": "block"}},
  {"trigger": {"url-filter": ".*"}, "action": {"type": "css-display-none", "selector": ".ad"}}
]"#;

type Res = Result<String, String>;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let selftest = args.iter().any(|a| a == "--selftest");

    let srv = server::start().expect("start local http server");
    gtk4::init().expect("gtk init (is WAYLAND_DISPLAY/DISPLAY set?)");

    if selftest {
        std::thread::spawn(|| {
            std::thread::sleep(WATCHDOG);
            eprintln!("hello: watchdog fired after {WATCHDOG:?}; aborting");
            std::process::exit(3);
        });
    }

    let work = std::env::temp_dir().join(format!("copper-hello-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("create work dir");

    // A persistent (but throwaway) session for the "normal" profile.
    let normal_session = NetworkSession::new(
        Some(work.join("data").to_str().unwrap()),
        Some(work.join("cache").to_str().unwrap()),
    );
    let ucm = UserContentManager::new();
    let view = WebView::builder()
        .network_session(&normal_session)
        .user_content_manager(&ucm)
        .build();
    view.set_vexpand(true);
    if let Some(s) = WebViewExt::settings(&view) {
        s.set_enable_developer_extras(true);
    }

    // The private view: ephemeral session, its own content manager.
    let private_session = NetworkSession::new_ephemeral();
    let private_view = WebView::builder().network_session(&private_session).build();
    private_view.set_size_request(-1, 120);

    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.append(&view);
    vbox.append(&private_view);
    let window = gtk4::Window::builder()
        .title("copper hello spike")
        .default_width(1024)
        .default_height(768)
        .child(&vbox)
        .build();
    window.present();

    let main_loop = glib::MainLoop::new(None, false);
    if !selftest {
        let url = args
            .first()
            .cloned()
            .unwrap_or_else(|| format!("{}/", srv.base));
        view.load_uri(&url);
        let ml = main_loop.clone();
        window.connect_close_request(move |_| {
            ml.quit();
            glib::Propagation::Proceed
        });
        main_loop.run();
        return;
    }

    let ctx = Ctx {
        base: srv.base.clone(),
        hits: srv.hits.clone(),
        view,
        private_view,
        normal_session,
        private_session,
        ucm,
        work,
    };
    glib::MainContext::default().spawn_local(async move {
        let mut fails = 0;
        for name in CHECKS {
            let res = match glib::future_with_timeout(STEP, run_check(&ctx, name)).await {
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
            "hello selftest: {}/{} passed",
            CHECKS.len() - fails,
            CHECKS.len()
        );
        std::process::exit(if fails == 0 { 0 } else { 1 });
    });
    main_loop.run();
}

struct Ctx {
    base: String,
    hits: server::Hits,
    view: WebView,
    private_view: WebView,
    normal_session: NetworkSession,
    private_session: NetworkSession,
    ucm: UserContentManager,
    work: PathBuf,
}

fn run_check<'a>(ctx: &'a Ctx, name: &str) -> std::pin::Pin<Box<dyn Future<Output = Res> + 'a>> {
    match name {
        "load-url" => Box::pin(check_load(ctx)),
        "eval-js-json" => Box::pin(check_eval(ctx)),
        "script-message-handler" => Box::pin(check_message(ctx)),
        "content-filter" => Box::pin(check_filter(ctx)),
        "ephemeral-session-cookies" => Box::pin(check_cookies(ctx)),
        "snapshot-visible" => Box::pin(check_snapshot(ctx, SnapshotRegion::Visible)),
        "snapshot-full-document" => Box::pin(check_snapshot(ctx, SnapshotRegion::FullDocument)),
        "download" => Box::pin(check_download(ctx)),
        "inspector-open" => Box::pin(check_inspector(ctx)),
        _ => unreachable!(),
    }
}

/// Poll a condition on the main loop (spike-grade; T-005 will use signals → channels).
async fn wait_until(mut cond: impl FnMut() -> bool) {
    while !cond() {
        glib::timeout_future(Duration::from_millis(20)).await;
    }
}

/// Load a URL and wait for LoadEvent::Finished (or load-failed).
async fn load(view: &WebView, url: &str) -> Result<(), String> {
    let done = Rc::new(Cell::new(false));
    let failed: Rc<RefCell<Option<String>>> = Rc::default();
    let d = done.clone();
    let h1 = view.connect_load_changed(move |_, ev| {
        if ev == LoadEvent::Finished {
            d.set(true);
        }
    });
    let f = failed.clone();
    let h2 = view.connect_load_failed(move |_, _, uri, err| {
        *f.borrow_mut() = Some(format!("{uri}: {err}"));
        false
    });
    view.load_uri(url);
    wait_until(|| done.get()).await;
    view.disconnect(h1);
    view.disconnect(h2);
    match failed.take() {
        Some(e) => Err(format!("load failed: {e}")),
        None => Ok(()),
    }
}

async fn eval_json(view: &WebView, js: &str) -> Result<serde_json::Value, String> {
    let v = view
        .evaluate_javascript_future(js, None, None)
        .await
        .map_err(|e| format!("evaluate_javascript: {e}"))?;
    let s = v.to_json(0).ok_or("value has no JSON form")?;
    serde_json::from_str(&s).map_err(|e| format!("bad JSON {s:?}: {e}"))
}

async fn check_load(ctx: &Ctx) -> Res {
    let url = format!("{}/", ctx.base);
    load(&ctx.view, &url).await?;
    let title = ctx.view.title().unwrap_or_default();
    let uri = ctx.view.uri().unwrap_or_default();
    if title != "Copper spike" {
        return Err(format!("unexpected title {title:?}"));
    }
    Ok(format!("uri={uri} title={title:?}"))
}

async fn check_eval(ctx: &Ctx) -> Res {
    let v = eval_json(
        &ctx.view,
        "({title: document.title, height: document.documentElement.scrollHeight, \
          sum: [1,2,3].reduce((a,b)=>a+b), nested: {ok: true}})",
    )
    .await?;
    if v["sum"] != 6 || v["nested"]["ok"] != true || v["title"] != "Copper spike" {
        return Err(format!("unexpected value {v}"));
    }
    // call_async_javascript_function: async body with an await, result as JSON.
    let v2 = ctx
        .view
        .call_async_javascript_function_future(
            "await new Promise(r => setTimeout(r, 10)); return {later: location.pathname};",
            None,
            None,
            None,
        )
        .await
        .map_err(|e| format!("call_async_javascript_function: {e}"))?;
    let later = v2.to_json(0).unwrap_or_default();
    Ok(format!("{v} async={later}"))
}

async fn check_message(ctx: &Ctx) -> Res {
    let got: Rc<RefCell<Option<String>>> = Rc::default();
    let g = got.clone();
    if !ctx.ucm.register_script_message_handler("copper", None) {
        return Err("register_script_message_handler returned false".into());
    }
    let h = ctx
        .ucm
        .connect_script_message_received(Some("copper"), move |_, value| {
            *g.borrow_mut() = value.to_json(0).map(|s| s.to_string());
        });
    ctx.view
        .evaluate_javascript_future(
            "window.webkit.messageHandlers.copper.postMessage({kind: 'hello', n: 42}); true",
            None,
            None,
        )
        .await
        .map_err(|e| format!("post: {e}"))?;
    wait_until(|| got.borrow().is_some()).await;
    ctx.ucm.disconnect(h);
    let msg = got.take().unwrap();
    let v: serde_json::Value = serde_json::from_str(&msg).map_err(|e| e.to_string())?;
    if v["n"] != 42 {
        return Err(format!("unexpected message {msg}"));
    }
    Ok(format!("received {msg}"))
}

async fn check_filter(ctx: &Ctx) -> Res {
    let store = UserContentFilterStore::new(ctx.work.join("filters").to_str().unwrap());
    let filter = store
        .save_future(
            "copper-spike",
            &glib::Bytes::from_static(FILTER_RULES.as_bytes()),
        )
        .await
        .map_err(|e| format!("UserContentFilterStore::save: {e}"))?;
    ctx.ucm.add_filter(&filter);
    // Prove the compiled filter round-trips through the store too.
    store
        .load_future("copper-spike")
        .await
        .map_err(|e| format!("UserContentFilterStore::load: {e}"))?;

    ctx.hits.lock().unwrap().clear();
    load(&ctx.view, &format!("{}/filter", ctx.base)).await?;
    // Let subresources settle.
    glib::timeout_future(Duration::from_millis(300)).await;
    let v = eval_json(
        &ctx.view,
        "({allowed: !!window.allowedRan, blocked: !!window.blockedRan, \
          adDisplay: getComputedStyle(document.querySelector('.ad')).display, \
          gifLoaded: document.getElementById('gif').naturalWidth > 0})",
    )
    .await?;
    let hits = ctx.hits.lock().unwrap().clone();
    let server_saw_blocked = hits.iter().any(|p| p.contains("blocked.js"));
    let server_saw_tracker = hits.iter().any(|p| p.contains("tracker.gif"));
    let server_saw_allowed = hits.iter().any(|p| p.contains("allowed.js"));
    ctx.ucm.remove_all_filters();
    if v["allowed"] != true
        || v["blocked"] != false
        || v["adDisplay"] != "none"
        || server_saw_blocked
        || server_saw_tracker
        || !server_saw_allowed
    {
        return Err(format!("page={v} server hits={hits:?}"));
    }
    Ok(format!(
        "3 rules compiled; blocked.js + tracker.gif never reached server, allowed.js did; .ad display=none; page={v}"
    ))
}

async fn check_cookies(ctx: &Ctx) -> Res {
    if ctx.normal_session.is_ephemeral() || !ctx.private_session.is_ephemeral() {
        return Err("is_ephemeral flags wrong".into());
    }
    load(&ctx.view, &format!("{}/setcookie?v=normal", ctx.base)).await?;
    load(
        &ctx.private_view,
        &format!("{}/setcookie?v=private", ctx.base),
    )
    .await?;
    let a = eval_json(&ctx.view, "document.cookie").await?;
    let b = eval_json(&ctx.private_view, "document.cookie").await?;
    let url = format!("{}/", ctx.base);
    let ca = cookie_names(&ctx.normal_session, &url).await?;
    let cb = cookie_names(&ctx.private_session, &url).await?;
    if a != "spike=normal"
        || b != "spike=private"
        || ca != ["spike=normal"]
        || cb != ["spike=private"]
    {
        return Err(format!(
            "normal js={a} store={ca:?}; private js={b} store={cb:?}"
        ));
    }
    Ok(format!(
        "normal sees {a} (CookieManager {ca:?}), ephemeral sees {b} (CookieManager {cb:?})"
    ))
}

async fn cookie_names(session: &NetworkSession, url: &str) -> Result<Vec<String>, String> {
    let cm = session.cookie_manager().ok_or("no cookie manager")?;
    let cookies = cm
        .cookies_future(url)
        .await
        .map_err(|e| format!("cookies: {e}"))?;
    Ok(cookies
        .into_iter()
        .map(|mut c| {
            format!(
                "{}={}",
                c.name().unwrap_or_default(),
                c.value().unwrap_or_default()
            )
        })
        .collect())
}

async fn check_snapshot(ctx: &Ctx, region: SnapshotRegion) -> Res {
    load(&ctx.view, &format!("{}/", ctx.base)).await?;
    glib::timeout_future(Duration::from_millis(200)).await;
    let tex = ctx
        .view
        .snapshot_future(region, SnapshotOptions::NONE)
        .await
        .map_err(|e| format!("get_snapshot: {e}"))?;
    let (w, h) = (tex.width(), tex.height());
    let name = match region {
        SnapshotRegion::Visible => "visible",
        _ => "full",
    };
    let out = Path::new("/tmp/copper-hello").join(format!("snapshot-{name}.png"));
    std::fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    tex.save_to_png(&out)
        .map_err(|e| format!("save_to_png: {e}"))?;
    let colors = distinct_colors(&tex);
    let view_h = ctx.view.height();
    if colors < 4 {
        return Err(format!("{w}x{h} looks blank ({colors} distinct colors)"));
    }
    if matches!(region, SnapshotRegion::FullDocument) && h <= view_h {
        return Err(format!(
            "full snapshot {w}x{h} not taller than view ({view_h}px)"
        ));
    }
    Ok(format!(
        "{w}x{h} texture, {colors} distinct colors (view height {view_h}), saved {}",
        out.display()
    ))
}

/// Count distinct pixel values (capped) to tell a rendered page from a blank one.
fn distinct_colors(tex: &gdk4::Texture) -> usize {
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
    seen.len()
}

async fn check_download(ctx: &Ctx) -> Res {
    let dest = ctx.work.join("downloads/copper-spike.bin");
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    let started = Rc::new(Cell::new(false));
    let s = started.clone();
    let h = ctx
        .normal_session
        .connect_download_started(move |_, _| s.set(true));
    let dl = ctx
        .view
        .download_uri(&format!("{}/file.bin", ctx.base))
        .ok_or("download_uri returned None")?;
    let state: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
    let d = dest.clone();
    dl.connect_decide_destination(move |dl, suggested| {
        tracing_line(&format!("decide-destination suggested={suggested}"));
        dl.set_destination(d.to_str().unwrap());
        true
    });
    let st = state.clone();
    dl.connect_failed(move |_, e| *st.borrow_mut() = Some(Err(e.to_string())));
    let st = state.clone();
    dl.connect_finished(move |_| {
        let mut s = st.borrow_mut();
        if s.is_none() {
            *s = Some(Ok(()));
        }
    });
    wait_until(|| state.borrow().is_some()).await;
    ctx.normal_session.disconnect(h);
    state.take().unwrap()?;
    let bytes = std::fs::read(&dest).map_err(|e| format!("read {}: {e}", dest.display()))?;
    if bytes != server::DOWNLOAD_BODY {
        return Err(format!(
            "downloaded {} bytes, content mismatch",
            bytes.len()
        ));
    }
    Ok(format!(
        "download-started={} finished, {} bytes at {} (received_data_length={})",
        started.get(),
        bytes.len(),
        dest.display(),
        dl.received_data_length()
    ))
}

fn tracing_line(s: &str) {
    if std::env::var_os("HELLO_DEBUG").is_some() {
        eprintln!("hello: {s}");
    }
}

async fn check_inspector(ctx: &Ctx) -> Res {
    let inspector = ctx.view.inspector().ok_or("no inspector")?;
    let opened = Rc::new(Cell::new(""));
    let o = opened.clone();
    inspector.connect_open_window(move |_| {
        o.set("open-window");
        false
    });
    let o = opened.clone();
    inspector.connect_attach(move |_| {
        o.set("attach");
        false
    });
    inspector.show();
    wait_until(|| !opened.get().is_empty() && inspector.web_view().is_some()).await;
    // Give the inspector front-end a moment to load before we tear it down.
    glib::timeout_future(Duration::from_millis(500)).await;
    let detail = format!(
        "signal={} attached={} inspected_uri={}",
        opened.get(),
        inspector.is_attached(),
        inspector.inspected_uri().unwrap_or_default()
    );
    inspector.close();
    Ok(detail)
}
