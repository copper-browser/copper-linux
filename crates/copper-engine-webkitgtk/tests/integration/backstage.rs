//! T-002 / D-006 checks: trusted input and the backstage.
//!
//! The bench is a "user" window (a focused field over a `GtkStack` showing
//! the user's tab) and an agent page held in that window by the engine's
//! backstage. Every check that acts on the agent page also checks that the
//! user's window did not change: GTK focus, the visible tab, what the
//! pointer would hit, the user page's own events, the pixels GTK renders for
//! the window's content, the ghost host's (empty) render node, and the set
//! of toplevels (no popup, picker or dialog appeared).

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use copper_engine_webkitgtk::copper_engine::{
    Backstage, BackstageMode, Engine, InputEvent, JsonValue, Modifiers, MouseButton, PageEvent,
    PageOptions, PageView, SnapshotRegion, Trust,
};
use copper_engine_webkitgtk::{TrustedInput, WebKitBackstage};
use gdk4::prelude::*;
use gtk4::prelude::*;
use serde_json::json;

use crate::{Ctx, Res, Tab, ensure, sleep};

const STAGE: (i32, i32) = (1024, 700);
/// Probe-page points (CSS px), see server.rs.
const BTN: (f64, f64) = (400.0, 140.0);
const FIELD: (f64, f64) = (450.0, 270.0);
const SELECT: (f64, f64) = (400.0, 335.0);
const FILE: (f64, f64) = (400.0, 395.0);

/// GTK warnings and criticals seen so far (a ghost host GTK could not
/// allocate or snapshot would show up here).
static GTK_WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Record every Gtk-domain warning (and still print it).
pub fn count_gtk_warnings() {
    glib::log_set_writer_func(|level, fields| {
        if matches!(
            level,
            glib::LogLevel::Warning | glib::LogLevel::Critical | glib::LogLevel::Error
        ) {
            let field = |k: &str| {
                fields
                    .iter()
                    .find(|f| f.key() == k)
                    .and_then(|f| f.value_str())
                    .unwrap_or("")
                    .to_owned()
            };
            if field("GLIB_DOMAIN") == "Gtk" {
                GTK_WARNINGS.lock().unwrap().push(field("MESSAGE"));
            }
        }
        glib::log_writer_default(level, fields)
    });
}

pub struct Bench {
    window: gtk4::Window,
    entry: gtk4::Entry,
    stack: gtk4::Stack,
    content: gtk4::Box,
    user: Tab,
    agent: Tab,
    backstage: WebKitBackstage,
    before: UserState,
    pixels: Option<Vec<u8>>,
    toplevels: usize,
    warnings_at_start: usize,
}

pub fn run<'a>(ctx: &'a mut Ctx, name: &str) -> Pin<Box<dyn Future<Output = Res> + 'a>> {
    match name {
        "self-check" => Box::pin(check_self_check(ctx)),
        "backstage-hold" => Box::pin(check_hold(ctx)),
        "backstage-raf" => Box::pin(check_raf(ctx)),
        "backstage-snapshot" => Box::pin(check_snapshot(ctx)),
        "trusted-click" => Box::pin(check_click(ctx)),
        "trusted-type" => Box::pin(check_type(ctx)),
        "enter-submits" => Box::pin(check_enter(ctx)),
        "backstage-popups" => Box::pin(check_popups(ctx)),
        "page-focus" => Box::pin(check_page_focus(ctx)),
        "handoff" => Box::pin(check_handoff(ctx)),
        "stalled-clock" => Box::pin(check_stalled(ctx)),
        "script-fallback" => Box::pin(check_fallback(ctx)),
        "release" => Box::pin(check_release(ctx)),
        "gtk-warnings" => Box::pin(check_warnings(ctx)),
        _ => unreachable!("unknown check {name}"),
    }
}

fn bench(ctx: &mut Ctx) -> Result<&mut Bench, String> {
    ctx.bench
        .as_mut()
        .ok_or_else(|| "no bench (backstage-hold failed)".to_string())
}

// ---------------------------------------------------------------------------
// The user's window

#[derive(Clone, Debug, PartialEq)]
struct UserState {
    visible: Option<String>,
    focus_is_field: bool,
    pick_is_user_tab: bool,
    page: JsonValue,
}

