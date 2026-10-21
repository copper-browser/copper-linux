//! `copper …` — a small, synchronous shell client for the MCP server in the
//! Copper already running (Fork/MCP/CLI.swift). The verbs map one-to-one
//! onto the server's tools, with the same names and arguments, so an agent
//! moves between the command line and MCP without learning a second API.
//!
//! Exit codes: 0 done, 1 the tool answered with an error, 2 usage or Copper
//! unreachable.

use std::io::{self, Write};
use std::time::Duration;

use base64::Engine as _;
use copper_pagejs::render::pretty_json;
use serde_json::{Map, Value, json};

use crate::client;
use crate::config::{self, Endpoint, PATH};
use crate::who;

/// What one command line asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    /// The JSON-RPC request; `None` for `health`.
    pub request: Option<Value>,
    pub health: bool,
    /// `tools`: print the catalogue as lines.
    pub tools_command: bool,
    /// Where a picture goes.
    pub image_path: Option<String>,
    /// `shot`: print only the path.
    pub image_command: bool,
}

/// The parsed command line around a [`Spec`].
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub json: bool,
    pub dry_run: bool,
    pub launch: bool,
    pub spec: Spec,
}

/// What `parse` decided without a request: help, or a usage error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Early {
    Help,
    Usage(String),
}

pub const NOT_RUNNING: &str =
    "Copper isn't running (or Settings › Agents is off). Start it with `copper`, or pass --launch.";

pub const USAGE: &str = "Usage: copper [--json] [--launch] [--tab ID] <command> [arguments]

  tabs                                      list open tabs, with ids; \"yours\" is where your calls land
  open URL [--focus]                        open URL in a background tab of your own; the user's
                                            tab stays put (--focus shows it to them)
  use [ID]                                  make a tab yours, in the background (no ID: the user's)
  show [ID]                                 make a tab yours and switch the user's window to it
  go URL                                    navigate your tab
  signin [--account USER] [--otp] [--no-submit]
                                            fill a shared saved account on the current tab
  autofill card|identity|field [--name NAME] [--submit]
                                            fill a shared Bitwarden or 1Password card, identity, or field
  observe [--no-text] [-n N]                fast Jev observation
  run \"GOAL…\" [--url URL] [--new-tab [--focus]] [--max N] [--no-elements]
  step \"GOAL…\"                              supervise one Jev decision
  extract \"INSTRUCTION…\" [--full] [--schema JSON]
  snapshot [--interactive] [-n N] [--selector CSS]
  text                                      read page text
  find TEXT                                 find matching page elements
  shot [PATH] [--full] [--jpeg]             save a screenshot
  eval 'JS'                                 evaluate JavaScript
  click REF | --selector CSS                click an element
  type REF TEXT [--submit]                  type into an element
  key KEY                                   press a key
  tools                                     list available tools
  health                                    check the agent server and Jev mode
  cloud-history delete [--since 1h|24h|7d|TIME] [--until TIME] [--host SITE]... [--page URL]... [--device ID|this] [--all]
                                            delete your history on Copper Cloud (every computer's); this
                                            one's stays. A site matches its subdomains; nothing chosen needs --all
  call TOOL [JSON-ARGS]                     call any MCP tool
  cloud pair CODE [--sync none|all|LIST] [--name NAME]
                                            pair this profile with Copper Cloud while Copper is
                                            not running (a headless Copper, before it starts)
  cloud sso [CODE]                          sign in with your company account in the browser
  cloud status                              the cloud link, account and sync switches
                                            (copper cloud help for the options)
  botpass pair --server URL --code CODE [--fingerprint FP] [--name NAME]
                                            pair with BotPass as one of its bots (what
                                            bp connect copper runs); also: botpass status,
                                            botpass unpair (copper botpass help)
  help                                      show this help

Every page command works on your tab: the one you opened or chose with use,
else the tab the user is looking at. A background tab still paints, takes
screenshots and real clicks, and is never brought in front of the user.
--tab ID works in another tab for one call.

Use --json for the raw JSON-RPC result. Pass --launch (or COPPER_LAUNCH=1)
to opt into starting Copper when it is down. (Hidden: --dry-run prints the decision.)
";

/// Commands the Mac has that this build does not (yet).
pub(crate) const ELSEWHERE: [&str; 9] = [
    "setup",
    "session",
    "link",
    "intelligence",
    "claude",
    "bitwarden",
    "crashes",
    "listen",
    "transcript",
];

fn rpc(method: &str, params: Value) -> Spec {
    Spec {
        request: Some(json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })),
        health: false,
        tools_command: method == "tools/list",
        image_path: None,
        image_command: false,
    }
}

