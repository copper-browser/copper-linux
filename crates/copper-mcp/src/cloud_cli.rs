//! `copper cloud pair|sso|status` — Copper Cloud from the command line,
//! with no window. A headless Copper (an agent's own browser) is paired this
//! way before it starts: the files a window reads are written for the
//! current `COPPER_WORLD`, then the window syncs as soon as it opens.
//!
//! The work is [`copper_cloud::offline`]'s; this is the command line around
//! it — flags, words, `--json`, and the refusal while a running Copper owns
//! the profile (a window keeps its own copy of `cloud.json` and would write
//! over a pairing done under it). Nothing here prints a token or a key.
//!
//! Exit codes: 0 done, 1 the cloud refused or couldn't be reached, 2 usage
//! or a running Copper owns this profile (`--json`: `"error":"running"`).

use std::io::{self, Write};
use std::time::Duration;

use copper_cloud::Failure;
use copper_cloud::offline::{self, JoinOptions, Joined, SyncChoice};
use copper_cloud::sso::SsoCancel;
use copper_core::Store;
use copper_pagejs::render::pretty_json;
use serde_json::{Value, json};

use crate::config;
use crate::intelligence::{Effective, read_files};

pub const USAGE: &str = "Usage: copper cloud <command> [--json]

  pair CODE [--sync none|all|LIST] [--name NAME] [--trust FP]
                     link and sign in with a pairing code (copper-cloud://…#p=…), with no
                     window: writes this profile's cloud.json and sync switches, so Copper
                     syncs as soon as it starts. Copper must not be running on this profile.
  sso [CODE] [--provider ID] [--sync …] [--name NAME] [--trust FP]
                     sign in with your company account in the system browser: a sign-in
                     link (copper-cloud://…#sso=google), a link code, or none (the instance
                     this Copper is connected to)
  status             the link, the account, this device, the sync switches, and whether
                     model keys are there (yes or no; never a key)

  --sync             what syncs: none, all, or a list of spaces,settings,bookmarks,tabs,
                     history,canvas. Default: the switches this profile had, else all.
  --name NAME        this device's name, here and on the server
  --trust FP         pin a certificate a code carried no fingerprint for, once you have
                     compared it with `copper-cloud doctor` on the server
  --json             print one JSON object: {\"ok\":true,\"email\",\"device_id\",…} or
                     {\"ok\":false,\"error\",\"message\"}

COPPER_WORLD=<name> picks the profile, as for the browser. Exit codes: 0 done, 1 the cloud
refused or couldn't be reached, 2 usage or a running Copper owns this profile.
";

/// What one `copper cloud …` asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Command {
    pub verb: Verb,
    pub code: Option<String>,
    pub json: bool,
    pub sync: SyncChoice,
    pub name: Option<String>,
    pub trust: Option<String>,
    pub provider: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verb {
    #[default]
    Help,
    Pair,
    Sso,
    Status,
}

/// Whether the first word, past the CLI's own flags, is `cloud`.
pub fn is_cloud(args: &[String]) -> bool {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--json" | "--dry-run" | "--launch" => i += 1,
            "--tab" => i += 2,
            word => return word == "cloud",
        }
    }
    false
}

/// Read a `copper [flags] cloud …` command line.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let at = args
        .iter()
        .position(|a| a == "cloud")
        .map_or(0, |at| at + 1);
    let mut cmd = Command {
        json: args[..at.saturating_sub(1).min(args.len())]
            .iter()
            .any(|a| a == "--json"),
        ..Default::default()
    };
    let mut words: Vec<String> = vec![];
    let mut it = args[at.min(args.len())..].iter().cloned();
    let mut sync_given = None;
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--json" => cmd.json = true,
            "--sync" => sync_given = Some(value("--sync")?),
            "--name" => cmd.name = Some(value("--name")?),
            "--trust" => cmd.trust = Some(value("--trust")?),
            "--provider" => cmd.provider = Some(value("--provider")?),
            "-h" | "--help" => {
                cmd.verb = Verb::Help;
                return Ok(cmd);
            }
            f if f.starts_with("--") => return Err(format!("unknown option {f}")),
            _ => words.push(a),
        }
    }
    let mut words = words.into_iter();
    cmd.verb = match words.next().as_deref() {
        None | Some("help") => Verb::Help,
        Some("pair") => Verb::Pair,
        Some("sso") => Verb::Sso,
        Some("status") => Verb::Status,
        Some(other) => return Err(format!("unknown cloud command: {other}")),
    };
    cmd.code = words.next();
    if let Some(extra) = words.next() {
        return Err(format!("unexpected argument: {extra}"));
    }
    if let Some(text) = sync_given {
        cmd.sync = SyncChoice::parse(&text)?;
    }
    match cmd.verb {
        Verb::Pair if cmd.code.is_none() => {
            return Err("pair needs CODE (copper-cloud://…#p=…)".into());
        }
        Verb::Status if cmd.code.is_some() => return Err("status takes no code".into()),
        _ => {}
    }
    Ok(cmd)
}

