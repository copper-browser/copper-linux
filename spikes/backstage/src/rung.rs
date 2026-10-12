//! One rung of the ladder, run in its own process (`backstage --rung NAME`).
//!
//! Every rung builds the same "user" window — a field (`GtkEntry`, focused)
//! over a `GtkStack` showing the user's tab — then hosts an agent page the
//! rung's way, measures it, checks the user's window did not change, and
//! prints one `RESULT {json}` line.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk4::prelude::*;
use serde_json::{Value, json};
use webkit6::prelude::*;
use webkit6::{LoadEvent, NetworkSession, SnapshotOptions, SnapshotRegion, WebView};

use crate::automation;
use crate::ghost::GhostHost;

/// Size of the user's window and of the agent's stage.
const WIN_W: i32 = 1024;
const WIN_H: i32 = 768;
const STAGE_W: i32 = 1024;
const STAGE_H: i32 = 700;
/// rAF / timer sampling window.
const SAMPLE: Duration = Duration::from_millis(2000);
const OUT_DIR: &str = "/tmp/copper-backstage";

/// Probe-page points (CSS px): see server.rs.
const BTN: (f64, f64) = (400.0, 140.0);
const FIELD: (f64, f64) = (450.0, 270.0);
const HOVER: (f64, f64) = (350.0, 130.0);

pub type Res<T> = Result<T, String>;

pub struct Stage {
    pub window: gtk4::Window,
    pub entry: gtk4::Entry,
    pub stack: gtk4::Stack,
    pub overlay: gtk4::Overlay,
    pub user_view: WebView,
    pub base: String,
}

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

async fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while !cond() {
        if Instant::now() > deadline {
            return false;
        }
        sleep(20).await;
    }
    true
}

pub async fn load(view: &WebView, url: &str) -> Res<()> {
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
    let ok = wait_until(Duration::from_secs(15), || done.get()).await;
    view.disconnect(h1);
    view.disconnect(h2);
    if let Some(e) = failed.take() {
        return Err(format!("load failed: {e}"));
    }
    if !ok {
        return Err(format!("load of {url} did not finish in 15 s"));
    }
    Ok(())
}

pub async fn eval(view: &WebView, js: &str) -> Res<Value> {
    let fut = view.evaluate_javascript_future(js, None, None);
    let v = glib::future_with_timeout(Duration::from_secs(10), fut)
        .await
        .map_err(|_| format!("eval timed out: {js}"))?
        .map_err(|e| format!("eval: {e}"))?;
    let s = v
        .to_json(0)
        .map(|s| s.to_string())
        .unwrap_or_else(|| "null".into());
    serde_json::from_str(&s).map_err(|e| format!("bad JSON {s:?}: {e}"))
}

/// The user's window: field on top, their tab below, focus in the field.
pub async fn user_stage(base: &str) -> Res<Stage> {
    let user_view = WebView::builder().build();
    user_view.set_vexpand(true);
    let stack = gtk4::Stack::new();
    stack.add_named(&user_view, Some("user"));
    stack.set_visible_child_name("user");
    stack.set_vexpand(true);
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&stack));
    let entry = gtk4::Entry::new();
    entry.set_text("the user's field");
    let vbox = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    vbox.append(&entry);
    vbox.append(&overlay);
    let window = gtk4::Window::builder()
        .title("user window")
        .default_width(WIN_W)
        .default_height(WIN_H)
        .child(&vbox)
        .build();
    window.present();
    entry.grab_focus();
    load(&user_view, &format!("{base}/user")).await?;
    Ok(Stage {
        window,
        entry,
        stack,
        overlay,
        user_view,
        base: base.to_string(),
    })
}

/// Everything about the user's window an agent must not change.
#[derive(Clone, Debug, PartialEq)]
pub struct UserState {
    visible_child: Option<String>,
    focus_is_field: bool,
    window_active: bool,
    pick_is_user_tab: bool,
    page_active_element: String,
}

pub async fn user_state(stage: &Stage) -> UserState {
    let focus_is_field = gtk4::prelude::GtkWindowExt::focus(&stage.window)
        .map(|w| w == stage.entry.clone().upcast::<gtk4::Widget>() || w.is_ancestor(&stage.entry))
        .unwrap_or(false);
    let centre = gtk4::graphene::Point::new(
        stage.stack.width() as f32 / 2.0,
        stage.stack.height() as f32 / 2.0,
    );
    let pick_is_user_tab = stage
        .stack
        .compute_point(&stage.window, &centre)
        .and_then(|p| {
            stage
                .window
                .pick(p.x() as f64, p.y() as f64, gtk4::PickFlags::DEFAULT)
        })
        .map(|w| {
            w == stage.user_view.clone().upcast::<gtk4::Widget>() || w.is_ancestor(&stage.user_view)
        })
        .unwrap_or(false);
    let page_active_element = eval(
        &stage.user_view,
        "document.activeElement ? document.activeElement.tagName : null",
    )
    .await
    .map(|v| v.to_string())
    .unwrap_or_else(|e| e);
    UserState {
        visible_child: stage.stack.visible_child_name().map(|s| s.to_string()),
        focus_is_field,
        window_active: stage.window.is_active(),
        pick_is_user_tab,
        page_active_element,
    }
}