fn call(name: &str, arguments: Value) -> Spec {
    rpc(
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
}

fn bad<T>(text: impl Into<String>) -> Result<T, Early> {
    Err(Early::Usage(text.into()))
}

fn number(value: Option<&String>) -> Option<u64> {
    value
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
}

/// The request for one verb (`CLI.makeRequest`).
pub fn make_request(command: &str, args: &[String]) -> Result<Spec, Early> {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    match command {
        "tabs" => {
            if !args.is_empty() {
                return bad("tabs takes no arguments");
            }
            Ok(call("browser_tabs", json!({ "action": "list" })))
        }
        "open" => {
            let focus = has("--focus");
            let rest: Vec<&String> = args.iter().filter(|a| *a != "--focus").collect();
            let [url] = rest.as_slice() else {
                return bad("open needs URL");
            };
            Ok(call(
                "browser_tabs",
                json!({ "action": "new", "url": url, "focus": focus }),
            ))
        }
        "use" | "show" => {
            if args.len() > 1 {
                return bad(format!("{command} takes one tab id or index"));
            }
            let mut arguments = json!({ "action": "select", "focus": command == "show" });
            if let Some(reference) = args.first() {
                arguments["tab"] = json!(reference);
            }
            Ok(call("browser_tabs", arguments))
        }
        "go" => {
            let [url] = args else {
                return bad("go needs URL");
            };
            Ok(call("browser_navigate", json!({ "url": url })))
        }
        "signin" => {
            let mut arguments = Map::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--account" => {
                        i += 1;
                        match args.get(i).filter(|v| !v.is_empty()) {
                            Some(v) => arguments.insert("account".into(), json!(v)),
                            None => return bad("signin --account needs USER"),
                        };
                    }
                    "--otp" => {
                        arguments.insert("what".into(), json!("otp"));
                    }
                    "--no-submit" => {
                        arguments.insert("submit".into(), json!(false));
                    }
                    other => return bad(format!("unknown signin option: {other}")),
                }
                i += 1;
            }
            Ok(call("browser_sign_in", Value::Object(arguments)))
        }
        "autofill" => {
            let Some(kind) = args
                .first()
                .filter(|k| ["card", "identity", "field"].contains(&k.as_str()))
            else {
                return bad("autofill needs card, identity, or field");
            };
            let mut arguments = Map::new();
            arguments.insert("kind".into(), json!(kind));
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--name" => {
                        i += 1;
                        match args.get(i).filter(|v| !v.is_empty()) {
                            Some(v) => arguments.insert("name".into(), json!(v)),
                            None => return bad("autofill --name needs NAME"),
                        };
                    }
                    "--submit" => {
                        arguments.insert("submit".into(), json!(true));
                    }
                    other => return bad(format!("unknown autofill option: {other}")),
                }
                i += 1;
            }
            Ok(call("browser_autofill", Value::Object(arguments)))
        }
        "observe" => {
            let mut arguments = Map::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--no-text" => {
                        arguments.insert("text".into(), json!(false));
                    }
                    "-n" | "--limit" => {
                        i += 1;
                        let Some(n) = number(args.get(i)) else {
                            return bad("observe -n needs a non-negative number");
                        };
                        arguments.insert("limit".into(), json!(n));
                    }
                    other => return bad(format!("unknown observe option: {other}")),
                }
                i += 1;
            }
            Ok(call("jev_observe", Value::Object(arguments)))
        }
        "run" => {
            let mut goal: Vec<&str> = Vec::new();
            let mut arguments = Map::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--url" => {
                        i += 1;
                        match args.get(i).filter(|v| !v.is_empty()) {
                            Some(v) => arguments.insert("url".into(), json!(v)),
                            None => return bad("run --url needs URL"),
                        };
                    }
                    "--new-tab" => {
                        arguments.insert("newTab".into(), json!(true));
                    }
                    "--focus" => {
                        arguments.insert("focus".into(), json!(true));
                    }
                    "--max" => {
                        i += 1;
                        let Some(n) = number(args.get(i)) else {
                            return bad("run --max needs a non-negative number");
                        };
                        arguments.insert("maxSteps".into(), json!(n));
                    }
                    "--no-elements" => {
                        arguments.insert("elements".into(), json!(false));
                    }
                    other if other.starts_with('-') => {
                        return bad(format!("unknown run option: {other}"));
                    }
                    other => goal.push(other),
                }
                i += 1;
            }
            if goal.is_empty() {
                return bad("run needs a goal");
            }
            arguments.insert("goal".into(), json!(goal.join(" ")));
            Ok(call("jev_run", Value::Object(arguments)))
        }
        "step" => {
            if args.is_empty() {
                return bad("step needs a goal");
            }
            if args.iter().any(|a| a.starts_with('-')) {
                return bad("step takes a goal, not options");
            }
            Ok(call("jev_step", json!({ "goal": args.join(" ") })))
        }
        "extract" => {
            let mut instruction: Vec<&str> = Vec::new();
            let mut arguments = Map::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--full" => {
                        arguments.insert("full".into(), json!(true));
                    }
                    "--schema" => {
                        i += 1;
                        let Some(schema) = args
                            .get(i)
                            .and_then(|v| serde_json::from_str::<Value>(v).ok())
                        else {
                            return bad("extract --schema needs valid JSON");
                        };
                        arguments.insert("schema".into(), schema);
                    }
                    other if other.starts_with('-') => {
                        return bad(format!("unknown extract option: {other}"));
                    }
                    other => instruction.push(other),
                }
                i += 1;
            }
            if instruction.is_empty() {
                return bad("extract needs an instruction");
            }
            arguments.insert("instruction".into(), json!(instruction.join(" ")));
            Ok(call("jev_extract", Value::Object(arguments)))
        }
        "snapshot" => {
            let mut arguments = Map::new();
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--interactive" => {
                        arguments.insert("interactive".into(), json!(true));
                    }
                    "-n" | "--limit" => {
                        i += 1;
                        let Some(n) = number(args.get(i)) else {
                            return bad("snapshot -n needs a non-negative number");
                        };
                        arguments.insert("limit".into(), json!(n));
                    }
                    "--selector" => {
                        i += 1;
                        match args.get(i).filter(|v| !v.is_empty()) {
                            Some(v) => arguments.insert("selector".into(), json!(v)),
                            None => return bad("snapshot --selector needs CSS"),
                        };
                    }
                    other => return bad(format!("unknown snapshot option: {other}")),
                }
                i += 1;
            }
            Ok(call("browser_snapshot", Value::Object(arguments)))
        }
        "text" => {
            if !args.is_empty() {
                return bad("text takes no arguments");
            }
            Ok(call("browser_get_text", json!({})))
        }
        "find" => {
            if args.is_empty() {
                return bad("find needs text");
            }
            if args.iter().any(|a| a.starts_with('-')) {
                return bad("find text cannot be an option");
            }
            Ok(call("browser_find", json!({ "text": args.join(" ") })))
        }
        "shot" => {
            let mut path: Option<&String> = None;
            let mut arguments = Map::new();
            arguments.insert("type".into(), json!("png"));
            for arg in args {
                match arg.as_str() {
                    "--full" => {
                        arguments.insert("fullPage".into(), json!(true));
                    }
                    "--jpeg" => {
                        arguments.insert("type".into(), json!("jpeg"));
                    }
                    other => {
                        if other.starts_with('-') || path.is_some() {
                            return bad("shot takes one optional PATH");
                        }
                        path = Some(arg);
                    }
                }
            }
            let suffix = if arguments["type"] == "jpeg" {
                "jpg"
            } else {
                "png"
            };
            let output = path.cloned().unwrap_or_else(|| {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                temp_image(&format!("copper-{now}.{suffix}"))
            });
            arguments.insert("filename".into(), json!(output));
            let mut spec = call("browser_take_screenshot", Value::Object(arguments));
            spec.image_path = Some(output);
            spec.image_command = true;
            Ok(spec)
        }
        "eval" => {
            let [function] = args else {
                return bad("eval needs JavaScript");
            };
            Ok(call("browser_evaluate", json!({ "function": function })))
        }
        "click" => {
            if args.is_empty() {
                return bad("click needs REF or --selector CSS");
            }
            if args[0] == "--selector" {
                if args.len() != 2 {
                    return bad("click --selector needs CSS");
                }
                return Ok(call("browser_click", json!({ "selector": args[1] })));
            }
            if args.len() != 1 {
                return bad("click takes one REF or --selector CSS");
            }
            Ok(call("browser_click", json!({ "ref": args[0] })))
        }
        "type" => {
            if args.len() < 2 {
                return bad("type needs REF TEXT");
            }
            let reference = &args[0];
            let submit = args[1..].iter().any(|a| a == "--submit");
            let text: Vec<&str> = args[1..]
                .iter()
                .filter(|a| *a != "--submit")
                .map(String::as_str)
                .collect();
            if text.is_empty() {
                return bad("type needs TEXT");
            }
            Ok(call(
                "browser_type",
                json!({ "ref": reference, "text": text.join(" "), "submit": submit }),
            ))
        }
        "key" => {
            let [key] = args else {
                return bad("key needs KEY");
            };
            Ok(call("browser_press_key", json!({ "key": key })))
        }
        "tools" => {
            if !args.is_empty() {
                return bad("tools takes no arguments");
            }
            Ok(rpc("tools/list", json!({})))
        }
        "health" => {
            if !args.is_empty() {
                return bad("health takes no arguments");
            }
            Ok(Spec {
                request: None,
                health: true,
                tools_command: false,
                image_path: None,
                image_command: false,
            })
        }
        "call" => {
            let Some(name) = args.first().filter(|n| !n.is_empty()) else {
                return bad("call needs TOOL");
            };
            let raw = args[1..].join(" ");
            if args.len() > 1 && raw.is_empty() {
                return bad("call JSON-ARGS is empty");
            }
            let arguments = if raw.is_empty() {
                Map::new()
            } else {
                match serde_json::from_str::<Value>(&raw) {
                    Ok(Value::Object(m)) => m,
                    _ => return bad("call JSON-ARGS must be a JSON object"),
                }
            };
            let image_path = (name == "browser_take_screenshot")
                .then(|| {
                    arguments
                        .get("filename")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .flatten();
            let mut spec = call(name, Value::Object(arguments));
            spec.image_path = image_path;
            Ok(spec)
        }
        "cloud-history" => {
            let usage = "cloud-history delete [--since 1h|24h|7d|TIME] [--until TIME] [--host SITE]... [--page URL]... [--device ID|this] | --all";
            if args.first().map(String::as_str) != Some("delete") {
                return bad(usage);
            }
            let mut arguments = Map::new();
            let (mut hosts, mut pages): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
            let mut rest = args[1..].iter();
            while let Some(flag) = rest.next() {
                if flag == "--all" {
                    arguments.insert("all".into(), json!(true));
                    continue;
                }
                if !["--since", "--until", "--host", "--page", "--device"].contains(&flag.as_str())
                {
                    return bad(format!("unknown option {flag} — {usage}"));
                }
                let Some(value) = rest.as_slice().first().filter(|v| !v.starts_with("--")) else {
                    return bad(format!("{flag} needs a value"));
                };
                rest.next();
                match flag.as_str() {
                    "--host" => hosts.push(value),
                    "--page" => pages.push(value),
                    other => {
                        arguments.insert(other[2..].to_string(), json!(value));
                    }
                }
            }
            if !hosts.is_empty() {
                arguments.insert("host".into(), json!(hosts));
            }
            if !pages.is_empty() {
                arguments.insert("page".into(), json!(pages));
            }
            if arguments.is_empty() {
                return bad(
                    "say what to delete (--since, --until, --host, --page, --device), or --all for everything",
                );
            }
            Ok(call("cloud_history_delete", Value::Object(arguments)))
        }
        // Answered without a window, before this parser (crate::cloud_cli).
        "cloud" => bad("cloud runs on its own: copper cloud pair|sso|status (copper cloud help)"),
        // Answered without a window, before this parser (copper_botpass::cli).
        "botpass" => {
            bad("botpass runs on its own: copper botpass pair|status|unpair (copper botpass help)")
        }
        c if ELSEWHERE.contains(&c) => bad(format!("{c} is not available in this build yet")),
        other => bad(format!("unknown command: {other}")),
    }
}