async fn user_state(b: &Bench) -> Result<UserState, String> {
    let entry: gtk4::Widget = b.entry.clone().upcast();
    let focus_is_field = gtk4::prelude::GtkWindowExt::focus(&b.window)
        .is_some_and(|w| w == entry || w.is_ancestor(&entry));
    let user_view = b.user.page.widget();
    let centre =
        gtk4::graphene::Point::new(b.stack.width() as f32 / 2.0, b.stack.height() as f32 / 2.0);
    let pick_is_user_tab = b
        .stack
        .compute_point(&b.window, &centre)
        .and_then(|p| {
            b.window
                .pick(p.x() as f64, p.y() as f64, gtk4::PickFlags::DEFAULT)
        })
        .is_some_and(|w| w == user_view || w.is_ancestor(&user_view));
    let page = b
        .user
        .js("({active: document.activeElement && document.activeElement.tagName, focus: document.hasFocus(), events: uprobe.events})")
        .await?;
    Ok(UserState {
        visible: b.stack.visible_child_name().map(|s| s.to_string()),
        focus_is_field,
        pick_is_user_tab,
        page,
    })
}

/// The window's content as GTK renders it (RGBA bytes).
fn user_pixels(b: &Bench) -> Option<Vec<u8>> {
    let snap = gtk4::Snapshot::new();
    b.window.snapshot_child(&b.content, &snap);
    let node = snap.to_node()?;
    let renderer = b.window.renderer()?;
    let origin = b
        .content
        .compute_point(&b.window, &gtk4::graphene::Point::new(0.0, 0.0))?;
    let viewport = gtk4::graphene::Rect::new(
        origin.x(),
        origin.y(),
        b.content.width() as f32,
        b.content.height() as f32,
    );
    let tex = renderer.render_texture(&node, Some(&viewport));
    let mut buf = vec![0u8; (tex.width() * tex.height() * 4) as usize];
    tex.download(&mut buf, (tex.width() * 4) as usize);
    Some(buf)
}

fn ghost_of(window: &gtk4::Window) -> Option<gtk4::Widget> {
    let mut child = window.first_child();
    while let Some(c) = child {
        if c.type_().name() == "CopperGhostHost" {
            return Some(c);
        }
        child = c.next_sibling();
    }
    None
}

/// Nothing about the user's window changed since the bench was built.
async fn user_unchanged(b: &Bench) -> Result<String, String> {
    let now = user_state(b).await?;
    ensure(
        b.before.focus_is_field
            && b.before.pick_is_user_tab
            && b.before.visible.as_deref() == Some("user"),
        || format!("bench is wrong before any agent work: {:?}", b.before),
    )?;
    ensure(now == b.before, || {
        format!("user state changed: before {:?} now {now:?}", b.before)
    })?;
    let changed = match (&b.pixels, user_pixels(b)) {
        (Some(a), Some(n)) if a.len() == n.len() => {
            a.chunks(4).zip(n.chunks(4)).filter(|(x, y)| x != y).count()
        }
        (a, n) => {
            return Err(format!(
                "user pixels not comparable: {:?} vs {:?}",
                a.as_ref().map(Vec::len),
                n.map(|v| v.len())
            ));
        }
    };
    ensure(changed == 0, || format!("{changed} user pixels changed"))?;
    if let Some(ghost) = ghost_of(&b.window) {
        let snap = gtk4::Snapshot::new();
        b.window.snapshot_child(&ghost, &snap);
        let node = snap.to_node();
        ensure(node.is_none(), || {
            format!(
                "the ghost host drew something: {:?}",
                node.map(|n| n.bounds())
            )
        })?;
    }
    let toplevels = gtk4::Window::list_toplevels().len();
    ensure(toplevels == b.toplevels, || {
        format!("toplevels {} -> {toplevels}", b.toplevels)
    })?;
    Ok("user window unchanged (focus, tab, pick, page events, 0 px, no popups)".into())
}

// ---------------------------------------------------------------------------
// Agent-page helpers