/// Where `copper cloud` reads and writes, and how it opens a browser.
pub struct CloudEnv<'a> {
    /// The profile (`Store::from_env`: the current `COPPER_WORLD`).
    pub store: Store,
    /// Open the system browser at a URL (`xdg-open` on Linux).
    pub open: &'a dyn Fn(&str) -> Result<(), String>,
    /// Why this profile is busy: a running Copper owns it.
    pub running: &'a dyn Fn(&Store) -> Option<String>,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// `copper cloud …` on the real environment; `open` opens the browser.
pub fn run(args: &[String], open: &dyn Fn(&str) -> Result<(), String>) -> i32 {
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let mut env = CloudEnv {
        store: Store::from_env(),
        open,
        running: &running_here,
        out: &mut out,
        err: &mut err,
    };
    run_with(args, &mut env)
}

/// `copper cloud …` against any profile and streams.
pub fn run_with(args: &[String], env: &mut CloudEnv<'_>) -> i32 {
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(text) => {
            let _ = writeln!(env.err, "copper: {text}");
            let _ = writeln!(env.err, "(copper cloud help shows the commands)");
            return 2;
        }
    };
    match cmd.verb {
        Verb::Help => {
            let _ = write!(env.out, "{USAGE}");
            0
        }
        Verb::Status => status(env, &cmd),
        Verb::Pair | Verb::Sso => join(env, &cmd),
    }
}

fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn join(env: &mut CloudEnv<'_>, cmd: &Command) -> i32 {
    if let Some(why) = (env.running)(&env.store) {
        let message = format!(
            "{why}. Quit it first (a window keeps its own copy of cloud.json and would write over this), or pair it in Settings › Cloud."
        );
        return refuse(env, cmd, "running", &message, None, 2);
    }
    let opts = JoinOptions {
        sync: cmd.sync.clone(),
        name: cmd.name.clone(),
        trust: cmd.trust.clone(),
        provider: cmd.provider.clone(),
    };
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(e) => return refuse(env, cmd, "runtime", &e.to_string(), None, 1),
    };
    let store = env.store.clone();
    let result = match cmd.verb {
        Verb::Pair => rt.block_on(offline::pair(
            store,
            cmd.code.as_deref().unwrap_or(""),
            &opts,
        )),
        _ => {
            let opener = env.open;
            let err = &mut *env.err;
            let open = move |url: &str| -> Result<(), String> {
                let _ = writeln!(
                    err,
                    "Opening your browser to sign in. If it doesn't open, visit:\n\n  {url}\n\nWaiting up to 5 minutes…"
                );
                if let Err(e) = opener(url) {
                    let _ = writeln!(
                        err,
                        "(couldn't open a browser: {e} — open the address above yourself)"
                    );
                }
                Ok(())
            };
            rt.block_on(offline::sso(
                store,
                cmd.code.as_deref(),
                &opts,
                open,
                &SsoCancel::new(),
            ))
        }
    };
    // What the sign-in left running (the event stream, the server info)
    // stops with the runtime; nothing it does needs to finish.
    rt.shutdown_timeout(Duration::from_millis(200));
    match result {
        Ok(joined) => {
            print_joined(env, cmd, &joined);
            0
        }
        Err(f) => failed(env, cmd, &f),
    }
}