/// The whole command line (`CLI.run`'s front half). `launch_env`: whether
/// `COPPER_LAUNCH=1` is set.
pub fn parse(raw: &[String], launch_env: bool) -> Result<Parsed, Early> {
    let mut args: Vec<String> = raw.to_vec();
    let json = args.iter().any(|a| a == "--json");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let launch = launch_env || args.iter().any(|a| a == "--launch");
    args.retain(|a| a != "--json" && a != "--dry-run" && a != "--launch");
    let mut tab: Option<String> = None;
    if let Some(at) = args.iter().position(|a| a == "--tab") {
        match args.get(at + 1).filter(|v| !v.is_empty()) {
            Some(v) => tab = Some(v.clone()),
            None => return bad("--tab needs a tab id (from copper tabs) or index"),
        }
        args.drain(at..=at + 1);
    }
    let Some(command) = args.first().cloned() else {
        return Err(Early::Help);
    };
    if ["help", "-h", "--help"].contains(&command.as_str()) {
        return Err(Early::Help);
    }
    let mut spec = make_request(&command, &args[1..])?;
    if let Some(tab) = tab
        && let Some(arguments) = spec
            .request
            .as_mut()
            .and_then(|r| r.get_mut("params"))
            .and_then(|p| p.get_mut("arguments"))
            .and_then(Value::as_object_mut)
        && !arguments.contains_key("tab")
    {
        arguments.insert("tab".into(), json!(tab));
    }
    Ok(Parsed {
        json,
        dry_run,
        launch,
        spec,
    })
}