/// rAF ticks per second over two seconds, and the page's visibility.
async fn rates(tab: &Tab) -> Result<(f64, String), String> {
    let q = "({raf: probe.raf, t: performance.now(), vis: document.visibilityState})";
    let a = tab.js(q).await?;
    sleep(2000).await;
    let b = tab.js(q).await?;
    let secs = (b["t"].as_f64().unwrap_or(0.0) - a["t"].as_f64().unwrap_or(0.0)) / 1000.0;
    let ticks = b["raf"].as_f64().unwrap_or(0.0) - a["raf"].as_f64().unwrap_or(0.0);
    let per = if secs > 0.0 { ticks / secs } else { 0.0 };
    Ok((
        (per * 10.0).round() / 10.0,
        b["vis"].as_str().unwrap_or("?").to_owned(),
    ))
}

/// A held page keeps at least this share of the machine's visible rAF rate.
const HELD_SHARE: f64 = 0.8;
/// What a held page manages with a 60 Hz display; reported, not required
/// (software-rendered CI runners tick slower).
const HELD_NOMINAL: f64 = 50.0;

/// One rAF counter reading.
struct Tick {
    n: f64,
    t: f64,
    vis: String,
}

async fn tick(tab: &Tab, counter: &str) -> Result<Tick, String> {
    let v = tab
        .js(&format!(
            "({{n: {counter}, t: performance.now(), vis: document.visibilityState}})"
        ))
        .await?;
    Ok(Tick {
        n: v["n"].as_f64().unwrap_or(0.0),
        t: v["t"].as_f64().unwrap_or(0.0),
        vis: v["vis"].as_str().unwrap_or("?").to_owned(),
    })
}

fn per_sec(a: &Tick, b: &Tick) -> f64 {
    let secs = (b.t - a.t) / 1000.0;
    let per = if secs > 0.0 { (b.n - a.n) / secs } else { 0.0 };
    (per * 10.0).round() / 10.0
}

/// The held page's rAF rate against the machine's own visible rate.
struct Held {
    held: f64,
    /// The user's tab: a trivial page in a visible window, same two seconds.
    visible: f64,
    vis: String,
    tries: u32,
}

impl Held {
    fn ok(&self) -> bool {
        self.vis == "visible"
            && self.held > 0.0
            && self.visible > 0.0
            && self.held >= HELD_SHARE * self.visible
    }

    fn describe(&self) -> String {
        let share = if self.visible > 0.0 {
            format!("{:.0}%", 100.0 * self.held / self.visible)
        } else {
            "n/a".into()
        };
        let nominal = if self.held < HELD_NOMINAL {
            format!(", below the nominal {HELD_NOMINAL}/s (informational)")
        } else {
            String::new()
        };
        format!(
            "rAF {}/s ({}) vs visible {}/s = {share}, try {}{nominal}",
            self.held, self.vis, self.visible, self.tries
        )
    }
}

/// Whether the held page ticks like a visible one on this machine: its rAF
/// rate and the user's tab's over the same two seconds, up to three tries
/// (a page just held may still be warming up). A throttled page (hidden, or
/// never painted) runs at 0–1/s and fails every try.
async fn held_rate(b: &Bench) -> Result<Held, String> {
    let mut last = None;
    for tries in 1..=3 {
        let (a0, u0) = (
            tick(&b.agent, "probe.raf").await?,
            tick(&b.user, "uprobe.raf").await?,
        );
        sleep(2000).await;
        let (a1, u1) = (
            tick(&b.agent, "probe.raf").await?,
            tick(&b.user, "uprobe.raf").await?,
        );
        let held = Held {
            held: per_sec(&a0, &a1),
            visible: per_sec(&u0, &u1),
            vis: a1.vis,
            tries,
        };
        if held.ok() {
            return Ok(held);
        }
        last = Some(held);
    }
    Ok(last.expect("three tries"))
}

struct Shot {
    w: i32,
    h: i32,
    colors: usize,
    nonwhite: f64,
    px: [u8; 4],
}

