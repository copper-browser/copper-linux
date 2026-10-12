//! T-002 spike: off-screen agent tabs and trusted input on WebKitGTK 6.0.
//!
//! `backstage --selftest`    runs every rung below, each in its own process
//!                           (`backstage --rung NAME`), prints the check
//!                           table and the D-006 decision, and exits 0 if at
//!                           least one rendering approach passes.
//! `backstage --rung NAME`   runs one rung and prints `RESULT {json}`.
//!
//! Rendering ladder (plan T-002): (a) a WebView kept in the user's window but
//! not shown — a1 as a non-visible GtkStack page, a2 mapped under a GhostHost
//! that snapshots it into a discarded snapshot; (b) a second toplevel — b1
//! never presented, b2 presented, b3 presented then minimized; (c) a nested
//! headless compositor opened as a second GdkDisplay.
//! Input ladder: (1) WebKitAutomationSession over the remote-inspector
//! socket; (2) GtkEventController signal emission + WebKit editing commands;
//! (3) JS-dispatched events.
//!
//! Throwaway code: it measures, it does not ship.

mod automation;
mod ghost;
mod rung;
mod server;

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const RENDER_RUNGS: [&str; 12] = [
    "z-baseline-visible",
    "a1-stack-hidden",
    "a2-ghost-host",
    "b1-hidden-toplevel",
    "b2-second-toplevel",
    "b3-minimized-toplevel",
    "c-nested-compositor",
    "c2-nested-ghost",
    "x-ghost-stalled",
    "x-ghost-selfdrive-stalled",
    "x-handoff-ghost",
    "x-handoff-nested",
];
const INPUT_RUNGS: [&str; 2] = ["input-automation", "input-signals"];
const CHILD_TIMEOUT: Duration = Duration::from_secs(90);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--rung") {
        let name = args.get(i + 1).cloned().unwrap_or_default();
        child(&name);
    } else if args.iter().any(|a| a == "--selftest") {
        let only: Vec<String> = args
            .iter()
            .position(|a| a == "--only")
            .and_then(|i| args.get(i + 1))
            .map(|s| s.split(',').map(String::from).collect())
            .unwrap_or_default();
        std::process::exit(selftest(&only));
    } else {
        eprintln!("usage: backstage --selftest [--only rung,rung] | --rung NAME");
        std::process::exit(2);
    }
}

// ---------------------------------------------------------------------------
// child

fn child(name: &str) {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(80));
        println!(
            "RESULT {}",
            json!({"error": "child watchdog fired after 80 s"})
        );
        std::process::exit(3);
    });
    let _ = std::fs::create_dir_all("/tmp/copper-backstage");
    let srv = server::start().expect("start local http server");
    gtk4::init().expect("gtk init (is WAYLAND_DISPLAY set?)");
    // A blinking caret in the user's field would show up in screen diffs.
    if let Some(s) = gtk4::Settings::default() {
        s.set_gtk_cursor_blink(false);
    }
    let name = name.to_string();
    let main_loop = glib::MainLoop::new(None, false);
    glib::MainContext::default().spawn_local(async move {
        let res = match name.as_str() {
            "input-signals" => rung::input_signals(&srv.base).await,
            "input-automation" => rung::input_automation(&srv.base).await,
            r => rung::render(r, &srv.base).await,
        };
        let out = res.unwrap_or_else(|e| json!({"rung": name, "error": e, "pass": false}));
        println!("RESULT {out}");
        std::process::exit(0);
    });
    main_loop.run();
}

// ---------------------------------------------------------------------------
// parent

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(47123)
}

fn run_child(name: &str) -> Value {
    let exe = std::env::current_exe().expect("current_exe");
    let log = format!("/tmp/copper-backstage/{name}.log");
    let stderr = std::fs::File::create(&log)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    let mut cmd = Command::new(exe);
    cmd.args(["--rung", name])
        .stdout(Stdio::piped())
        .stderr(stderr);
    if let Some(sock) = PRIVATE_WL.with(|c| c.borrow().clone()) {
        cmd.env("WAYLAND_DISPLAY", sock);
    }
    if name == "input-automation" {
        cmd.env(
            "WEBKIT_INSPECTOR_SERVER",
            format!("127.0.0.1:{}", free_port()),
        );
    }
    let t0 = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return json!({"rung": name, "error": format!("spawn: {e}")}),
    };
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() > CHILD_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return json!({"rung": name, "error": format!("timed out after {CHILD_TIMEOUT:?}; log {log}")});
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return json!({"rung": name, "error": format!("wait: {e}")}),
        }
    }
    let mut stdout = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    let mut v = stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix("RESULT "))
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or_else(|| json!({"rung": name, "error": format!("no RESULT line; log {log}")}));
    v["wall_s"] = json!((t0.elapsed().as_secs_f64() * 10.0).round() / 10.0);
    v["log"] = json!(log);
    v
}