/// What the user sees in their tab area, rendered through GTK (includes any
/// overlay child, so a host that drew anything would show up here).
fn user_pixels(stage: &Stage) -> Option<Vec<u8>> {
    let (w, h) = (stage.overlay.width(), stage.overlay.height());
    // Snapshot the overlay the way its parent does while painting the window
    // (a GtkWidgetPaintable keeps a cached image and is not a fair probe).
    let parent = stage.overlay.parent()?;
    let snap = gtk4::Snapshot::new();
    parent.snapshot_child(&stage.overlay, &snap);
    let Some(node) = snap.to_node() else {
        eprintln!("backstage: user_pixels: empty node ({w}x{h})");
        return None;
    };
    let renderer = stage.window.renderer()?;
    let origin = stage
        .overlay
        .compute_point(&parent, &gtk4::graphene::Point::new(0.0, 0.0))?;
    let viewport = gtk4::graphene::Rect::new(origin.x(), origin.y(), w as f32, h as f32);
    let tex = renderer.render_texture(&node, Some(&viewport));
    let mut buf = vec![0u8; (tex.width() * tex.height() * 4) as usize];
    tex.download(&mut buf, (tex.width() * 4) as usize);
    Some(buf)
}

/// The whole output as the compositor shows it (weston-screenshooter; the
/// selftest's private weston runs with --debug so capture is allowed).
fn screen(tag: &str) -> Option<Vec<u8>> {
    let dir = format!("{OUT_DIR}/screen-{}-{tag}", std::process::id());
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let ok = std::process::Command::new("weston-screenshooter")
        .current_dir(&dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?
        .success();
    if !ok {
        eprintln!(
            "backstage: weston-screenshooter failed (is the compositor running with --debug?)"
        );
        return None;
    }
    let file = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "png"))?;
    let tex = gdk4::Texture::from_filename(&file).ok()?;
    let mut buf = vec![0u8; (tex.width() * tex.height() * 4) as usize];
    tex.download(&mut buf, (tex.width() * 4) as usize);
    let _ = std::fs::rename(&file, format!("{OUT_DIR}/screen-{tag}.png"));
    let _ = std::fs::remove_dir_all(&dir);
    Some(buf)
}

fn diff_pixels(a: &Option<Vec<u8>>, b: &Option<Vec<u8>>) -> Option<usize> {
    match (a, b) {
        (Some(a), Some(b)) if a.len() == b.len() => {
            Some(a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count())
        }
        _ => {
            eprintln!(
                "backstage: user pixels not comparable: before={:?} after={:?}",
                a.as_ref().map(|v| v.len()),
                b.as_ref().map(|v| v.len())
            );
            None
        }
    }
}

/// rAF and timer rates of a page carrying `window.probe` (or `uprobe`).
pub async fn rates(view: &WebView, var: &str) -> Res<Value> {
    let js = format!(
        "({{raf: {var}.raf, timers: {var}.timers ?? 0, t: performance.now(), vis: document.visibilityState, focus: document.hasFocus()}})"
    );
    let a = eval(view, &js).await?;
    sleep(SAMPLE.as_millis() as u64).await;
    let b = eval(view, &js).await?;
    let secs = (b["t"].as_f64().unwrap_or(0.0) - a["t"].as_f64().unwrap_or(0.0)) / 1000.0;
    let per = |k: &str| {
        let d = b[k].as_f64().unwrap_or(0.0) - a[k].as_f64().unwrap_or(0.0);
        if secs > 0.0 {
            (d / secs * 10.0).round() / 10.0
        } else {
            0.0
        }
    };
    Ok(json!({
        "raf_per_s": per("raf"),
        "timers_per_s": per("timers"),
        "visibility": b["vis"],
        "has_focus": b["focus"],
        "sample_s": (secs * 100.0).round() / 100.0,
    }))
}

struct Snap {
    w: i32,
    h: i32,
    colors: usize,
    nonwhite: f64,
    probe_px: [u8; 4],
    ms: u128,
}