async fn shot(tab: &Tab, path: &str) -> Result<Shot, String> {
    let img = tab
        .page
        .snapshot(SnapshotRegion::Visible)
        .await
        .map_err(|e| e.to_string())?;
    let _ = std::fs::write(path, &img.png);
    let tex = gdk4::Texture::from_bytes(&glib::Bytes::from(&img.png)).map_err(|e| e.to_string())?;
    let (w, h) = (tex.width(), tex.height());
    let mut buf = vec![0u8; (w * h * 4) as usize];
    tex.download(&mut buf, (w * 4) as usize);
    let mut set = std::collections::HashSet::new();
    let mut nonwhite = 0usize;
    for px in buf.as_chunks::<4>().0 {
        set.insert(*px);
        if px[0] < 250 || px[1] < 250 || px[2] < 250 {
            nonwhite += 1;
        }
    }
    let i = ((100 * w + 100) * 4) as usize;
    let px = buf
        .get(i..i + 4)
        .map(|s| [s[0], s[1], s[2], s[3]])
        .unwrap_or_default();
    Ok(Shot {
        w,
        h,
        colors: set.len(),
        nonwhite: nonwhite as f64 / (w * h).max(1) as f64,
        px,
    })
}

async fn reset_probe(tab: &Tab) -> Result<(), String> {
    tab.js("probe.moves = 0; probe.movesTrusted = 0; for (const k of ['clicks','dbl','keys','inputs','focus','submits','changes']) probe[k] = []; probe.cancels = 0; true")
        .await
        .map(|_| ())
}

async fn input(tab: &Tab, ev: InputEvent) -> Result<Trust, String> {
    let what = format!("{ev:?}");
    tab.input(ev).await.map_err(|e| format!("{what}: {e}"))
}

async fn expect_trusted(tab: &Tab, ev: InputEvent) -> Result<(), String> {
    let what = format!("{ev:?}");
    let t = input(tab, ev).await?;
    ensure(t == Trust::Trusted, || format!("{what} -> {t:?}"))
}

/// Poll `code` in the page world until it returns `want` (or `ms` pass).
async fn wait_js(tab: &Tab, code: &str, want: &JsonValue, ms: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        let got = tab.js(code).await?;
        if got == *want {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("{code} = {got}, want {want}"));
        }
        sleep(25).await;
    }
}

fn key(key: &str, text: Option<&str>, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key {
        key: key.into(),
        text: text.map(Into::into),
        modifiers,
    }
}

// ---------------------------------------------------------------------------
// Checks

async fn check_self_check(ctx: &mut Ctx) -> Res {
    let t = ctx.a.page.trusted_input();
    ensure(t.pointer && t.editing && t.page_focus, || {
        format!("self-check fell back: {t:?}")
    })?;
    Ok(format!("{t:?}"))
}

async fn check_hold(ctx: &mut Ctx) -> Res {
    if let Some(s) = gtk4::Settings::default() {
        // A blinking caret in the user's field would show up in pixel diffs.
        s.set_gtk_cursor_blink(false);
    }
    let warnings_at_start = GTK_WARNINGS.lock().unwrap().len();
    let mut user = Tab::new(ctx.engine.new_page(&ctx.alpha, PageOptions::default()));
    let uw = user.page.widget();
    uw.set_vexpand(true);
    let stack = gtk4::Stack::new();
    stack.add_named(&uw, Some("user"));
    stack.set_visible_child_name("user");
    stack.set_vexpand(true);
    let entry = gtk4::Entry::new();
    entry.set_text("the user's field");
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&entry);
    content.append(&stack);
    let window = gtk4::Window::builder()
        .title("user window")
        .default_width(1024)
        .default_height(768)
        .child(&content)
        .build();
    window.present();
    entry.grab_focus();
    user.load(&ctx.srv.url("/user")).await?;
    sleep(400).await;

    let mut agent = Tab::new(ctx.engine.new_page(&ctx.alpha, PageOptions::default()));
    let backstage = ctx.engine.backstage().clone();
    backstage.set_stage_size(STAGE.0, STAGE.1);
    let mut b = Bench {
        window,
        entry,
        stack,
        content,
        user,
        agent: Tab::new(agent.page.clone()),
        backstage,
        before: UserState {
            visible: None,
            focus_is_field: false,
            pick_is_user_tab: false,
            page: JsonValue::Null,
        },
        pixels: None,
        toplevels: 0,
        warnings_at_start,
    };
    b.before = user_state(&b).await?;
    b.pixels = user_pixels(&b);
    b.toplevels = gtk4::Window::list_toplevels().len();

    let idle = b.backstage.status();
    b.backstage
        .hold(&agent.page, &b.window)
        .map_err(|e| e.to_string())?;
    agent.load(&ctx.srv.url("/probe")).await?;
    b.agent = agent;
    sleep(500).await;
    let st = b.backstage.status();
    let ghost = ghost_of(&b.window).ok_or("no ghost host in the user's window")?;
    ensure(
        idle.mode == BackstageMode::Idle
            && b.backstage.holds(&b.agent.page)
            && st.held == 1
            && st.hosted == 1
            && st.hosts == 1
            && st.mode == BackstageMode::InWindow
            && st.stage == (STAGE.0 as u32, STAGE.1 as u32),
        || format!("status {idle:?} -> {st:?}"),
    )?;
    let w = b.agent.page.widget();
    ensure(
        w.parent().as_ref() == Some(&ghost)
            && w.is_mapped()
            && w.width() == STAGE.0
            && w.height() == STAGE.1,
        || {
            format!(
                "agent view parent {:?} mapped {} size {}x{}",
                w.parent().map(|p| p.type_().name()),
                w.is_mapped(),
                w.width(),
                w.height()
            )
        },
    )?;
    let same = user_unchanged(&b).await?;
    ctx.bench = Some(b);
    Ok(format!(
        "held in the user's window at {STAGE:?}: {st:?}; {same}"
    ))
}