/// Where the CLI reads its endpoint and how it may start Copper.
pub struct Env<'a> {
    pub endpoint: &'a dyn Fn() -> Option<Endpoint>,
    pub launch: &'a dyn Fn() -> io::Result<()>,
    pub launch_env: bool,
    pub out: &'a mut dyn Write,
    pub err: &'a mut dyn Write,
}

/// `copper …` on the real environment. Returns the exit code. A launch
/// (with `--launch`) runs this executable again with no arguments.
pub fn run(args: &[String]) -> i32 {
    let store = copper_core::Store::from_env();
    let endpoint = || {
        let mut e = config::endpoint(&store)?;
        if let Some(port) = std::env::var("COPPER_AGENT_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
        {
            e.port = port;
        }
        Some(e)
    };
    let launch = || -> io::Result<()> {
        let mut command = std::process::Command::new(browser_exe()?);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{
                CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
            };
            // No console of its own and out of the terminal's process group,
            // so closing the terminal or Ctrl+C there leaves the browser up.
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }
        command.spawn().map(|_| ())
    };
    let stdout = io::stdout();
    let stderr = io::stderr();
    let mut out = stdout.lock();
    let mut err = stderr.lock();
    let mut env = Env {
        endpoint: &endpoint,
        launch: &launch,
        launch_env: std::env::var("COPPER_LAUNCH").as_deref() == Ok("1"),
        out: &mut out,
        err: &mut err,
    };
    run_with(args, &mut env)
}