fn s(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn yn(v: &Value) -> &'static str {
    match v {
        Value::Bool(true) => "yes",
        Value::Bool(false) => "no",
        _ => "-",
    }
}

/// A private headless weston for the selftest, started with --debug so the
/// rungs can take compositor screenshots, and with no panel (its clock would
/// change the screen between two captures).
fn private_compositor() -> Option<std::process::Child> {
    let runtime = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let sock = format!("copper-backstage-wl-{}", std::process::id());
    let ini = "/tmp/copper-backstage/weston.ini";
    std::fs::write(
        ini,
        "[core]\nidle-time=0\n[shell]\npanel-position=none\nbackground-color=0xff1d2330\n",
    )
    .ok()?;
    let log = std::fs::File::create("/tmp/copper-backstage/weston.log").ok()?;
    let child = Command::new("weston")
        .args([
            "--backend=headless",
            "--renderer=pixman",
            "--debug",
            &format!("--config={ini}"),
            &format!("--socket={sock}"),
            "--width=1440",
            "--height=900",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone().ok()?)
        .stderr(log)
        .spawn()
        .ok()?;
    let path = std::path::Path::new(&runtime).join(&sock);
    let t0 = Instant::now();
    while !path.exists() {
        if t0.elapsed() > Duration::from_secs(5) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    PRIVATE_WL.with(|c| *c.borrow_mut() = Some(sock));
    Some(child)
}

thread_local! {
    static PRIVATE_WL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn selftest(only: &[String]) -> i32 {
    let _ = std::fs::create_dir_all("/tmp/copper-backstage");
    let mut compositor = private_compositor();
    match PRIVATE_WL.with(|c| c.borrow().clone()) {
        Some(s) => {
            eprintln!("backstage: rungs run on a private weston --debug (WAYLAND_DISPLAY={s})")
        }
        None => eprintln!(
            "backstage: private weston failed; using the session compositor (no screen diffs)"
        ),
    }
    let wanted = |r: &str| only.is_empty() || only.iter().any(|o| o == r);
    let mut results: Vec<Value> = Vec::new();
    for r in RENDER_RUNGS
        .iter()
        .chain(INPUT_RUNGS.iter())
        .filter(|r| wanted(r))
    {
        eprintln!("backstage: running {r} ...");
        results.push(run_child(r));
    }
    if let Some(c) = compositor.as_mut() {
        let _ = c.kill();
        let _ = c.wait();
    }
    let dump = serde_json::to_string_pretty(&results).unwrap_or_default();
    let _ = std::fs::write("/tmp/copper-backstage/results.json", &dump);

    println!(
        "RENDERING (agent page in the hidden host; pass = rAF>=50/s, snapshot non-blank+live, user tab/focus/pixels unchanged, not shown to the user)"
    );
    println!(
        "{:<22} {:>7} {:>9} {:<10} {:<22} {:<10} {:<9} {:>8}  verdict",
        "rung",
        "rAF/s",
        "timers/s",
        "visibility",
        "snapshot colors/live",
        "user-same",
        "on-screen",
        "user rAF"
    );
    let mut chosen: Option<String> = None;
    for v in results.iter().filter(|v| {
        v["kind"] == "render" || RENDER_RUNGS.contains(&v["rung"].as_str().unwrap_or(""))
    }) {
        let rung = s(&v["rung"]);
        let pass = v["pass"] == true;
        if pass && chosen.is_none() && v["experiment"] != true {
            chosen = Some(rung.clone());
        }
        let snap = &v["snapshot"];
        let snap_s = if snap["ok"] == true {
            format!("{} / {}", s(&snap["colors"]), yn(&snap["live"]))
        } else if snap.is_null() {
            "-".into()
        } else {
            format!("ERR {}", s(&snap["error"]))
        };
        println!(
            "{:<22} {:>7} {:>9} {:<10} {:<22} {:<10} {:<9} {:>8}  {}",
            rung,
            s(&v["agent"]["raf_per_s"]),
            s(&v["agent"]["timers_per_s"]),
            s(&v["agent"]["visibility"]),
            snap_s,
            yn(&v["user"]["unchanged"]),
            yn(&v["user"]["visible_to_user"]),
            s(&v["user_raf_per_s"]),
            if rung.starts_with("z-") {
                "REF".to_string()
            } else if pass {
                "PASS".to_string()
            } else {
                format!(
                    "FAIL{}",
                    v.get("error")
                        .map(|e| format!(" ({})", s(e)))
                        .unwrap_or_default()
                )
            }
        );
    }
    for v in results.iter().filter(|v| v["after_handoff"].is_object()) {
        let h = &v["after_handoff"];
        println!(
            "  {} hand-off to the user's stack: shown={} rAF/s {} -> {} (visible baseline {}), snapshot live={}",
            s(&v["rung"]),
            yn(&h["shown"]),
            s(&v["agent"]["raf_per_s"]),
            s(&h["raf_per_s"]),
            results
                .iter()
                .find(|r| r["rung"] == "z-baseline-visible")
                .map(|r| s(&r["agent"]["raf_per_s"]))
                .unwrap_or_else(|| "-".into()),
            yn(&h["snapshot"]["live"]),
        );
    }

    println!();
    println!(
        "INPUT (agent page hosted a2-style; t = every event isTrusted, f = some untrusted, - = no event)"
    );
    println!(
        "{:<28} {:<7} {:<7} {:<7} {:<7} {:<7} {:<7} {:<7} {:<16} {:<9}",
        "method",
        "hover",
        "click",
        "dblclk",
        "focus",
        "keys",
        "input",
        "submit",
        "field value",
        "user-same"
    );
    let tf = |v: &Value| match v {
        Value::Bool(true) => "t",
        Value::Bool(false) => "f",
        _ => "-",
    };
    let row = |label: &str,
               hover: &Value,
               click: &Value,
               dbl: &Value,
               focus: &Value,
               typed: &Value,
               submit: &Value,
               user: &Value| {
        let moves = if hover["moves"].as_u64().unwrap_or(0) == 0 {
            "-"
        } else if hover["moves"] == hover["moves_trusted"] {
            "t"
        } else {
            "f"
        };
        println!(
            "{:<28} {:<7} {:<7} {:<7} {:<7} {:<7} {:<7} {:<7} {:<16} {:<9}",
            label,
            moves,
            tf(&click["click_trusted"]),
            tf(&dbl["dblclick_trusted"]),
            tf(&focus["focus_trusted"]),
            tf(&typed["keys_trusted"]),
            tf(&typed["input_trusted"]),
            tf(&submit["submit_trusted"]),
            s(&typed["value"]),
            yn(user),
        );
    };
    for v in &results {
        match v["rung"].as_str() {
            Some("input-automation") => {
                if let Some(e) = v.get("error") {
                    println!("{:<28} ERROR {}", "(1) automation session", s(e));
                    continue;
                }
                let a = &v["automation"];
                row(
                    "(1) automation session",
                    &a["hover"],
                    &a["click"],
                    &a["dblclick"],
                    &a["focus_field"],
                    &a["type_hi"],
                    &a["enter"],
                    &v["user"]["unchanged"],
                );
                println!(
                    "{:<28} right-click contextmenu trusted={} | navigator.webdriver={} | session ephemeral={} | contexts={}",
                    "",
                    tf(&a["right_click"]["contextmenu_trusted"]),
                    s(&v["navigator_webdriver"]),
                    s(&v["agent_session_ephemeral"]),
                    s(&v["browsing_contexts"])
                );
            }
            Some("input-signals") => {
                if let Some(e) = v.get("error") {
                    println!("{:<28} ERROR {}", "(2) controller signals", s(e));
                    continue;
                }
                let g = &v["signals"];
                let ed = &v["editing"]["InsertText(hello)"];
                row(
                    "(2a) controller signals",
                    &g["hover"],
                    &g["click"],
                    &g["dblclick"],
                    &g["focus_field"],
                    &Value::Null,
                    &Value::Null,
                    &v["user"]["unchanged"],
                );
                row(
                    "(2b) editing InsertText",
                    &Value::Null,
                    &Value::Null,
                    &Value::Null,
                    &Value::Null,
                    ed,
                    &Value::Null,
                    &v["user"]["unchanged"],
                );
                println!(
                    "{:<28} click pointerType={} | DeleteBackward value={} input={} | InsertNewline submit={}",
                    "",
                    s(&g["click"]["click_pointer_type"]),
                    s(&v["editing"]["DeleteBackward"]["value"]),
                    tf(&v["editing"]["DeleteBackward"]["input_trusted"]),
                    tf(&v["editing"]["InsertNewline"]["submit_trusted"]),
                );
                let j = &v["js"];
                row(
                    "(3) JS-dispatched",
                    j,
                    j,
                    j,
                    j,
                    j,
                    j,
                    &v["user"]["unchanged"],
                );
            }
            _ => {}
        }
    }

    println!();
    match &chosen {
        Some(r) => {
            println!("RENDER DECISION: {r} (first rung in ladder order that passes every check)")
        }
        None => println!("RENDER DECISION: none passed — BLOCKED"),
    }
    println!("details: /tmp/copper-backstage/results.json, snapshots /tmp/copper-backstage/*.png");
    if chosen.is_some() { 0 } else { 1 }
}