async fn check_raf(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let held = held_rate(b).await?;
    ensure(held.ok(), || format!("held page {}", held.describe()))?;
    Ok(format!(
        "held page {}; {:?}",
        held.describe(),
        b.backstage.status()
    ))
}

async fn check_snapshot(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let dir = std::env::temp_dir();
    let p1 = dir.join("copper-backstage-1.png");
    let p2 = dir.join("copper-backstage-2.png");
    let a = shot(&b.agent, &p1.to_string_lossy()).await?;
    sleep(500).await;
    let c = shot(&b.agent, &p2.to_string_lossy()).await?;
    ensure((a.w, a.h) == STAGE, || format!("snapshot {}x{}", a.w, a.h))?;
    ensure(a.colors >= 16 && a.nonwhite > 0.05, || {
        format!("blank: {} colours, {:.3} non-white", a.colors, a.nonwhite)
    })?;
    ensure(a.px != c.px, || {
        format!("stale: pixel (100,100) {:?} both times", a.px)
    })?;
    Ok(format!(
        "{}x{}, {} colours, {:.0}% non-white, live ({:?} -> {:?}); {}",
        a.w,
        a.h,
        a.colors,
        a.nonwhite * 100.0,
        a.px,
        c.px,
        p1.display()
    ))
}

async fn check_click(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let tab = &b.agent;
    reset_probe(tab).await?;
    expect_trusted(
        tab,
        InputEvent::Move {
            x: 350.0,
            y: 130.0,
            modifiers: Modifiers::NONE,
        },
    )
    .await?;
    expect_trusted(tab, InputEvent::click(BTN.0, BTN.1)).await?;
    expect_trusted(
        tab,
        InputEvent::Click {
            x: BTN.0,
            y: BTN.1,
            button: MouseButton::Left,
            count: 2,
            modifiers: Modifiers::NONE,
        },
    )
    .await?;
    let p = tab
        .js("({moves: probe.moves, trusted: probe.movesTrusted, clicks: probe.clicks, dbl: probe.dbl})")
        .await?;
    let clicks = p["clicks"].as_array().cloned().unwrap_or_default();
    ensure(
        p["trusted"].as_u64().unwrap_or(0) > 0
            && clicks.len() == 3
            && clicks.iter().all(|c| c["t"] == true)
            && clicks[0]["d"] == 1
            && clicks[2]["d"] == 2
            && clicks[0]["p"] == "mouse"
            && p["dbl"] == json!([true]),
        || format!("probe {p}"),
    )?;
    let miss = tab.page.input(InputEvent::click(5000.0, 5000.0)).await;
    ensure(miss.is_err(), || format!("click off the page -> {miss:?}"))?;
    // The host keeps GTK focus out of the page.
    ensure(!tab.page.widget().grab_focus(), || {
        "a held page could take GTK focus".into()
    })?;
    let same = user_unchanged(b).await?;
    Ok(format!(
        "hover, click, double click all isTrusted ({p}); {same}"
    ))
}