fn print_joined(env: &mut CloudEnv<'_>, cmd: &Command, joined: &Joined) {
    if cmd.json {
        let mut v = serde_json::to_value(joined).unwrap_or_else(|_| json!({}));
        if let Some(o) = v.as_object_mut() {
            o.insert("ok".into(), json!(true));
        }
        print_json(env, &v);
        return;
    }
    let verb = if cmd.verb == Verb::Pair {
        "Paired with"
    } else {
        "Signed in to"
    };
    let mut text = format!("{verb} {} as {}", joined.host, joined.email);
    if joined.created == Some(true) {
        text.push_str(" (a new account)");
    }
    let _ = writeln!(env.out, "{text}.");
    let _ = writeln!(
        env.out,
        "This device: {} ({})",
        joined.device_name, joined.device_id
    );
    let _ = writeln!(
        env.out,
        "Sync: {}",
        if joined.sync.is_empty() {
            "off".to_string()
        } else {
            format!("on — {}", joined.sync.join(", "))
        }
    );
    let _ = writeln!(
        env.out,
        "The cloud provides: {}",
        if joined.provided.is_empty() {
            "no model keys".to_string()
        } else {
            format!("{} keys", joined.provided.join(" and "))
        }
    );
    for w in &joined.warnings {
        let _ = writeln!(env.err, "copper: note: {w}");
    }
}

fn failed(env: &mut CloudEnv<'_>, cmd: &Command, f: &Failure) -> i32 {
    let mut message = f.message.clone();
    if f.code == "untrusted"
        && let Some(seen) = &f.seen
    {
        message = format!(
            "{message}\nThe instance showed this certificate (SHA-256):\n  {}\nIf it matches, run again with --trust {seen}",
            pairs(seen)
        );
    }
    refuse(env, cmd, &f.code, &message, f.seen.as_deref(), 1)
}

fn refuse(
    env: &mut CloudEnv<'_>,
    cmd: &Command,
    code: &str,
    message: &str,
    seen: Option<&str>,
    exit: i32,
) -> i32 {
    if cmd.json {
        let mut v = json!({"ok": false, "error": code, "message": message});
        if let Some(seen) = seen {
            v["fingerprint"] = json!(seen);
        }
        print_json(env, &v);
    } else {
        let _ = writeln!(env.err, "copper: {message}");
    }
    exit
}

fn pairs(fp: &str) -> String {
    fp.as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

fn print_json(env: &mut CloudEnv<'_>, value: &Value) {
    let _ = writeln!(env.out, "{}", pretty_json(value).replace("\\/", "/"));
}

fn yes(v: bool) -> &'static str {
    if v { "yes" } else { "no" }
}