async fn snapshot(view: &WebView, path: &str) -> Res<Snap> {
    let t0 = Instant::now();
    let fut = view.snapshot_future(SnapshotRegion::Visible, SnapshotOptions::NONE);
    let tex = glib::future_with_timeout(Duration::from_secs(10), fut)
        .await
        .map_err(|_| "snapshot timed out (10 s)".to_string())?
        .map_err(|e| format!("snapshot: {e}"))?;
    let ms = t0.elapsed().as_millis();
    let (w, h) = (tex.width(), tex.height());
    let mut buf = vec![0u8; (w * h * 4) as usize];
    tex.download(&mut buf, (w * 4) as usize);
    let mut set = HashSet::new();
    let mut nonwhite = 0usize;
    for px in buf.chunks(4) {
        set.insert(u32::from_ne_bytes([px[0], px[1], px[2], px[3]]));
        if px[0] < 250 || px[1] < 250 || px[2] < 250 {
            nonwhite += 1;
        }
    }
    let i = ((100 * w + 100) * 4) as usize;
    let probe_px = if buf.len() >= i + 4 {
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    } else {
        [0; 4]
    };
    let _ = tex.save_to_png(path);
    Ok(Snap {
        w,
        h,
        colors: set.len(),
        nonwhite: nonwhite as f64 / (w * h).max(1) as f64,
        probe_px,
        ms,
    })
}

/// Two snapshots 500 ms apart: non-blank, and live (the rAF box changed).
async fn snapshots(view: &WebView, rung: &str) -> Value {
    let a = snapshot(view, &format!("{OUT_DIR}/{rung}-1.png")).await;
    sleep(500).await;
    let b = snapshot(view, &format!("{OUT_DIR}/{rung}-2.png")).await;
    match (a, b) {
        (Ok(a), Ok(b)) => json!({
            "ok": true,
            "size": format!("{}x{}", a.w, a.h),
            "colors": a.colors,
            "nonwhite": (a.nonwhite * 1000.0).round() / 1000.0,
            "live": a.probe_px != b.probe_px,
            "ms": [a.ms, b.ms],
            "non_blank": a.colors >= 16 && a.nonwhite > 0.05,
        }),
        (Err(e), _) | (_, Err(e)) => {
            json!({"ok": false, "error": e, "non_blank": false, "live": false})
        }
    }
}

/// WebKit features (Settings::all_features) that sound like hidden-page
/// throttling, with their defaults.
fn throttling_features(settings: Option<&webkit6::Settings>, disable: bool) -> Value {
    let Some(list) = webkit6::Settings::all_features() else {
        return json!("all_features() returned None");
    };
    let mut out = Vec::new();
    for i in 0..list.length() {
        let Some(f) = list.get(i) else { continue };
        let id = f.identifier().map(|s| s.to_string()).unwrap_or_default();
        let l = id.to_lowercase();
        if !(l.contains("hidden")
            || l.contains("throttl")
            || l.contains("visib")
            || l.contains("background")
            || l.contains("suspend"))
        {
            continue;
        }
        let mut entry = json!({"id": id, "status": format!("{:?}", f.status()), "default": f.is_default_value()});
        if let Some(s) = settings {
            let turns_throttling_off = l.contains("throttl") || l.contains("suspension");
            if disable && turns_throttling_off {
                s.set_feature_enabled(&f, false);
            }
            entry["now"] = json!(s.is_feature_enabled(&f));
        }
        out.push(entry);
    }
    json!(out)
}

fn agent_view(automation: bool) -> WebView {
    let view = if automation {
        WebView::builder().is_controlled_by_automation(true).build()
    } else {
        WebView::builder()
            .network_session(&NetworkSession::new_ephemeral())
            .build()
    };
    // A right click from an agent must never pop a menu over the user's window.
    view.connect_context_menu(|_, _, _| true);
    view
}

// ---------------------------------------------------------------------------
// Rendering rungs