async fn check_type(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let tab = &b.agent;
    reset_probe(tab).await?;
    expect_trusted(tab, InputEvent::click(FIELD.0, FIELD.1)).await?;
    expect_trusted(tab, InputEvent::Type("hello".into())).await?;
    let p = tab
        .js("({value: field.value, inputs: probe.inputs, focus: probe.focus, keys: probe.keys, active: document.activeElement.id})")
        .await?;
    let inputs = p["inputs"].as_array().cloned().unwrap_or_default();
    ensure(
        p["value"] == "hello"
            && p["active"] == "field"
            && p["focus"] == json!([true])
            && inputs.len() == 10
            && inputs
                .iter()
                .all(|e| e["t"] == true && e["it"] == "insertText")
            && p["keys"].as_array().is_some_and(|k| k.len() == 10)
            && p["keys"][0] == "keydown:h"
            && p["keys"][1] == "keyup:h",
        || format!("probe {p}"),
    )?;
    // Editing keys go through WebKit editing commands too.
    let none = Modifiers::NONE;
    let ctrl = Modifiers { ctrl: true, ..none };
    expect_trusted(tab, InputEvent::key("Backspace")).await?;
    wait_js(tab, "field.value", &json!("hell"), 1000).await?;
    expect_trusted(tab, InputEvent::key("Home")).await?;
    expect_trusted(tab, InputEvent::Type("X".into())).await?;
    wait_js(tab, "field.value", &json!("Xhell"), 1000).await?;
    expect_trusted(tab, InputEvent::key("End")).await?;
    expect_trusted(tab, InputEvent::key("ArrowLeft")).await?;
    expect_trusted(tab, InputEvent::Type("-".into())).await?;
    wait_js(tab, "field.value", &json!("Xhel-l"), 1000).await?;
    expect_trusted(tab, key("a", Some("a"), ctrl)).await?;
    expect_trusted(tab, InputEvent::Type("new".into())).await?;
    wait_js(tab, "field.value", &json!("new"), 1000).await?;
    let all_trusted = tab.js("probe.inputs.every(e => e.t)").await?;
    ensure(all_trusted == true, || "an untrusted input event".into())?;
    // Text with a Tab in it: Tab moves focus from script.
    let mixed = input(tab, InputEvent::Type("!\t".into())).await?;
    ensure(mixed == Trust::Mixed, || format!("'!\\t' -> {mixed:?}"))?;
    let same = user_unchanged(b).await?;
    Ok(format!(
        "click-to-focus + \"hello\" with 10 trusted beforeinput/input, Backspace/Home/End/ArrowLeft/Ctrl+A edit natively; {same}"
    ))
}

async fn check_enter(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let tab = &b.agent;
    expect_trusted(tab, InputEvent::click(FIELD.0, FIELD.1)).await?;
    reset_probe(tab).await?;
    expect_trusted(tab, InputEvent::key("Enter")).await?;
    wait_js(tab, "probe.submits", &json!([true]), 1000).await?;
    let same = user_unchanged(b).await?;
    Ok(format!("Enter in the form field -> trusted submit; {same}"))
}