/// The browser a launch starts. Linux: this executable (the `copper` binary
/// is the browser). Windows: this executable too when it is `copper.exe`
/// (the CLI runs inside the browser binary); from any other host binary the
/// per-user install, `%LOCALAPPDATA%\Programs\Copper\copper.exe`, when it
/// exists.
fn browser_exe() -> io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    #[cfg(windows)]
    {
        let is_copper = exe
            .file_stem()
            .is_some_and(|s| s.eq_ignore_ascii_case("copper"));
        if !is_copper && let Some(installed) = installed_copper_exe().filter(|p| p.is_file()) {
            return Ok(installed);
        }
    }
    Ok(exe)
}

/// `%LOCALAPPDATA%\Programs\Copper\copper.exe`, the per-user install.
#[cfg(windows)]
fn installed_copper_exe() -> Option<std::path::PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())?;
    Some(local.join("Programs").join("Copper").join("copper.exe"))
}

fn error(env: &mut Env<'_>, text: &str) {
    let _ = writeln!(env.err, "copper: {text}");
}

fn print_json(env: &mut Env<'_>, value: &Value) {
    let _ = writeln!(env.out, "{}", pretty_json(value).replace("\\/", "/"));
}

fn health(port: u16) -> Option<(Value, bool)> {
    let r = client::request(port, "GET", "/health", &[], &[], Duration::from_secs(1)).ok()?;
    if r.status != 200 {
        return None;
    }
    let object: Value = serde_json::from_slice(&r.body).ok()?;
    let running = object["running"].as_bool().unwrap_or(false);
    Some((object, running))
}