pub async fn render(rung: &str, base: &str) -> Res<Value> {
    let stage = user_stage(base).await?;
    sleep(300).await;
    let before = user_state(&stage).await;
    let px_before = user_pixels(&stage);
    let scr_before = screen(&format!("{rung}-before"));

    let agent = agent_view(false);
    let mut notes: Vec<String> = Vec::new();
    let mut visible_to_user = false;
    let mut keep: Vec<gtk4::Window> = Vec::new();
    let mut nested: Option<std::process::Child> = None;
    let mut features = Value::Null;
    let mut host: Option<GhostHost> = None;

    match rung {
        "a1-stack-hidden" => {
            features = throttling_features(WebViewExt::settings(&agent).as_ref(), true);
            stack_add(&stage, &agent);
            notes.push("agent view is a non-visible GtkStack page of the user's window; throttling features set to false".into());
        }
        "a2-ghost-host" | "x-handoff-ghost" => {
            let h = GhostHost::new(STAGE_W, STAGE_H);
            stage.overlay.add_overlay(&h);
            h.hang(&agent);
            notes.push("agent view mapped inside the user's window under a 1x1 GhostHost that snapshots it into a discarded GtkSnapshot".into());
            host = Some(h);
        }
        "z-baseline-visible" => {
            // Reference: the probe page as the user's visible tab.
            stage.stack.add_named(&agent, Some("agent"));
            stage.stack.set_visible_child_name("agent");
            visible_to_user = true;
            notes.push("reference only: the probe page shown as the visible tab (this box renders in software)".into());
        }
        "b1-hidden-toplevel" => {
            let w = second_window(&agent, "backstage (never presented)");
            notes.push("second GtkWindow, never presented (unmapped)".into());
            keep.push(w);
        }
        "b2-second-toplevel" | "b3-minimized-toplevel" => {
            let w = second_window(&agent, "backstage toplevel");
            w.present();
            visible_to_user = true;
            if rung == "b3-minimized-toplevel" {
                sleep(300).await;
                w.minimize();
                notes.push("second GtkWindow presented, then minimize() (Wayland gives no off-screen placement)".into());
            } else {
                notes.push(
                    "second GtkWindow presented: a real toplevel on the user's output".into(),
                );
            }
            keep.push(w);
        }
        "c-nested-compositor" | "c2-nested-ghost" | "x-handoff-nested" => {
            let (child, display, sock) = nested_display()?;
            let w = gtk4::Window::builder()
                .title("backstage (nested)")
                .default_width(STAGE_W)
                .default_height(STAGE_H)
                .build();
            w.set_display(&display);
            if rung != "c-nested-compositor" {
                let h = GhostHost::new(STAGE_W, STAGE_H);
                w.set_child(Some(&h));
                h.hang(&agent);
                host = Some(h);
                notes.push(format!("GhostHost in a GtkWindow on a private headless weston (socket {sock}): mapped there, never composited"));
            } else {
                w.set_child(Some(&agent));
                notes.push(format!("second GtkWindow on a private headless weston (socket {sock}) opened with gdk::Display::open"));
            }
            w.present();
            nested = Some(child);
            keep.push(w);
        }
        "x-ghost-selfdrive-stalled" | "x-ghost-stalled" => {
            // A window whose frame clock stops: weston stops frame callbacks for
            // a minimized surface but GTK/WebKit still think it is visible —
            // the same as an occluded window on a compositor that throttles.
            let w = gtk4::Window::builder()
                .title("stalled window")
                .default_width(STAGE_W)
                .default_height(STAGE_H)
                .build();
            let h = GhostHost::new(STAGE_W, STAGE_H);
            // A spinner keeps damaging the window, so GTK commits every frame
            // and then waits for frame callbacks the compositor never sends.
            let spinner = gtk4::Spinner::new();
            spinner.set_spinning(true);
            spinner.set_size_request(64, 64);
            let overlay = gtk4::Overlay::new();
            overlay.set_child(Some(&spinner));
            overlay.add_overlay(&h);
            w.set_child(Some(&overlay));
            h.hang(&agent);
            w.present();
            sleep(300).await;
            w.minimize();
            if rung == "x-ghost-selfdrive-stalled" {
                // The source runs until the process exits.
                let _ = h.self_drive(Duration::from_millis(8));
                notes.push("experiment: GhostHost + 8 ms self-drive timer, inside a window whose frame clock is stalled".into());
            } else {
                notes.push("experiment: GhostHost without self-drive, inside a window whose frame clock is stalled".into());
            }
            host = Some(h);
            keep.push(w);
        }
        other => return Err(format!("unknown rung {other}")),
    }

    let probe_url = format!("{}/probe", stage.base);
    let loaded = load(&agent, &probe_url).await;
    sleep(500).await;
    let mut out = json!({"rung": rung, "kind": "render"});
    match loaded {
        Err(e) => {
            out["error"] = json!(e);
        }
        Ok(()) => {
            out["agent"] = rates(&agent, "probe")
                .await
                .unwrap_or_else(|e| json!({"error": e}));
            out["snapshot"] = snapshots(&agent, rung).await;
        }
    }
    // Hand-off: the user picks the agent's tab. The view leaves its host
    // and becomes the visible page of the user's stack.
    if rung.starts_with("x-handoff") && out.get("error").is_none() {
        agent.unparent();
        stage.stack.add_named(&agent, Some("agent"));
        stage.stack.set_visible_child_name("agent");
        sleep(700).await;
        let mut h = rates(&agent, "probe")
            .await
            .unwrap_or_else(|e| json!({"error": e}));
        h["snapshot"] = snapshots(&agent, &format!("{rung}-shown")).await;
        h["shown"] = json!(
            agent.is_mapped() && stage.stack.visible_child_name().as_deref() == Some("agent")
        );
        out["after_handoff"] = h;
        stage.stack.set_visible_child_name("user");
        sleep(200).await;
    }
    out["user_raf_per_s"] = rates(&stage.user_view, "uprobe")
        .await
        .map(|v| v["raf_per_s"].clone())
        .unwrap_or(Value::Null);
    let after = user_state(&stage).await;
    let px_after = user_pixels(&stage);
    let scr_after = screen(&format!("{rung}-after"));
    let changed_px = diff_pixels(&px_before, &px_after);
    let changed_screen = diff_pixels(&scr_before, &scr_after);
    if changed_screen.is_some_and(|n| n > 0) {
        visible_to_user = true;
    }
    let user_unchanged = before == after
        && before.focus_is_field
        && before.visible_child.as_deref() == Some("user")
        && changed_px == Some(0)
        && changed_screen.is_none_or(|n| n == 0);
    out["user"] = json!({
        "before": format!("{before:?}"),
        "after": format!("{after:?}"),
        "changed_pixels_gtk": changed_px,
        "changed_pixels_screen": changed_screen,
        "unchanged": user_unchanged,
        "visible_to_user": visible_to_user,
    });
    if let Some(h) = &host {
        out["ghost_snapshots"] = json!(h.snapshots());
        out["ghost_self_driven"] = json!(h.self_driven());
        out["ghost_allocations"] = json!(h.allocations());
    }
    out["experiment"] = json!(rung.starts_with("x-") || rung.starts_with("z-"));
    if !features.is_null() {
        out["throttling_features"] = features;
    }
    out["notes"] = json!(notes);
    let raf = out["agent"]["raf_per_s"].as_f64().unwrap_or(0.0);
    let snap_ok = out["snapshot"]["non_blank"] == true && out["snapshot"]["live"] == true;
    // Experiments only answer "does the page keep painting?".
    out["pass"] = if rung.starts_with("x-handoff") {
        let h = &out["after_handoff"];
        // Shown pages are composited in software here (z-baseline-visible
        // measures the ceiling, ~39/s); 30/s means "keeps rendering".
        json!(
            raf >= 50.0
                && snap_ok
                && h["raf_per_s"].as_f64().unwrap_or(0.0) >= 30.0
                && h["snapshot"]["live"] == true
                && h["shown"] == true
        )
    } else if rung.starts_with("x-") || rung.starts_with("z-") {
        json!(raf >= 50.0 && snap_ok)
    } else {
        json!(raf >= 50.0 && snap_ok && user_unchanged && !visible_to_user)
    };
    for w in keep {
        w.destroy();
    }
    if let Some(mut c) = nested {
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(out)
}

fn stack_add(stage: &Stage, agent: &WebView) {
    stage.stack.add_named(agent, Some("agent"));
    stage.stack.set_visible_child_name("user");
}

fn second_window(agent: &WebView, title: &str) -> gtk4::Window {
    let w = gtk4::Window::builder()
        .title(title)
        .default_width(STAGE_W)
        .default_height(STAGE_H)
        .build();
    w.set_child(Some(agent));
    w
}

/// Start a private headless weston and open it as a second GdkDisplay.
fn nested_display() -> Res<(std::process::Child, gdk4::Display, String)> {
    let sock = format!("copper-backstage-{}", std::process::id());
    let runtime = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR unset")?;
    let log =
        std::fs::File::create(format!("{OUT_DIR}/nested-weston.log")).map_err(|e| e.to_string())?;
    let child = std::process::Command::new("weston")
        .args([
            "--backend=headless",
            "--renderer=pixman",
            &format!("--socket={sock}"),
            &format!("--width={STAGE_W}"),
            &format!("--height={STAGE_H}"),
            "--idle-time=0",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("spawn weston: {e}"))?;
    let path = std::path::Path::new(&runtime).join(&sock);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        if Instant::now() > deadline {
            return Err(format!(
                "nested weston socket {} never appeared",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let display = gdk4::Display::open(Some(&sock))
        .ok_or_else(|| format!("gdk::Display::open({sock}) failed"))?;
    Ok((child, display, sock))
}

// ---------------------------------------------------------------------------
// Input rungs (agent page hosted the a2 way)

async fn probe(view: &WebView) -> Value {
    eval(
        view,
        "({moves: probe.moves, movesTrusted: probe.movesTrusted, lastMove: probe.lastMove ?? null, \
          clicks: probe.clicks, downs: probe.downs, dbl: probe.dbl, ctx: probe.ctx, keys: probe.keys, \
          inputs: probe.inputs, focus: probe.focus, submits: probe.submits, \
          active: document.activeElement && document.activeElement.id, value: document.getElementById('field').value, \
          hasFocus: document.hasFocus(), webdriver: navigator.webdriver ?? null})",
    )
    .await
    .unwrap_or_else(|e| json!({"error": e}))
}

async fn reset_probe(view: &WebView) {
    let _ = eval(
        view,
        "probe.moves=0; probe.movesTrusted=0; for (const k of ['clicks','downs','dbl','ctx','keys','inputs','focus','submits']) probe[k]=[]; true",
    )
    .await;
}

fn all_trusted(v: &Value) -> Value {
    match v.as_array() {
        Some(a) if !a.is_empty() => json!(a.iter().all(|e| e["t"] == true)),
        _ => Value::Null,
    }
}

/// Summary of one input attempt: how many events of each kind and whether
/// every one was trusted (null = none arrived).
fn summarize(p: &Value) -> Value {
    json!({
        "moves": p["moves"], "moves_trusted": p["movesTrusted"],
        "clicks": p["clicks"].as_array().map(|a| a.len()), "click_trusted": all_trusted(&p["clicks"]),
        "click_pointer_type": p["clicks"].get(0).map(|c| c["p"].clone()),
        "click0": p["clicks"].get(0).cloned(), "down0": p["downs"].get(0).cloned(), "last_move": p["lastMove"],
        "dblclick_trusted": all_trusted(&p["dbl"]),
        "contextmenu_trusted": all_trusted(&p["ctx"]),
        "focus_trusted": all_trusted(&p["focus"]),
        "keys": p["keys"].as_array().map(|a| a.len()), "keys_trusted": all_trusted(&p["keys"]),
        "inputs": p["inputs"], "input_trusted": all_trusted(&p["inputs"]),
        "submit_trusted": all_trusted(&p["submits"]),
        "active": p["active"], "value": p["value"], "page_has_focus": p["hasFocus"],
    })
}

struct Controllers {
    motion: Option<gtk4::EventControllerMotion>,
    touch_click: Option<gtk4::GestureClick>,
    focus: Option<gtk4::EventControllerFocus>,
    count: u32,
}

/// WebKitWebViewBase's own event controllers (gtk_widget_observe_controllers).
fn controllers(view: &WebView) -> Controllers {
    let list = view.observe_controllers();
    let mut c = Controllers {
        motion: None,
        touch_click: None,
        focus: None,
        count: list.n_items(),
    };
    for i in 0..list.n_items() {
        let Some(obj) = list.item(i) else { continue };
        if let Ok(m) = obj.clone().downcast::<gtk4::EventControllerMotion>() {
            c.motion = Some(m);
        } else if let Ok(f) = obj.clone().downcast::<gtk4::EventControllerFocus>() {
            c.focus = Some(f);
        } else if let Ok(g) = obj.downcast::<gtk4::GestureClick>()
            && g.is_touch_only()
        {
            c.touch_click = Some(g);
        }
    }
    c
}

pub async fn input_signals(base: &str) -> Res<Value> {
    let stage = user_stage(base).await?;
    sleep(300).await;
    let before = user_state(&stage).await;
    let px_before = user_pixels(&stage);
    let scr_before = screen("input-signals-before");
    let agent = agent_view(false);
    let host = GhostHost::new(STAGE_W, STAGE_H);
    stage.overlay.add_overlay(&host);
    host.hang(&agent);
    load(&agent, &format!("{}/probe", stage.base)).await?;
    sleep(500).await;

    let ctl = controllers(&agent);
    let mut out = json!({"rung": "input-signals", "kind": "input", "controllers": ctl.count});

    // (2a) GtkEventController signal emission.
    let mut sig = serde_json::Map::new();
    if let Some(m) = &ctl.motion {
        reset_probe(&agent).await;
        m.emit_by_name::<()>("enter", &[&HOVER.0, &HOVER.1]);
        sleep(200).await;
        sig.insert("hover".into(), summarize(&probe(&agent).await));
    }
    if let Some(g) = &ctl.touch_click {
        reset_probe(&agent).await;
        g.emit_by_name::<()>("released", &[&1i32, &BTN.0, &BTN.1]);
        sleep(250).await;
        sig.insert("click".into(), summarize(&probe(&agent).await));

        reset_probe(&agent).await;
        g.emit_by_name::<()>("released", &[&1i32, &BTN.0, &BTN.1]);
        g.emit_by_name::<()>("released", &[&2i32, &BTN.0, &BTN.1]);
        sleep(250).await;
        sig.insert("dblclick".into(), summarize(&probe(&agent).await));

        reset_probe(&agent).await;
        g.emit_by_name::<()>("released", &[&1i32, &FIELD.0, &FIELD.1]);
        sleep(250).await;
        sig.insert("focus_field".into(), summarize(&probe(&agent).await));
    } else {
        sig.insert(
            "error".into(),
            json!("no touch-only GtkGestureClick on the web view"),
        );
    }
    // Page focus without GTK focus: WebKit's focus controller "enter" sets
    // ActivityState::IsFocused (+ WindowIsActive); GTK's focus stays put.
    if let Some(f) = &ctl.focus {
        let q = "({hasFocus: document.hasFocus(), active: document.activeElement && document.activeElement.id})";
        let before_focus = eval(&agent, q).await.unwrap_or(Value::Null);
        f.emit_by_name::<()>("enter", &[]);
        sleep(200).await;
        let during = eval(&agent, q).await.unwrap_or(Value::Null);
        let user_during = user_state(&stage).await;
        f.emit_by_name::<()>("leave", &[]);
        sleep(200).await;
        let after_focus = eval(&agent, q).await.unwrap_or(Value::Null);
        sig.insert(
            "page_focus_via_focus_controller".into(),
            json!({"before": before_focus, "after_enter": during, "after_leave": after_focus,
                   "user_unchanged_during": user_during == before, "gtk_focus_on_agent": agent.has_focus() || agent.is_focus()}),
        );
    }
    out["signals"] = Value::Object(sig);

    // (2b) WebKit editing commands into the focused field.
    reset_probe(&agent).await;
    agent.execute_editing_command_with_argument("InsertText", "hello");
    sleep(250).await;
    let ins = summarize(&probe(&agent).await);
    reset_probe(&agent).await;
    agent.execute_editing_command("DeleteBackward");
    sleep(250).await;
    let del = summarize(&probe(&agent).await);
    reset_probe(&agent).await;
    agent.execute_editing_command("InsertNewline");
    sleep(250).await;
    let nl = summarize(&probe(&agent).await);
    out["editing"] = json!({"InsertText(hello)": ins, "DeleteBackward": del, "InsertNewline": nl});

    // (3) JS-dispatched events (what PageView::input does today).
    reset_probe(&agent).await;
    let js_res = eval(
        &agent,
        "(() => { const b=document.getElementById('btn'); b.dispatchEvent(new MouseEvent('mousemove',{bubbles:true,clientX:350,clientY:130})); \
         b.dispatchEvent(new MouseEvent('mousedown',{bubbles:true})); b.click(); \
         const f=document.getElementById('field'); f.focus(); \
         f.dispatchEvent(new KeyboardEvent('keydown',{key:'x',bubbles:true})); \
         Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(f,'via-js'); \
         f.dispatchEvent(new InputEvent('input',{bubbles:true,inputType:'insertText',data:'via-js'})); \
         document.getElementById('form').requestSubmit(); return true; })()",
    )
    .await;
    sleep(200).await;
    out["js"] = summarize(&probe(&agent).await);
    if let Err(e) = js_res {
        out["js"]["error"] = json!(e);
    }

    // Latency: emit → DOM event, polled from Rust (resolution ≈ one eval round trip).
    let mut lat = serde_json::Map::new();
    for (name, target, event) in [("click", "btn", "click"), ("insert_text", "field", "input")] {
        let mut samples = Vec::new();
        if name == "insert_text"
            && let Some(g) = &ctl.touch_click
        {
            g.emit_by_name::<()>("released", &[&1i32, &FIELD.0, &FIELD.1]);
            sleep(200).await;
        }
        for _ in 0..5 {
            let _ = eval(
                &agent,
                &format!("window.__lat=null; document.getElementById('{target}').addEventListener('{event}', () => {{ window.__lat = performance.now(); }}, {{once: true}}); true"),
            )
            .await;
            let t0 = Instant::now();
            if name == "click" {
                if let Some(g) = &ctl.touch_click {
                    g.emit_by_name::<()>("released", &[&1i32, &BTN.0, &BTN.1]);
                }
            } else {
                agent.execute_editing_command_with_argument("InsertText", "x");
            }
            let mut ok = false;
            while t0.elapsed() < Duration::from_secs(2) {
                if eval(&agent, "window.__lat !== null")
                    .await
                    .unwrap_or(Value::Null)
                    == true
                {
                    ok = true;
                    break;
                }
            }
            samples.push(if ok {
                json!((t0.elapsed().as_secs_f64() * 10000.0).round() / 10.0)
            } else {
                Value::Null
            });
        }
        lat.insert(format!("{name}_ms"), json!(samples));
    }
    out["signals"]["latency"] = Value::Object(lat);

    let after = user_state(&stage).await;
    let px_after = user_pixels(&stage);
    let scr_after = screen(&format!(
        "{}-after",
        out["rung"].as_str().unwrap_or("input")
    ));
    let changed_px = diff_pixels(&px_before, &px_after);
    let changed_screen = diff_pixels(&scr_before, &scr_after);
    out["agent_gtk_focus"] = json!(agent.has_focus() || agent.is_focus());
    out["user"] = json!({
        "before": format!("{before:?}"),
        "after": format!("{after:?}"),
        "changed_pixels_gtk": changed_px,
        "changed_pixels_screen": changed_screen,
        "unchanged": before == after && before.focus_is_field && changed_px == Some(0) && changed_screen.is_none_or(|n| n == 0),
    });
    Ok(out)
}

pub async fn input_automation(base: &str) -> Res<Value> {
    let addr = std::env::var("WEBKIT_INSPECTOR_SERVER")
        .map_err(|_| "WEBKIT_INSPECTOR_SERVER unset (the parent sets it)")?;
    let ctx = webkit6::WebContext::default().ok_or("no default WebContext")?;
    ctx.set_automation_allowed(true);
    let started: Rc<RefCell<Option<String>>> = Rc::default();
    let s = started.clone();
    ctx.connect_automation_started(move |_, session| {
        *s.borrow_mut() = session.id().map(|i| i.to_string());
        session.connect_create_web_view(None, |_| {
            WebView::builder().is_controlled_by_automation(true).build()
        });
    });

    let stage = user_stage(base).await?;
    sleep(300).await;
    let before = user_state(&stage).await;
    let px_before = user_pixels(&stage);
    let scr_before = screen("input-automation-before");
    let agent = agent_view(true);
    let host = GhostHost::new(STAGE_W, STAGE_H);
    stage.overlay.add_overlay(&host);
    host.hang(&agent);
    load(&agent, &format!("{}/probe", stage.base)).await?;
    sleep(300).await;

    let mut out = json!({"rung": "input-automation", "kind": "input"});
    let t0 = Instant::now();
    let (mut client, browser) = automation::Client::connect(&addr, Duration::from_secs(10)).await?;
    out["session"] = json!({"started_signal": *started.borrow(), "browser": browser, "connect_ms": t0.elapsed().as_millis()});
    let t = Duration::from_secs(10);
    let contexts = client
        .call("Automation.getBrowsingContexts", json!({}), t)
        .await?;
    let list = contexts["contexts"].as_array().cloned().unwrap_or_default();
    out["browsing_contexts"] = json!(list.iter().map(|c| c["url"].clone()).collect::<Vec<_>>());
    let handle = list
        .iter()
        .find(|c| c["url"].as_str().is_some_and(|u| u.contains("/probe")))
        .and_then(|c| c["handle"].as_str())
        .ok_or_else(|| format!("probe page not among browsing contexts {list:?}"))?
        .to_string();

    let mouse = |x: f64, y: f64, button: &str, interaction: &str| json!({"handle": handle, "position": {"x": x, "y": y}, "button": button, "interaction": interaction, "modifiers": []});
    let mut res = serde_json::Map::new();
    let mut step =
        async |name: &str, client: &mut automation::Client, method: &str, params: Value| {
            reset_probe(&agent).await;
            let t0 = Instant::now();
            let r = client.call(method, params, t).await;
            sleep(150).await;
            let mut s = summarize(&probe(&agent).await);
            s["call_ms"] = json!(t0.elapsed().as_millis());
            if let Err(e) = r {
                s["error"] = json!(e);
            }
            res.insert(name.into(), s);
        };
    step(
        "hover",
        &mut client,
        "Automation.performMouseInteraction",
        mouse(HOVER.0, HOVER.1, "None", "Move"),
    )
    .await;
    step(
        "click",
        &mut client,
        "Automation.performMouseInteraction",
        mouse(BTN.0, BTN.1, "Left", "SingleClick"),
    )
    .await;
    step(
        "dblclick",
        &mut client,
        "Automation.performMouseInteraction",
        mouse(BTN.0, BTN.1, "Left", "DoubleClick"),
    )
    .await;
    step(
        "right_click",
        &mut client,
        "Automation.performMouseInteraction",
        mouse(BTN.0, BTN.1, "Right", "SingleClick"),
    )
    .await;
    step(
        "focus_field",
        &mut client,
        "Automation.performMouseInteraction",
        mouse(FIELD.0, FIELD.1, "Left", "SingleClick"),
    )
    .await;
    step(
        "type_hi",
        &mut client,
        "Automation.performKeyboardInteractions",
        json!({"handle": handle, "interactions": [{"type": "InsertByKey", "text": "hi"}]}),
    )
    .await;
    step(
        "enter",
        &mut client,
        "Automation.performKeyboardInteractions",
        json!({"handle": handle, "interactions": [{"type": "InsertByKey", "key": "Enter"}]}),
    )
    .await;
    // The W3C actions path (WEBDRIVER_ACTIONS_API) instead of the legacy calls.
    let seq = json!({"handle": handle, "inputSources": [{"sourceId": "m", "sourceType": "Mouse"}], "steps": [
        {"states": [{"sourceId": "m", "origin": "Viewport", "location": {"x": BTN.0, "y": BTN.1}}]},
        {"states": [{"sourceId": "m", "pressedButton": "Left"}]},
        {"states": [{"sourceId": "m"}]}
    ]});
    step(
        "sequence_click",
        &mut client,
        "Automation.performInteractionSequence",
        seq,
    )
    .await;
    out["automation"] = Value::Object(res);
    let p = probe(&agent).await;
    out["navigator_webdriver"] = p["webdriver"].clone();
    out["agent_session_ephemeral"] = json!(agent.network_session().map(|s| s.is_ephemeral()));
    out["agent_is_controlled"] = json!(agent.is_controlled_by_automation());

    let after = user_state(&stage).await;
    let px_after = user_pixels(&stage);
    let scr_after = screen(&format!(
        "{}-after",
        out["rung"].as_str().unwrap_or("input")
    ));
    let changed_px = diff_pixels(&px_before, &px_after);
    let changed_screen = diff_pixels(&scr_before, &scr_after);
    out["agent_gtk_focus"] = json!(agent.has_focus() || agent.is_focus());
    out["user"] = json!({
        "before": format!("{before:?}"),
        "after": format!("{after:?}"),
        "changed_pixels_gtk": changed_px,
        "changed_pixels_screen": changed_screen,
        "unchanged": before == after && before.focus_is_field && changed_px == Some(0) && changed_screen.is_none_or(|n| n == 0),
    });
    Ok(out)
}