async fn check_popups(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let blocked = |b: &Bench| b.backstage.status().popups_blocked;
    let start = blocked(b);
    reset_probe(&b.agent).await?;
    // <select>: WebKit's popup is kept, not shown; keys drive it.
    expect_trusted(&b.agent, InputEvent::click(SELECT.0, SELECT.1)).await?;
    sleep(200).await;
    let after_select = blocked(b);
    ensure(after_select > start, || {
        "select popup was not intercepted".into()
    })?;
    expect_trusted(&b.agent, InputEvent::key("ArrowDown")).await?;
    expect_trusted(&b.agent, InputEvent::key("Enter")).await?;
    wait_js(
        &b.agent,
        "({v: sel.value, changes: probe.changes})",
        &json!({"v": "b", "changes": [{"v": "b", "t": true}]}),
        2000,
    )
    .await?;
    // Type-ahead and Escape.
    expect_trusted(&b.agent, InputEvent::click(SELECT.0, SELECT.1)).await?;
    sleep(200).await;
    expect_trusted(&b.agent, InputEvent::Type("ch".into())).await?;
    expect_trusted(&b.agent, InputEvent::key("Escape")).await?;
    sleep(200).await;
    let v = b.agent.js("sel.value").await?;
    ensure(v == "b", || format!("Escape changed the value to {v}"))?;
    expect_trusted(&b.agent, InputEvent::click(SELECT.0, SELECT.1)).await?;
    sleep(200).await;
    expect_trusted(&b.agent, InputEvent::Type("ch\n".into())).await?;
    wait_js(&b.agent, "sel.value", &json!("c"), 2000).await?;

    // File chooser: cancelled, nothing opens.
    let before_file = blocked(b);
    expect_trusted(&b.agent, InputEvent::click(FILE.0, FILE.1)).await?;
    sleep(300).await;
    ensure(blocked(b) > before_file, || {
        "file chooser was not intercepted".into()
    })?;
    let cancels = b.agent.js("probe.cancels").await?;

    // Print dialog.
    let before_print = blocked(b);
    b.agent.js("window.print(); true").await?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while blocked(b) == before_print && Instant::now() < deadline {
        sleep(25).await;
    }
    ensure(blocked(b) > before_print, || {
        "print dialog was not intercepted".into()
    })?;

    // Permission prompt: denied, no PermissionRequest event.
    b.agent.drain();
    b.agent
        .js("window.__geo = 'pending'; navigator.geolocation.getCurrentPosition(() => window.__geo = 'ok', (e) => window.__geo = 'err' + e.code); true")
        .await?;
    wait_js(&b.agent, "window.__geo", &json!("err1"), 3000).await?;
    let mut asked = false;
    while let Some(e) = b.agent.ev.try_next() {
        asked |= matches!(e, PageEvent::PermissionRequest { .. });
    }
    ensure(!asked, || "a PermissionRequest event was emitted".into())?;
    let same = user_unchanged(b).await?;
    Ok(format!(
        "select kept (ArrowDown+Enter -> trusted change, type-ahead, Escape), file chooser cancelled (cancel events {cancels}), print swallowed, geolocation denied silently; {} blocked; {same}",
        blocked(b) - start
    ))
}

async fn check_page_focus(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let q = "document.hasFocus()";
    let before = b.agent.js(q).await?;
    b.backstage.set_page_focus(true);
    wait_js(&b.agent, q, &json!(true), 1000).await?;
    let same = user_unchanged(b).await?;
    b.backstage.set_page_focus(false);
    wait_js(&b.agent, q, &json!(false), 1000).await?;
    Ok(format!(
        "hasFocus {before} -> true -> false without GTK focus; {same}"
    ))
}

async fn check_handoff(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let w = b.agent.page.widget();
    // The user picks the agent's tab.
    let stack = b.stack.clone();
    let placed = b.backstage.hand_off_with(&b.agent.page, |w| {
        stack.add_named(w, Some("agent"));
    });
    ensure(
        placed && w.parent().as_ref() == Some(b.stack.upcast_ref()),
        || "hand-off did not land in the stack".into(),
    )?;
    b.stack.set_visible_child_name("agent");
    sleep(500).await;
    let st = b.backstage.status();
    ensure(
        w.is_mapped() && st.shown == 1 && st.hosted == 0 && b.backstage.holds(&b.agent.page),
        || format!("after hand-off: mapped {} {st:?}", w.is_mapped()),
    )?;
    // Shown pages are composited in software on the devbox (~39/s ceiling).
    let (shown_raf, vis) = rates(&b.agent).await?;
    ensure(shown_raf >= 30.0 && vis == "visible", || {
        format!("shown page rAF {shown_raf}/s {vis}")
    })?;
    let a = shot(&b.agent, "/tmp/copper-backstage-shown-1.png").await?;
    sleep(300).await;
    let c = shot(&b.agent, "/tmp/copper-backstage-shown-2.png").await?;
    ensure(a.px != c.px, || "shown page snapshot is stale".into())?;
    // The user leaves the tab while the hold is live: the backstage takes
    // the page back.
    b.stack.set_visible_child_name("user");
    b.stack.remove(&w);
    sleep(300).await;
    let st = b.backstage.status();
    ensure(
        w.parent().and_then(|p| p.parent()).as_ref() == Some(b.window.upcast_ref::<gtk4::Widget>())
            && st.hosted == 1
            && st.shown == 0,
        || format!("not taken back: {st:?}"),
    )?;
    let back = held_rate(b).await?;
    ensure(back.ok(), || {
        format!("back in the host: {}", back.describe())
    })?;
    let same = user_unchanged(b).await?;
    Ok(format!(
        "shown: rAF {shown_raf}/s, live snapshot; taken back after the user left: {}; {same}",
        back.describe()
    ))
}