/// POST one request; `None` after saying why on stderr.
fn post(env: &mut Env<'_>, request: &Value, conf: &Endpoint, timeout: Duration) -> Option<Value> {
    let headers = [
        ("Content-Type", "application/json".to_string()),
        ("Authorization", format!("Bearer {}", conf.token)),
        ("X-Copper-Agent", who::environment::header("copper-cli", "")),
    ];
    let body = serde_json::to_vec(request).unwrap_or_default();
    let response = client::request(conf.port, "POST", PATH, &headers, &body, timeout).ok();
    let object = response
        .filter(|r| (200..300).contains(&r.status))
        .and_then(|r| serde_json::from_slice::<Value>(&r.body).ok())
        .filter(Value::is_object);
    let Some(object) = object else {
        error(env, "could not reach Copper");
        return None;
    };
    if let Some(message) = object["error"]["message"].as_str() {
        error(env, message);
        return None;
    }
    if !object["result"].is_object() {
        error(env, "Copper returned an invalid JSON-RPC response");
        return None;
    }
    Some(object)
}

fn ensure_running(env: &mut Env<'_>, conf: &mut Endpoint, launch: bool) -> bool {
    if health(conf.port).is_some_and(|(_, r)| r) {
        return true;
    }
    if !launch {
        error(env, NOT_RUNNING);
        return false;
    }
    let _ = (env.launch)();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if health(conf.port).is_some_and(|(_, r)| r) {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    if let Some(fresh) = (env.endpoint)() {
        *conf = fresh;
    }
    if !conf.enabled || conf.token.is_empty() || !health(conf.port).is_some_and(|(_, r)| r) {
        error(env, NOT_RUNNING);
        return false;
    }
    true
}

/// `copper …` against any endpoint and streams.
pub fn run_with(args: &[String], env: &mut Env<'_>) -> i32 {
    let parsed = match parse(args, env.launch_env) {
        Ok(p) => p,
        Err(Early::Help) => {
            let _ = writeln!(env.out, "{USAGE}");
            return 0;
        }
        Err(Early::Usage(text)) => {
            error(env, &text);
            return 2;
        }
    };
    let spec = parsed.spec;
    if parsed.dry_run {
        let running = (env.endpoint)().is_some_and(|c| health(c.port).is_some_and(|(_, r)| r));
        if running {
            match &spec.request {
                Some(request) => print_json(env, request),
                None => {
                    let _ = writeln!(env.out, "GET /health");
                }
            }
        } else if parsed.launch {
            let _ = writeln!(env.out, "would launch Copper");
        } else {
            let _ = writeln!(env.out, "would not launch Copper (pass --launch)");
        }
        return 0;
    }
    let Some(mut conf) = (env.endpoint)().filter(|c| c.enabled && !c.token.is_empty()) else {
        error(env, NOT_RUNNING);
        return 2;
    };
    if !ensure_running(env, &mut conf, parsed.launch) {
        return 2;
    }
    if spec.health {
        return do_health(env, &conf, parsed.json);
    }
    let Some(request) = &spec.request else {
        error(env, "could not construct request");
        return 2;
    };
    let name = request["params"]["name"].as_str().unwrap_or("");
    let timeout = Duration::from_secs(match name {
        "jev_run" => 240,
        "cloud_history_delete" => 600,
        // A big canvas may take up to a minute to come up before the call runs.
        n if crate::catalogue::is_canvas(n) => 120,
        _ => 60,
    });
    let Some(response) = post(env, request, &conf, timeout) else {
        return 2;
    };
    render(env, response, &spec, parsed.json)
}

fn do_health(env: &mut Env<'_>, conf: &Endpoint, json_out: bool) -> i32 {
    let Some((object, running)) = health(conf.port) else {
        error(env, NOT_RUNNING);
        return 2;
    };
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": { "name": "copper-cli", "version": copper_core::version::app() } },
    });
    if post(env, &initialize, conf, Duration::from_secs(10)).is_none() {
        return 2;
    }
    let list = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} });
    let Some(response) = post(env, &list, conf, Duration::from_secs(10)) else {
        return 2;
    };
    let Some(tools) = response["result"]["tools"].as_array() else {
        return 2;
    };
    let jev = tools.iter().any(|t| t["name"] == "jev_run");
    let headless = object["headless"].as_bool().unwrap_or(false);
    if json_out {
        print_json(
            env,
            &json!({ "health": object, "jev": jev, "headless": headless }),
        );
    } else {
        let version = object["version"]
            .as_str()
            .map(|v| format!(" · version: {v}"))
            .unwrap_or_default();
        let _ = writeln!(
            env.out,
            "Copper health: running: {running} (port {}){version}",
            conf.port
        );
        let _ = writeln!(env.out, "Jev mode: {}", if jev { "on" } else { "off" });
        let _ = writeln!(env.out, "Headless: {}", if headless { "yes" } else { "no" });
    }
    0
}