fn status(env: &mut CloudEnv<'_>, cmd: &Command) -> i32 {
    let st = offline::status(&env.store);
    let eff = Effective::new(&read_files(&env.store), false);
    let source = |key: &str| {
        eff.sources
            .get(key)
            .cloned()
            .unwrap_or_else(|| "none".into())
    };
    let running = (env.running)(&env.store);
    if cmd.json {
        let mut v = serde_json::to_value(&st).unwrap_or_else(|_| json!({}));
        if let Some(o) = v.as_object_mut() {
            o.insert("ok".into(), json!(true));
            o.insert(
                "intelligence".into(),
                json!({
                    "jev": eff.jev_ready(),
                    "jev_source": source("jevKey"),
                    "router": eff.router_ready(),
                    "router_source": source("routerKey"),
                }),
            );
            o.insert("running".into(), json!(running.is_some()));
        }
        print_json(env, &v);
        return 0;
    }
    let line = match (&st.host, st.signed_in) {
        (Some(host), true) => format!(
            "Copper Cloud: signed in to {host} as {}",
            st.email.as_deref().unwrap_or("?")
        ),
        (Some(host), false) => format!("Copper Cloud: connected to {host}, not signed in"),
        (None, _) => "Copper Cloud: not connected".to_string(),
    };
    let _ = writeln!(env.out, "{line}");
    if st.linked {
        let _ = writeln!(
            env.out,
            "Certificate: {}",
            if st.pinned {
                "pinned by fingerprint"
            } else {
                "public authorities (or a loopback server)"
            }
        );
    }
    if st.device_id.is_empty() {
        let _ = writeln!(env.out, "This device: not set up yet");
    } else {
        let _ = writeln!(
            env.out,
            "This device: {} ({})",
            st.device_name, st.device_id
        );
    }
    let on = st.sync.domains();
    let _ = writeln!(
        env.out,
        "Sync: {}",
        match (st.sync.on, on.is_empty()) {
            (true, false) => format!("on — {}", on.join(", ")),
            (true, true) => "on, but every switch is off".into(),
            (false, false) => format!("off (switches: {})", on.join(", ")),
            (false, true) => "off".into(),
        }
    );
    let _ = writeln!(
        env.out,
        "Model keys: Jev {} ({}), router {} ({})",
        yes(eff.jev_ready()),
        source("jevKey"),
        yes(eff.router_ready()),
        source("routerKey")
    );
    if let Some(why) = running {
        let _ = writeln!(env.out, "Running: {why}");
    }
    0
}

/// Whether a running Copper owns `store`'s profile, and how that is known:
/// its agent server answers with this profile's token, or (Linux) a Copper
/// window process runs with this profile's data folder.
pub fn running_here(store: &Store) -> Option<String> {
    if let Some(port) = agent_answers(store) {
        return Some(format!(
            "Copper is running on this profile (its agent server answers on 127.0.0.1:{port})"
        ));
    }
    #[cfg(target_os = "linux")]
    if let Some(pid) = linux::window_on(store) {
        return Some(format!("Copper is running on this profile (pid {pid})"));
    }
    None
}

/// The port, when the profile's agent server is up and takes its token.
fn agent_answers(store: &Store) -> Option<u16> {
    let mut conf = config::endpoint(store)?;
    if let Some(port) = std::env::var("COPPER_AGENT_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
    {
        conf.port = port;
    }
    if !conf.enabled || conf.token.is_empty() {
        return None;
    }
    let timeout = Duration::from_millis(800);
    let health = crate::client::request(conf.port, "GET", "/health", &[], &[], timeout).ok()?;
    let named: Value = serde_json::from_slice(&health.body).ok()?;
    if health.status != 200 || named["name"] != "copper" {
        return None;
    }
    // Another profile's Copper may hold the same port: only this one's
    // token is accepted.
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let headers = [
        ("Content-Type", "application/json".to_string()),
        ("Authorization", format!("Bearer {}", conf.token)),
    ];
    let answer =
        crate::client::request(conf.port, "POST", config::PATH, &headers, body, timeout).ok()?;
    (!matches!(answer.status, 401 | 403)).then_some(conf.port)
}

/// Whether a word is a command-line verb (`copper tabs`), not an address a
/// window was started with.
pub fn is_cli_word(word: &str) -> bool {
    if matches!(word, "cloud" | "help") || crate::cli::ELSEWHERE.contains(&word) {
        return true;
    }
    !matches!(
        crate::cli::make_request(word, &[]),
        Err(crate::cli::Early::Usage(t)) if t.starts_with("unknown command")
    )
}

/// Whether a Copper started with these arguments is a window (it owns its
/// profile), not the command line or the stdio bridge.
pub fn is_window(args: &[String]) -> bool {
    if args.iter().any(|a| a == "--mcp-stdio" || a == "--cli") {
        return false;
    }
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--json" | "--dry-run" | "--launch" => i += 1,
            "--tab" => i += 2,
            "--version" | "-V" | "--help" | "-h" => return false,
            word => return word.starts_with('-') || !is_cli_word(word),
        }
    }
    true
}