async fn check_stalled(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    // A window that keeps committing (a spinner) while the compositor sends
    // it no frame callbacks (weston stops them for a minimized surface): its
    // frame clock stalls, so only the nudge timer can drive the host.
    let spinner = gtk4::Spinner::new();
    spinner.set_spinning(true);
    spinner.set_size_request(64, 64);
    let stalled = gtk4::Window::builder()
        .title("stalled window")
        .default_width(400)
        .default_height(300)
        .child(&spinner)
        .build();
    stalled.present();
    sleep(300).await;
    stalled.minimize();
    sleep(300).await;
    let nudges = b.backstage.status().nudges;
    b.backstage
        .hold(&b.agent.page, &stalled)
        .map_err(|e| e.to_string())?;
    sleep(500).await;
    let in_stalled =
        ghost_of(&stalled).is_some_and(|g| b.agent.page.widget().parent().as_ref() == Some(&g));
    let held = held_rate(b).await?;
    let st = b.backstage.status();
    // The window closes under the page: the backstage re-homes it in the
    // user's window.
    stalled.destroy();
    sleep(300).await;
    let home =
        ghost_of(&b.window).is_some_and(|g| b.agent.page.widget().parent().as_ref() == Some(&g));
    let after = b.backstage.status();
    ensure(
        in_stalled && home && after.mode == BackstageMode::InWindow && after.hosts == 1,
        || format!("moved to the stalled window {in_stalled}, re-homed {home}: {after:?}"),
    )?;
    ensure(held.ok() && st.nudges > nudges + 100, || {
        format!(
            "stalled host: {}, nudges {nudges} -> {}",
            held.describe(),
            st.nudges
        )
    })?;
    let back = held_rate(b).await?;
    ensure(back.ok(), || format!("back home {}", back.describe()))?;
    Ok(format!(
        "in a window with a stalled frame clock: {}, {} nudges; re-homed when that window closed: {}",
        held.describe(),
        st.nudges - nudges,
        back.describe()
    ))
}

async fn check_fallback(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let tab = &b.agent;
    tab.page.set_trusted_input(false);
    let off = tab.page.trusted_input();
    reset_probe(tab).await?;
    let t = input(tab, InputEvent::click(BTN.0, BTN.1)).await?;
    let p = tab.js("probe.clicks").await?;
    tab.page.set_trusted_input(true);
    ensure(
        off == (TrustedInput {
            pointer: false,
            editing: false,
            page_focus: false,
        }) && t == Trust::Script
            && p[0]["t"] == false,
        || format!("{off:?} {t:?} {p}"),
    )?;
    ensure(tab.page.trusted_input().pointer, || {
        "trusted input not restored".into()
    })?;
    Ok(format!("script path: {t:?}, click isTrusted {}", p[0]["t"]))
}

async fn check_release(ctx: &mut Ctx) -> Res {
    let b = bench(ctx)?;
    let w = b.agent.page.widget();
    b.backstage.release(&b.agent.page);
    let st = b.backstage.status();
    ensure(
        !b.backstage.holds(&b.agent.page)
            && w.parent().is_none()
            && st.held == 0
            && st.mode == BackstageMode::Idle,
        || format!("after release: parent {:?} {st:?}", w.parent()),
    )?;
    // Back to an ordinary background tab: hidden to WebKit.
    wait_js(&b.agent, "document.visibilityState", &json!("hidden"), 3000).await?;
    sleep(100).await;
    let same = user_unchanged(b).await?;
    Ok(format!("released: hidden background tab, {st:?}; {same}"))
}

async fn check_warnings(ctx: &mut Ctx) -> Res {
    let b = ctx.bench.take().ok_or("no bench")?;
    let all = GTK_WARNINGS.lock().unwrap().clone();
    let ours = &all[b.warnings_at_start.min(all.len())..];
    b.window.destroy();
    ensure(ours.is_empty(), || format!("GTK warnings: {ours:?}"))?;
    Ok(format!(
        "no GTK warnings during the backstage checks ({} before)",
        b.warnings_at_start
    ))
}