fn image_path(spec: &Spec, mime: Option<&str>, index: usize) -> String {
    if let Some(preferred) = &spec.image_path {
        if index == 0 {
            return preferred.clone();
        }
        let path = std::path::Path::new(preferred);
        let stem = path.with_extension("");
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        return format!("{}-{index}.{ext}", stem.display());
    }
    let suffix = if mime == Some("image/jpeg") {
        "jpg"
    } else {
        "png"
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let n = if index == 0 {
        String::new()
    } else {
        format!("-{index}")
    };
    temp_image(&format!("copper-{now}{n}.{suffix}"))
}

/// A default screenshot path in the temp folder: `/tmp/<name>` on Linux (as
/// always, whatever `TMPDIR` says), `std::env::temp_dir()` elsewhere
/// (`%TEMP%` on Windows).
fn temp_image(name: &str) -> String {
    #[cfg(not(windows))]
    {
        format!("/tmp/{name}")
    }
    #[cfg(windows)]
    {
        std::env::temp_dir()
            .join(name)
            .to_string_lossy()
            .into_owned()
    }
}

fn expand(path: &str) -> std::path::PathBuf {
    copper_core::platform::expand_tilde(path)
}

fn render(env: &mut Env<'_>, response: Value, spec: &Spec, json_out: bool) -> i32 {
    let Some(mut result) = response.get("result").cloned().filter(Value::is_object) else {
        error(env, "Copper returned no result");
        return 2;
    };
    if spec.tools_command && !json_out {
        let Some(tools) = result["tools"].as_array() else {
            error(env, "Copper returned no tool catalogue");
            return 2;
        };
        for tool in tools {
            let Some(name) = tool["name"].as_str() else {
                continue;
            };
            let description = tool["description"].as_str().unwrap_or("");
            let first = description.split('.').next().unwrap_or(description);
            let _ = writeln!(env.out, "{name} — {first}");
        }
        return 0;
    }
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let mut paths: Vec<String> = Vec::new();
    let mut text: Vec<String> = Vec::new();
    if let Some(content) = result["content"].as_array().cloned() {
        let mut transformed = Vec::new();
        for (index, item) in content.into_iter().enumerate() {
            match item["type"].as_str() {
                Some("text") if item["text"].is_string() => {
                    text.push(item["text"].as_str().unwrap_or("").to_string());
                    transformed.push(item);
                }
                Some("image") => {
                    let decoded = item["data"]
                        .as_str()
                        .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok());
                    let Some(data) = decoded else {
                        transformed.push(item);
                        continue;
                    };
                    let path = image_path(spec, item["mimeType"].as_str(), index);
                    let file = expand(&path);
                    let written = file
                        .parent()
                        .map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|_| std::fs::write(&file, &data));
                    if written.is_err() {
                        error(env, &format!("could not write screenshot to {path}"));
                        return 2;
                    }
                    transformed.push(json!({ "type": "image", "path": path }));
                    paths.push(path);
                }
                _ => transformed.push(item),
            }
        }
        result["content"] = Value::Array(transformed);
    }
    if json_out {
        print_json(env, &result);
    } else if is_error {
        for line in &text {
            error(env, line);
        }
    } else if !paths.is_empty() {
        for path in &paths {
            let _ = writeln!(env.out, "{path}");
        }
        if !spec.image_command {
            for line in &text {
                let _ = write!(env.out, "{line}");
            }
        }
    } else {
        for line in &text {
            let _ = write!(env.out, "{line}");
        }
    }
    if is_error { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn args_of(p: &Parsed) -> Value {
        p.spec.request.as_ref().unwrap()["params"]["arguments"].clone()
    }

    #[test]
    fn verbs_map_to_tools() {
        let p = parse(&s(&["--json", "open", "example.com", "--focus"]), false).unwrap();
        assert!(p.json);
        assert_eq!(
            args_of(&p),
            json!({"action": "new", "url": "example.com", "focus": true})
        );
        let p = parse(
            &s(&[
                "run",
                "book",
                "a",
                "flight",
                "--url",
                "x.com",
                "--max",
                "5",
                "--new-tab",
            ]),
            false,
        )
        .unwrap();
        assert_eq!(
            args_of(&p),
            json!({"goal": "book a flight", "url": "x.com", "maxSteps": 5, "newTab": true})
        );
        let p = parse(
            &s(&["type", "e3", "hello", "world", "--submit", "--tab", "1a2b"]),
            false,
        )
        .unwrap();
        assert_eq!(
            args_of(&p),
            json!({"ref": "e3", "text": "hello world", "submit": true, "tab": "1a2b"})
        );
        let p = parse(&s(&["tools"]), false).unwrap();
        assert!(p.spec.tools_command);
        let p = parse(&s(&["health"]), true).unwrap();
        assert!(p.spec.health && p.launch);
        let p = parse(&s(&["shot", "/tmp/x.jpg", "--jpeg"]), false).unwrap();
        assert_eq!(p.spec.image_path.as_deref(), Some("/tmp/x.jpg"));
        assert_eq!(args_of(&p)["type"], "jpeg");
        let p = parse(&s(&["call", "browser_tabs", r#"{"action":"list"}"#]), false).unwrap();
        assert_eq!(args_of(&p), json!({"action": "list"}));
        for verb in [
            "tabs", "use", "go", "signin", "autofill", "observe", "step", "extract", "snapshot",
            "text", "find", "eval", "click", "key",
        ] {
            let extra: Vec<&str> = match verb {
                "go" | "find" | "eval" | "click" | "key" | "step" | "extract" => vec![verb, "x"],
                "autofill" => vec![verb, "card"],
                _ => vec![verb],
            };
            assert!(parse(&s(&extra), false).is_ok(), "{verb}");
        }
    }

    #[test]
    fn usage_errors() {
        assert_eq!(parse(&s(&[]), false), Err(Early::Help));
        assert_eq!(
            parse(&s(&["open"]), false),
            Err(Early::Usage("open needs URL".into()))
        );
        assert_eq!(
            parse(&s(&["--tab"]), false),
            Err(Early::Usage(
                "--tab needs a tab id (from copper tabs) or index".into()
            ))
        );
        assert_eq!(
            parse(&s(&["call", "x", "[1]"]), false),
            Err(Early::Usage("call JSON-ARGS must be a JSON object".into()))
        );
        assert!(
            matches!(parse(&s(&["frobnicate"]), false), Err(Early::Usage(t)) if t == "unknown command: frobnicate")
        );
        // `cloud` never reaches the running Copper: it pairs with no window.
        assert!(
            matches!(parse(&s(&["--cli", "cloud", "status"])[1..], false), Err(Early::Usage(t)) if t.contains("copper cloud pair|sso|status"))
        );
        assert!(USAGE.contains("cloud pair CODE [--sync none|all|LIST] [--name NAME]"));
        assert!(USAGE.contains("cloud status"));
    }

    #[test]
    fn default_shot_goes_to_the_temp_folder() {
        let p = temp_image("copper-1.png");
        #[cfg(not(windows))]
        assert_eq!(p, "/tmp/copper-1.png");
        #[cfg(windows)]
        assert_eq!(
            std::path::PathBuf::from(&p),
            std::env::temp_dir().join("copper-1.png")
        );
    }

    #[test]
    fn launch_runs_this_executable_when_it_is_copper() {
        // The test binary is not `copper`: Linux still runs this executable;
        // Windows falls back to it when no per-user install exists.
        let exe = browser_exe().unwrap();
        #[cfg(not(windows))]
        assert_eq!(exe, std::env::current_exe().unwrap());
        #[cfg(windows)]
        assert!(
            exe == std::env::current_exe().unwrap()
                || installed_copper_exe().is_some_and(|i| i == exe)
        );
    }
}