#[cfg(target_os = "linux")]
mod linux {
    use copper_core::{Paths, Store};
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    fn canonical(path: PathBuf) -> PathBuf {
        std::fs::canonicalize(&path).unwrap_or(path)
    }

    /// A Copper window process (not this one) whose environment puts its
    /// data folder where `store`'s is.
    pub(super) fn window_on(store: &Store) -> Option<u32> {
        let want = canonical(store.paths().data.clone());
        let me = std::process::id();
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == me {
                continue;
            }
            let dir = entry.path();
            match std::fs::read_to_string(dir.join("comm")) {
                Ok(comm) if comm.trim() == "copper" => {}
                _ => continue,
            }
            let Ok(cmdline) = std::fs::read(dir.join("cmdline")) else {
                continue;
            };
            if cmdline.is_empty() {
                continue; // a zombie
            }
            let args: Vec<String> = cmdline
                .split(|b| *b == 0)
                .skip(1)
                .filter(|a| !a.is_empty())
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect();
            if !super::is_window(&args) {
                continue;
            }
            let Ok(environ) = std::fs::read(dir.join("environ")) else {
                continue;
            };
            let vars: HashMap<String, OsString> = environ
                .split(|b| *b == 0)
                .filter_map(|pair| {
                    let at = pair.iter().position(|b| *b == b'=')?;
                    let key = String::from_utf8(pair[..at].to_vec()).ok()?;
                    Some((key, OsString::from_vec(pair[at + 1..].to_vec())))
                })
                .collect();
            let theirs = canonical(Paths::from_vars(|k| vars.get(k).cloned()).data);
            if theirs == want {
                return Some(pid);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::Paths;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn commands_parse() {
        assert!(is_cloud(&s(&["cloud", "status"])));
        assert!(is_cloud(&s(&["--json", "cloud", "pair", "x"])));
        assert!(!is_cloud(&s(&["cloud-history", "delete"])));
        assert!(!is_cloud(&s(&["example.com"])));
        let c = parse(&s(&[
            "cloud",
            "pair",
            "copper-cloud://h:1/#p=cp_x",
            "--sync",
            "tabs,bookmarks",
            "--name=FluxBot · Vera",
            "--json",
        ]))
        .unwrap();
        assert_eq!(c.verb, Verb::Pair);
        assert_eq!(c.code.as_deref(), Some("copper-cloud://h:1/#p=cp_x"));
        assert!(c.json);
        assert_eq!(c.name.as_deref(), Some("FluxBot · Vera"));
        assert_eq!(c.sync, SyncChoice::parse("bookmarks,tabs").unwrap());
        let c = parse(&s(&["--json", "cloud", "status"])).unwrap();
        assert!(c.json && c.verb == Verb::Status);
        let c = parse(&s(&["cloud", "sso", "--provider", "google"])).unwrap();
        assert_eq!(
            (c.verb, c.code, c.provider.as_deref()),
            (Verb::Sso, None, Some("google"))
        );
        assert_eq!(parse(&s(&["cloud"])).unwrap().verb, Verb::Help);
        assert_eq!(parse(&s(&["cloud", "--help"])).unwrap().verb, Verb::Help);
        assert_eq!(
            parse(&s(&["cloud", "pair", "x", "--sync", "none"]))
                .unwrap()
                .sync,
            SyncChoice::Nothing
        );
    }

    #[test]
    fn usage_errors() {
        for (args, want) in [
            (&["cloud", "pair"][..], "pair needs CODE"),
            (
                &["cloud", "pair", "x", "--sync"][..],
                "--sync needs a value",
            ),
            (
                &["cloud", "pair", "x", "--sync", "passwords"][..],
                "unknown sync choice",
            ),
            (&["cloud", "pair", "x", "y"][..], "unexpected argument: y"),
            (&["cloud", "frob"][..], "unknown cloud command: frob"),
            (
                &["cloud", "status", "--force"][..],
                "unknown option --force",
            ),
            (&["cloud", "status", "x"][..], "status takes no code"),
        ] {
            let err = parse(&s(args)).unwrap_err();
            assert!(err.starts_with(want), "{args:?}: {err}");
        }
    }

    #[test]
    fn windows_and_command_lines() {
        assert!(is_window(&s(&[])));
        assert!(is_window(&s(&["https://example.com"])));
        assert!(is_window(&s(&["rust lifetimes"])));
        assert!(is_window(&s(&["--screenshot", "/tmp/x.png"])));
        assert!(!is_window(&s(&["--mcp-stdio"])));
        assert!(!is_window(&s(&["health"])));
        assert!(!is_window(&s(&["--json", "tabs"])));
        assert!(!is_window(&s(&["cloud", "pair", "x"])));
        assert!(!is_window(&s(&["setup"])));
        assert!(!is_window(&s(&["--version"])));
    }

    fn with_env<R>(
        dir: &std::path::Path,
        running: Option<&str>,
        f: impl FnOnce(&mut CloudEnv<'_>) -> R,
    ) -> (R, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let busy = running.map(str::to_string);
        let running = move |_: &Store| busy.clone();
        let open = |_: &str| Ok(());
        let r = {
            let mut env = CloudEnv {
                store: Store::new(Paths::new(dir.join("data"), dir.join("config"))),
                open: &open,
                running: &running,
                out: &mut out,
                err: &mut err,
            };
            f(&mut env)
        };
        (
            r,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn refuses_while_a_window_owns_the_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let (code, out, _) = with_env(
            tmp.path(),
            Some("Copper is running on this profile (pid 7)"),
            |env| {
                run_with(
                    &s(&["cloud", "pair", "copper-cloud://h:1/#p=cp_x", "--json"]),
                    env,
                )
            },
        );
        assert_eq!(code, 2);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "running");
        assert!(v["message"].as_str().unwrap().contains("pid 7"));
        assert!(
            !tmp.path().join("data/cloud.json").exists(),
            "nothing written"
        );
    }

    #[test]
    fn status_of_a_fresh_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let (code, out, _) = with_env(tmp.path(), None, |env| {
            run_with(&s(&["--json", "cloud", "status"]), env)
        });
        assert_eq!(code, 0);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["linked"], false);
        assert_eq!(v["signed_in"], false);
        assert_eq!(v["sync"]["on"], false);
        assert_eq!(v["running"], false);
        assert!(v["intelligence"]["jev"].is_boolean());
        let (code, out, _) = with_env(tmp.path(), None, |env| {
            run_with(&s(&["cloud", "status"]), env)
        });
        assert_eq!(code, 0);
        assert!(out.starts_with("Copper Cloud: not connected"), "{out}");
        assert!(out.contains("Model keys: Jev "), "{out}");
    }

    #[test]
    fn a_code_that_cannot_pair_is_explained() {
        let tmp = tempfile::tempdir().unwrap();
        let fp = "b".repeat(64);
        let (code, out, _) = with_env(tmp.path(), None, |env| {
            run_with(
                &s(&[
                    "cloud",
                    "pair",
                    &format!("copper-cloud://h:1/#sso=google&fp={fp}"),
                    "--json",
                ]),
                env,
            )
        });
        assert_eq!(code, 1);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"], "sso_link");
        let (code, _, err) = with_env(tmp.path(), None, |env| {
            run_with(&s(&["cloud", "pair"]), env)
        });
        assert_eq!(code, 2);
        assert!(err.contains("pair needs CODE"));
        let (code, out, _) = with_env(tmp.path(), None, |env| {
            run_with(&s(&["cloud", "help"]), env)
        });
        assert_eq!(code, 0);
        assert!(out.starts_with("Usage: copper cloud"));
    }
}
