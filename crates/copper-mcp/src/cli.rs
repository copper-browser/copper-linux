//! `copper …` — a small, synchronous shell client for the MCP server in the
//! Copper already running (Fork/MCP/CLI.swift). The verbs map one-to-one
//! onto the server's tools, with the same names and arguments, so an agent
//! moves between the command line and MCP without learning a second API.
//!
//! Exit codes: 0 done, 1 the tool answered with an error, 2 usage or Copper
//! unreachable.

use std::io::{self, Read, Write};
use std::time::Duration;

use base64::Engine as _;
use copper_pagejs::render::pretty_json;
use serde_json::{Map, Value, json};

use crate::client;
use crate::config::{self, Endpoint, PATH};
use crate::screenshot;
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
    /// A verb's own help (`copper intelligence --help`).
    HelpFor(&'static str),
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
             [--model jev-1.14.0] [--strategy fast,literal,burst,lean,paste,watch|turbo|llm=haiku] [--json-run]
  step \"GOAL…\"                              supervise one Jev decision
  extract \"INSTRUCTION…\" [--full] [--schema JSON]
  snapshot [--interactive] [-n N] [--selector CSS]
  text                                      read page text
  find TEXT                                 find matching page elements
  shot [PATH] [--full] [--jpeg] [--ref REF | --selector CSS [--frame F] | --region X,Y,W,H]
       [--padding N] [--scale N | --hd]
                                            save a screenshot (default /tmp/copper-<time>.png): the viewport,
                                            the whole page (--full), one element or a region in CSS px (the
                                            page's with --full), with --padding px round it. --scale 1-4
                                            draws it again at that many pixels per CSS px (--hd = 2);
                                            a folder PATH gets copper-YYYYMMDD-HHMMSS.png in it
  eval 'JS'                                 evaluate JavaScript
  click REF | --selector CSS                click an element
  type REF TEXT [--submit]                  type into an element
  key KEY                                   press a key
  hover REF | --selector CSS                hover over an element
  select REF VALUE...                       choose options in a <select>
  scroll [PIXELS] [--ref REF]               scroll the page (or within REF); --ref alone brings it into view
  wait TEXT | --gone TEXT | --time SECONDS  wait for text to appear or go, or for a time
  back | forward                            go back or forward in your tab
  upload PATH... | --cancel                 answer the file chooser your click opened (or arm the next one)
  dialog accept [TEXT] | dismiss            answer the page's alert, confirm or prompt
  pdf [PATH]                                save the page as a PDF (default: the downloads folder)
  downloads [--wait SECONDS]                downloads your tab started, and where the files are
  tools                                     list available tools
  health                                    check the agent server and Jev mode
  intelligence [status|sources|refresh|set|reload]  model access, keys, their source and readiness; never prints a key
                                            (copper intelligence --help)
  bitwarden save -|note -                   keep a login or a secure note in the open Bitwarden vault;
                                            ONE JSON object on stdin, never a value back
                                            (copper bitwarden --help)
  claude [status|signin|paste -|signout|cancel]
                                            Claude account sign-in and status; never prints a token
                                            (copper claude --help)
  pins [status] | pins sleep on|off         every pin's state, and whether big pins may sleep
                                            (Settings › Tabs › Let big pins sleep)
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

pub const INTELLIGENCE_USAGE: &str = "Usage: copper intelligence <command>

The models Copper asks: Jev (TypeSafe) and the router (a LiteLLM gateway).
Output is JSON and never contains a key.

  status                     {jevReady, routerReady, routerURL, routerModel, jevModel,
                             lane, laneSource, localLane, tier, model,
                             modelReady, claudeReady, claudeAccount, sources, cloud} (default);
                             lane is the one in force: key (laneSource cloud) while
                             Copper Cloud provides a gateway key, else localLane
  sources                    where each key and address comes from: local | cloud | none
                             (keys) or local | cloud | default (addresses, jev model),
                             and the lane: local | cloud;
                             Copper Cloud provides keys through GET /v1/intelligence
  refresh                    ask the linked Copper Cloud for its keys again now
  set [--lane key|claude] [--model haiku|sonnet|opus]
      [--haiku-model M] [--sonnet-model M] [--opus-model M]
      [--jev KEY] [--jev-model M]
      [--router-key KEY] [--router-url URL] [--router-model M] [--text-model M]
                             write into intelligence.json (0600) through the running app;
                             a value of - is read from stdin, keeping keys out of `ps`
  reload                     re-read intelligence.json (Copper reads it on every use)

Exit 0 on success, 1 when the app refuses a value, 2 on usage or when Copper is unreachable.
";

pub const CLAUDE_USAGE: &str = "Usage: copper claude <command>

Sign in with a Claude account instead of an API key. `signin` opens claude.ai
in the running Copper window; finish the OAuth sign-in there and the callback
returns to Copper. If the tab cannot return, copy the callback code and use
`paste -` (stdin is preferred; a literal code is also accepted).

  status                     account status, email and expiry (default)
  signin                     open claude.ai and start OAuth with PKCE
  paste -|CODE               finish sign-in from stdin or a literal callback code
  signout                    forget claude.json and the account
  cancel                     cancel a waiting sign-in

Tokens live in claude.json beside intelligence.json, private to you. Output
never contains a token. Use a Claude Pro, Max, Team or Enterprise account.

Exit 0 on success, 1 when the app refuses an operation, 2 on usage or when
Copper is unreachable.
";

pub const BITWARDEN_USAGE: &str = "Usage: copper bitwarden <command>

The Bitwarden vault in the running Copper (Settings › Passwords › Bitwarden).
Output is always JSON — never a password, key or session.

  save -                     keep a login; reads ONE JSON object from stdin:
                             {url (or host), username, password, totp? (otpauth URI
                              or base32 secret), notes?, name? (default: the host),
                              folder?}
                             The same site + username already saved: its password
                             (and totp/notes when given) change instead.
  note -                     keep a secure note (backup codes); stdin:
                             {name, text, folder?} — the note with exactly that name
                             gets the new text.
                             save and note answer {ok, id, created, updated, name,
                             folder}, never a value. Unlocked only; with agent access
                             by folder new items go to the Agents folder, and only
                             items shared with agents can change.

status, login -, cancel, lock, logout, sync and policy are not available in this
build yet: sign in and unlock in Settings › Passwords.

Exit 0 on ok, 1 when the result says {ok: false}, 2 on usage or when Copper is unreachable.
";

/// The values `copper bitwarden` never shows, even with --dry-run.
const BITWARDEN_SECRETS: [&str; 7] = [
    "password",
    "clientId",
    "clientSecret",
    "otp",
    "totp",
    "notes",
    "text",
];

/// `copper bitwarden save -|note -` (`CLI.runBitwarden`): the loopback-only
/// `copper/bitwarden` method. Values come only on stdin, never as arguments.
fn bitwarden(args: &[String], stdin: &mut dyn Read) -> Result<Spec, Early> {
    let (op, rest) = match args.split_first() {
        Some((op, rest)) => (op.as_str(), rest),
        None => ("status", args),
    };
    if ["help", "-h", "--help"].contains(&op) || rest.iter().any(|a| a == "-h" || a == "--help") {
        return Err(Early::HelpFor(BITWARDEN_USAGE));
    }
    let fields: &[&str] = match op {
        "save" => &[
            "name", "url", "host", "username", "password", "totp", "notes", "folder",
        ],
        "note" => &["name", "text", "folder"],
        "status" | "login" | "cancel" | "lock" | "logout" | "sync" | "policy" => {
            return bad(format!("bitwarden {op} is not available in this build yet"));
        }
        _ => {
            return bad(format!(
                "unknown bitwarden command: {op} (see copper bitwarden --help)"
            ));
        }
    };
    if rest != ["-"] {
        return bad(format!(
            "bitwarden {op} reads its JSON from stdin: `copper bitwarden {op} -` (secrets are never accepted as arguments)"
        ));
    }
    let limit = 64 * 1024;
    let mut data = Vec::new();
    let read = stdin.take(limit + 1).read_to_end(&mut data);
    let object = match serde_json::from_slice::<Value>(&data) {
        Ok(Value::Object(o)) if read.is_ok() && data.len() as u64 <= limit => o,
        _ => return bad(format!("bitwarden {op} - : stdin must be one JSON object")),
    };
    let mut params = Map::new();
    params.insert("op".into(), json!(op));
    for field in fields {
        if let Some(v) = object.get(*field).filter(|v| !v.is_null()) {
            params.insert((*field).into(), v.clone());
        }
    }
    Ok(rpc("copper/bitwarden", Value::Object(params)))
}

/// What `--dry-run` prints of a request: every secret a control carries as `…`.
fn shown(request: &Value) -> Value {
    let mut shown = request.clone();
    if shown["method"] == "copper/bitwarden" {
        for secret in BITWARDEN_SECRETS {
            if let Some(v) = shown["params"].get_mut(secret) {
                *v = json!("…");
            }
        }
    }
    for secret in ["jevKey", "routerKey", "code"] {
        if let Some(v) = shown["params"].get_mut(secret) {
            *v = json!("…");
        }
    }
    shown
}

/// `copper claude …` (`CLI.runClaude`): the loopback-only `copper/claude`
/// method.
fn claude(args: &[String]) -> Result<Spec, Early> {
    let (op, rest) = match args.split_first() {
        Some((op, rest)) => (op.as_str(), rest),
        None => ("status", args),
    };
    if ["help", "-h", "--help"].contains(&op) || rest.iter().any(|a| a == "-h" || a == "--help") {
        return Err(Early::HelpFor(CLAUDE_USAGE));
    }
    let mut params = Map::new();
    params.insert("op".into(), json!(op));
    match op {
        "status" | "signin" | "signout" | "cancel" => {
            if !rest.is_empty() {
                return bad(format!("claude {op} takes no arguments"));
            }
        }
        "paste" => {
            let [code] = rest else {
                return bad("claude paste needs CODE or - (stdin is preferred)");
            };
            let mut code = code.clone();
            if code == "-" {
                let mut all = String::new();
                let _ = io::stdin().read_to_string(&mut all);
                code = all.trim().to_string();
                if code.is_empty() {
                    return bad("claude paste -: nothing on stdin");
                }
            }
            params.insert("code".into(), json!(code));
        }
        _ => {
            return bad(format!(
                "unknown claude command: {op} (see copper claude --help)"
            ));
        }
    }
    Ok(rpc("copper/claude", Value::Object(params)))
}

/// `copper intelligence …` (`CLI.runIntelligence`): the loopback-only
/// `copper/intelligence` method.
fn intelligence(args: &[String]) -> Result<Spec, Early> {
    let (op, rest) = match args.split_first() {
        Some((op, rest)) => (op.as_str(), rest),
        None => ("status", args),
    };
    if ["help", "-h", "--help"].contains(&op) || rest.iter().any(|a| a == "-h" || a == "--help") {
        return Err(Early::HelpFor(INTELLIGENCE_USAGE));
    }
    let mut params = Map::new();
    params.insert("op".into(), json!(op));
    match op {
        "status" | "reload" | "sources" | "refresh" => {
            if !rest.is_empty() {
                return bad(format!("intelligence {op} takes no arguments"));
            }
        }
        "set" => {
            let mut flags = rest.iter();
            while let Some(flag) = flags.next() {
                let name = match flag.as_str() {
                    "--jev" => "jevKey",
                    "--router-key" => "routerKey",
                    "--router-url" => "routerURL",
                    "--router-model" => "routerModel",
                    "--text-model" => "textModel",
                    "--jev-model" => "jevModel",
                    "--lane" => "lane",
                    "--model" => "tier",
                    "--haiku-model" => "haikuModel",
                    "--sonnet-model" => "sonnetModel",
                    "--opus-model" => "opusModel",
                    other => return bad(format!("unknown intelligence set option: {other}")),
                };
                let Some(mut value) = flags.next().cloned() else {
                    return bad(format!("{flag} needs a value"));
                };
                // `-` reads the value from stdin, so a key need not sit in
                // the process list.
                if value == "-" {
                    let mut line = String::new();
                    if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
                        return bad(format!("{flag} - : nothing on stdin"));
                    }
                    value = line.trim_end_matches(['\r', '\n']).to_string();
                }
                params.insert(name.into(), json!(value));
            }
            if params.len() == 1 {
                return bad(
                    "intelligence set needs at least one of --lane, --model, --haiku-model, --sonnet-model, --opus-model, --jev, --jev-model, --router-key, --router-url, --router-model, --text-model",
                );
            }
        }
        _ => {
            return bad(format!(
                "unknown intelligence command: {op} (see copper intelligence --help)"
            ));
        }
    }
    Ok(rpc("copper/intelligence", Value::Object(params)))
}

/// `copper pins [status]` / `copper pins sleep on|off` (`CLI.runPins`):
/// Settings › Tabs › Let big pins sleep, and every pin's state, through the
/// loopback-only `copper/pins` method.
fn pins(args: &[String]) -> Result<Spec, Early> {
    let on = match args {
        [] => None,
        [status] if status == "status" => None,
        [sleep, on] if sleep == "sleep" && (on == "on" || on == "off") => Some(on == "on"),
        _ => return bad("usage: copper pins [status] | copper pins sleep on|off"),
    };
    let mut params = Map::new();
    if let Some(on) = on {
        params.insert("on".into(), json!(on));
    }
    Ok(rpc("copper/pins", Value::Object(params)))
}

/// Commands the Mac has that this build does not (yet).
pub(crate) const ELSEWHERE: [&str; 6] = [
    "setup",
    "session",
    "link",
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
                    "--model" => {
                        i += 1;
                        match args.get(i).filter(|v| !v.is_empty()) {
                            Some(v) => arguments.insert("model".into(), json!(v)),
                            None => {
                                return bad(
                                    "run --model needs a Jev model name (jev-latest, jev-preview, jev-1.14.0)",
                                );
                            }
                        };
                    }
                    "--json-run" => {
                        arguments.insert("json".into(), json!(true));
                    }
                    "--strategy" => {
                        i += 1;
                        match args
                            .get(i)
                            .filter(|v| crate::ultrafast::Strategy::parse(v).is_some())
                        {
                            Some(v) => arguments.insert("strategy".into(), json!(v)),
                            None => {
                                return bad(
                                    "run --strategy takes classic, fast, literal, burst, lean, paste, watch, turbo, llm or llm=<model>, comma-separated",
                                );
                            }
                        };
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
        "shot" => shot(args),
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
        // The everyday page verbs, each one tool with its MCP arguments
        // (`CLI.swift`); paths are made absolute here, where the shell's
        // working folder is known.
        "back" | "forward" => {
            if !args.is_empty() {
                return bad(format!("{command} takes no arguments"));
            }
            let tool = if command == "back" {
                "browser_navigate_back"
            } else {
                "browser_navigate_forward"
            };
            Ok(call(tool, json!({})))
        }
        "hover" => {
            if args.first().is_some_and(|a| a == "--selector") {
                if args.len() != 2 {
                    return bad("hover --selector needs CSS");
                }
                return Ok(call("browser_hover", json!({ "selector": args[1] })));
            }
            let [reference] = args else {
                return bad("hover takes one REF or --selector CSS");
            };
            Ok(call("browser_hover", json!({ "ref": reference })))
        }
        "select" => {
            if args.len() < 2 {
                return bad("select needs REF VALUE...");
            }
            Ok(call(
                "browser_select_option",
                json!({ "ref": args[0], "values": args[1..] }),
            ))
        }
        "scroll" => {
            let mut arguments = Map::new();
            let mut rest = args.iter();
            while let Some(arg) = rest.next() {
                if arg == "--ref" {
                    match rest.next().filter(|v| !v.is_empty()) {
                        Some(value) => arguments.insert("ref".into(), json!(value)),
                        None => return bad("scroll --ref needs REF"),
                    };
                } else if let Ok(dy) = arg.parse::<f64>() {
                    arguments.insert("deltaY".into(), json!(dy));
                } else {
                    return bad("scroll takes [PIXELS] [--ref REF]");
                }
            }
            Ok(call("browser_scroll", Value::Object(arguments)))
        }
        "wait" => {
            if args.first().is_some_and(|a| a == "--time") {
                let seconds = match args {
                    [_, s] => s.parse::<f64>().ok(),
                    _ => None,
                };
                let Some(seconds) = seconds else {
                    return bad("wait --time needs SECONDS");
                };
                return Ok(call("browser_wait_for", json!({ "time": seconds })));
            }
            let gone = args.first().is_some_and(|a| a == "--gone");
            let text = args[usize::from(gone)..].join(" ");
            if text.is_empty() {
                return bad("wait needs TEXT, --gone TEXT or --time SECONDS");
            }
            let key = if gone { "textGone" } else { "text" };
            Ok(call("browser_wait_for", json!({ key: text })))
        }
        "upload" => {
            if args == ["--cancel"] {
                return Ok(call("browser_file_upload", json!({})));
            }
            if args.is_empty() || args.iter().any(|a| a.starts_with('-')) {
                return bad("upload needs PATH... or --cancel");
            }
            let paths: Vec<String> = args.iter().map(|p| absolute(p)).collect();
            Ok(call("browser_file_upload", json!({ "paths": paths })))
        }
        "dialog" => {
            let answer = args.first().map(String::as_str);
            if !matches!(answer, Some("accept" | "dismiss")) {
                return bad("dialog needs accept [TEXT] or dismiss");
            }
            let mut arguments = json!({ "accept": answer == Some("accept") });
            if args.len() > 1 {
                arguments["promptText"] = json!(args[1..].join(" "));
            }
            Ok(call("browser_handle_dialog", arguments))
        }
        "pdf" => {
            if args.len() > 1 {
                return bad("pdf takes one optional PATH");
            }
            let arguments = match args.first() {
                Some(path) => json!({ "filename": absolute(path) }),
                None => json!({}),
            };
            Ok(call("browser_pdf_save", arguments))
        }
        "downloads" => match args {
            [] => Ok(call("browser_downloads", json!({}))),
            [flag, seconds] if flag == "--wait" => match seconds.parse::<f64>() {
                Ok(s) => Ok(call("browser_downloads", json!({ "wait": s }))),
                Err(_) => bad("downloads takes [--wait SECONDS]"),
            },
            _ => bad("downloads takes [--wait SECONDS]"),
        },
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
            let mut arguments = if raw.is_empty() {
                Map::new()
            } else {
                match serde_json::from_str::<Value>(&raw) {
                    Ok(Value::Object(m)) => m,
                    _ => return bad("call JSON-ARGS must be a JSON object"),
                }
            };
            // A relative `filename`, or ~/…, is made the path the shell
            // meant; Copper names a folder's file and says where it saved.
            let mut image_path = None;
            if name == "browser_take_screenshot"
                && let Some(given) = arguments
                    .get("filename")
                    .and_then(Value::as_str)
                    .filter(|f| !f.is_empty())
            {
                let path = shot_path(given);
                arguments.insert("filename".into(), json!(path));
                image_path = Some(path);
            }
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
        "intelligence" => intelligence(args),
        "claude" => claude(args),
        "pins" => pins(args),
        "bitwarden" => bitwarden(args, &mut io::stdin()),
        "cloud" => bad("cloud runs on its own: copper cloud pair|sso|status (copper cloud help)"),
        // Answered without a window, before this parser (copper_botpass::cli).
        "botpass" => {
            bad("botpass runs on its own: copper botpass pair|status|unpair (copper botpass help)")
        }
        c if ELSEWHERE.contains(&c) => bad(format!("{c} is not available in this build yet")),
        other => bad(format!("unknown command: {other}")),
    }
}

/// `copper shot`'s options, for its help line and its usage errors.
const SHOT_USAGE: &str = "[PATH] [--full] [--jpeg] [--ref REF | --selector CSS [--frame F] | --region X,Y,W,H] [--padding N] [--scale N | --hd]";

/// A screenshot's path as the shell meant it (`CLI.shotPath`): `~`
/// expanded, a relative one made absolute against the working folder, a
/// trailing separator (a folder) kept. Copper names a folder's file.
fn shot_path(path: &str) -> String {
    let expanded = screenshot::expand(path);
    let absolute = std::path::absolute(&expanded).unwrap_or(expanded);
    let mut text = absolute.to_string_lossy().into_owned();
    let folder = |p: &str| p.ends_with('/') || p.ends_with(std::path::MAIN_SEPARATOR);
    if folder(path) && !folder(&text) {
        text.push(std::path::MAIN_SEPARATOR);
    }
    text
}

/// A path as the shell meant it: `~` expanded, made absolute against the
/// working folder (`URL(fileURLWithPath:).standardizedFileURL.path`).
fn absolute(path: &str) -> String {
    let expanded = screenshot::expand(path);
    std::path::absolute(&expanded)
        .unwrap_or(expanded)
        .to_string_lossy()
        .into_owned()
}

/// Where this command writes a screenshot itself, when Copper could not:
/// `path`, or for a folder a new name in it, as Copper would have named it.
fn shot_destination(path: &str, jpeg: bool) -> String {
    screenshot::destination(path, jpeg, &screenshot::stamp())
        .to_string_lossy()
        .into_owned()
}

/// `copper shot …` (`CLI.swift` `shot`): each flag is one of
/// `browser_take_screenshot`'s arguments, checked here as Copper checks
/// them so a bad call exits 2 like any other usage error. `--hd` is
/// `--scale 2`; no PATH is a file in the temp folder.
fn shot(args: &[String]) -> Result<Spec, Early> {
    let usage = format!("usage: copper shot {SHOT_USAGE}");
    let wrong = |problem: &str| bad(format!("{problem} — {usage}"));
    let mut path: Option<&String> = None;
    let mut arguments = Map::new();
    arguments.insert("type".into(), json!("png"));
    let mut hd = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let flag = arg.as_str();
        match flag {
            "--full" => {
                arguments.insert("fullPage".into(), json!(true));
            }
            "--jpeg" => {
                arguments.insert("type".into(), json!("jpeg"));
            }
            "--hd" => hd = true,
            "--ref" | "--selector" | "--frame" => {
                let Some(value) = rest.next().filter(|v| !v.is_empty()) else {
                    return wrong(&format!("shot {flag} needs a value"));
                };
                arguments.insert(flag[2..].into(), json!(value));
            }
            "--region" => {
                let numbers: Option<Vec<f64>> = rest.next().map(|v| {
                    v.split(',')
                        .filter_map(|n| n.trim().parse::<f64>().ok().filter(|n| n.is_finite()))
                        .collect()
                });
                let Some(&[x, y, width, height]) = numbers
                    .as_deref()
                    .and_then(|n| <&[f64; 4]>::try_from(n).ok())
                else {
                    return wrong("shot --region needs X,Y,W,H: four numbers in CSS pixels");
                };
                arguments.insert(
                    "region".into(),
                    json!({ "x": x, "y": y, "width": width, "height": height }),
                );
            }
            "--padding" | "--scale" => {
                let Some(n) = rest
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|n| n.is_finite())
                else {
                    return wrong(&format!("shot {flag} needs a number"));
                };
                arguments.insert(flag[2..].into(), json!(n));
            }
            _ if flag.starts_with('-') || path.is_some() => {
                return wrong("shot takes one optional PATH and these options");
            }
            _ => path = Some(arg),
        }
    }
    if hd {
        if arguments.contains_key("scale") {
            return wrong("shot takes --scale N or --hd, not both");
        }
        arguments.insert("scale".into(), json!(2));
    }
    if arguments.contains_key("frame") && !arguments.contains_key("selector") {
        return wrong("shot --frame goes with --selector");
    }
    if let Err(problem) = screenshot::ShotArgs::parse(&arguments) {
        return wrong(&problem);
    }
    let jpeg = arguments["type"] == "jpeg";
    let output = match path {
        Some(p) => shot_path(p),
        None => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            temp_image(&format!(
                "copper-{now}.{}",
                if jpeg { "jpg" } else { "png" }
            ))
        }
    };
    arguments.insert("filename".into(), json!(output));
    let mut spec = call("browser_take_screenshot", Value::Object(arguments));
    spec.image_path = Some(output);
    spec.image_command = true;
    Ok(spec)
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
/// Seconds a `browser_handoff` call may take: its own wait (default 15, at
/// most 600) and half a minute more.
pub(crate) fn handoff_wait(args: &Value) -> u64 {
    let wait = args["timeout"]
        .as_f64()
        .filter(|s| *s > 0.0)
        .unwrap_or(15.0);
    wait.min(600.0).ceil() as u64 + 30
}

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
        Err(Early::HelpFor(text)) => {
            let _ = writeln!(env.out, "{text}");
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
                Some(request) => print_json(env, &shown(request)),
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
        "browser_handoff" => handoff_wait(&request["params"]["arguments"]),
        "jev_run" => 240,
        "cloud_history_delete" => 600,
        // A big canvas may take up to a minute to come up before the call runs.
        n if crate::catalogue::is_canvas(n) => 120,
        _ => 60,
    });
    let Some(response) = post(env, request, &conf, timeout) else {
        return 2;
    };
    if request["method"]
        .as_str()
        .is_some_and(|m| m.starts_with("copper/"))
    {
        return render_control(env, &response);
    }
    render(env, response, &spec, parsed.json)
}

/// A control's answer (`copper intelligence …`): JSON always; its `error`
/// goes to stderr with exit 1.
fn render_control(env: &mut Env<'_>, response: &Value) -> i32 {
    let Some(result) = response.get("result").filter(|r| r.is_object()) else {
        error(env, "Copper returned no result");
        return 2;
    };
    // `bitwarden save|note`: {ok:false, error} is the answer itself (exit 1).
    if result["ok"] == false {
        print_json(env, result);
        return 1;
    }
    if let Some(message) = result["error"].as_str().filter(|m| !m.is_empty()) {
        error(env, message);
        return 1;
    }
    print_json(env, result);
    0
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

/// The file Copper says it saved a screenshot asked for at `asked` to
/// (` Saved to <path>.` in its answer, `CLI.savedShot`): `asked` itself,
/// or for a folder the `copper-YYYYMMDD-HHMMSS[-N].png|jpg` it made there.
/// `None` when it saved nothing there.
fn saved_to(texts: &[&str], asked: &str) -> Option<String> {
    let folder = asked
        .strip_suffix(['/', std::path::MAIN_SEPARATOR])
        .unwrap_or(asked);
    for text in texts {
        if text.contains(&format!("Saved to {asked}.")) {
            return Some(asked.to_string());
        }
        for sep in ['/', std::path::MAIN_SEPARATOR] {
            let lead = format!("Saved to {folder}{sep}");
            let Some(at) = text.find(&lead) else {
                continue;
            };
            let rest = &text[at + lead.len()..];
            let name = rest.split(". ").next().unwrap_or(rest);
            let name = name.strip_suffix('.').unwrap_or(name);
            if generated_name(name) {
                return Some(format!("{folder}{sep}{name}"));
            }
        }
    }
    None
}

/// `copper-YYYYMMDD-HHMMSS.png`, `-N` before the extension allowed, or
/// `.jpg`: a name Copper makes in a folder.
fn generated_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_suffix(".png")
        .or_else(|| name.strip_suffix(".jpg"))
        .and_then(|n| n.strip_prefix("copper-"))
    else {
        return false;
    };
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    let mut parts = stem.split('-');
    let (Some(day), Some(time)) = (parts.next(), parts.next()) else {
        return false;
    };
    let extra = parts.next();
    digits(day, 8)
        && digits(time, 6)
        && extra.is_none_or(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && parts.next().is_none()
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
        // A picture Copper saved where it was asked is not written again.
        let said: Vec<&str> = content.iter().filter_map(|c| c["text"].as_str()).collect();
        let saved = spec
            .image_path
            .as_deref()
            .and_then(|asked| saved_to(&said, asked));
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
                    if index == 0
                        && let Some(path) = &saved
                    {
                        transformed.push(json!({ "type": "image", "path": path }));
                        paths.push(path.clone());
                        continue;
                    }
                    let mut path = image_path(spec, item["mimeType"].as_str(), index);
                    if index == 0 && spec.image_path.is_some() {
                        // Where Copper would have put it: a folder's new file.
                        let jpeg = item["mimeType"] == "image/jpeg";
                        path = shot_destination(&path, jpeg);
                    }
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
            &s(&[
                "run",
                "sign in",
                "--strategy",
                "fast,burst",
                "--model",
                "jev-1.14.0",
                "--json-run",
            ]),
            false,
        )
        .unwrap();
        assert_eq!(
            args_of(&p),
            json!({"goal": "sign in", "strategy": "fast,burst", "model": "jev-1.14.0", "json": true})
        );
        assert!(parse(&s(&["run", "x", "--strategy", "warp"]), false).is_err());
        assert!(parse(&s(&["run", "x", "--strategy"]), false).is_err());
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
    fn page_verbs_map_onto_their_tools() {
        let one = |v: &[&str]| {
            let p = parse(&s(v), false).unwrap();
            (
                p.spec.request.as_ref().unwrap()["params"]["name"].clone(),
                args_of(&p),
            )
        };
        let usage = |v: &[&str]| match parse(&s(v), false) {
            Err(Early::Usage(t)) => t,
            _ => panic!("{v:?} parsed"),
        };
        assert_eq!(one(&["back"]), (json!("browser_navigate_back"), json!({})));
        assert_eq!(one(&["forward"]).0, json!("browser_navigate_forward"));
        assert_eq!(usage(&["back", "x"]), "back takes no arguments");
        assert_eq!(one(&["hover", "e3"]).1, json!({"ref": "e3"}));
        assert_eq!(
            one(&["hover", "--selector", "a"]).1,
            json!({"selector": "a"})
        );
        assert_eq!(usage(&["hover"]), "hover takes one REF or --selector CSS");
        assert_eq!(
            one(&["select", "e4", "Red", "Blue"]),
            (
                json!("browser_select_option"),
                json!({"ref": "e4", "values": ["Red", "Blue"]})
            )
        );
        assert_eq!(usage(&["select", "e4"]), "select needs REF VALUE...");
        assert_eq!(
            one(&["scroll", "600", "--ref", "e9"]).1,
            json!({"deltaY": 600.0, "ref": "e9"})
        );
        assert_eq!(
            usage(&["scroll", "down"]),
            "scroll takes [PIXELS] [--ref REF]"
        );
        assert_eq!(
            one(&["wait", "Order", "placed"]).1,
            json!({"text": "Order placed"})
        );
        assert_eq!(
            one(&["wait", "--gone", "Loading"]).1,
            json!({"textGone": "Loading"})
        );
        assert_eq!(one(&["wait", "--time", "1.5"]).1, json!({"time": 1.5}));
        assert_eq!(usage(&["wait", "--time"]), "wait --time needs SECONDS");
        assert_eq!(
            usage(&["wait"]),
            "wait needs TEXT, --gone TEXT or --time SECONDS"
        );
        assert_eq!(
            one(&["upload", "--cancel"]),
            (json!("browser_file_upload"), json!({}))
        );
        let (_, a) = one(&["upload", "a.txt"]);
        let path = a["paths"][0].as_str().unwrap();
        assert!(
            std::path::Path::new(path).is_absolute() && path.ends_with("a.txt"),
            "{path}"
        );
        assert_eq!(usage(&["upload", "-x"]), "upload needs PATH... or --cancel");
        assert_eq!(
            one(&["dialog", "accept", "my", "name"]),
            (
                json!("browser_handle_dialog"),
                json!({"accept": true, "promptText": "my name"})
            )
        );
        assert_eq!(one(&["dialog", "dismiss"]).1, json!({"accept": false}));
        assert_eq!(
            usage(&["dialog", "ok"]),
            "dialog needs accept [TEXT] or dismiss"
        );
        assert_eq!(one(&["pdf"]), (json!("browser_pdf_save"), json!({})));
        assert!(
            one(&["pdf", "out.pdf"]).1["filename"]
                .as_str()
                .unwrap()
                .ends_with("out.pdf")
        );
        assert_eq!(usage(&["pdf", "a", "b"]), "pdf takes one optional PATH");
    }

    #[test]
    fn shot_flags_map_onto_the_tool() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("card.png");
        let f = file.to_str().unwrap();
        let p = parse(
            &s(&[
                "shot",
                f,
                "--selector",
                ".pricing-card",
                "--padding",
                "12",
                "--hd",
            ]),
            false,
        )
        .unwrap();
        assert!(p.spec.image_command);
        assert_eq!(p.spec.image_path.as_deref(), Some(f));
        assert_eq!(
            args_of(&p),
            json!({"type": "png", "selector": ".pricing-card", "padding": 12.0, "scale": 2, "filename": f})
        );
        let p = parse(
            &s(&[
                "--tab",
                "1a2b",
                "shot",
                f,
                "--region",
                "100,-5.5,200,100",
                "--scale",
                "1.5",
                "--full",
                "--jpeg",
            ]),
            false,
        )
        .unwrap();
        assert_eq!(
            args_of(&p),
            json!({"type": "jpeg", "fullPage": true, "scale": 1.5, "filename": f, "tab": "1a2b",
                   "region": {"x": 100.0, "y": -5.5, "width": 200.0, "height": 100.0}})
        );
        let p = parse(&s(&["shot", f, "--ref", "e3"]), false).unwrap();
        assert_eq!(args_of(&p)["ref"], "e3");
        // A folder goes as it is (Copper names its file); a relative path
        // is made absolute, a trailing separator kept.
        let d = dir.path().to_str().unwrap();
        let p = parse(&s(&["shot", d, "--jpeg"]), false).unwrap();
        assert_eq!(p.spec.image_path.as_deref(), Some(d));
        assert_eq!(args_of(&p)["filename"], d);
        let here = std::env::current_dir().unwrap();
        let p = parse(&s(&["shot", "rel.png"]), false).unwrap();
        assert_eq!(p.spec.image_path.as_deref(), here.join("rel.png").to_str());
        let p = parse(&s(&["shot", "out/"]), false).unwrap();
        let out = format!(
            "{}{}",
            here.join("out").display(),
            std::path::MAIN_SEPARATOR
        );
        assert_eq!(p.spec.image_path.as_deref(), Some(out.as_str()));
        // `call` too, so the file Copper saves is where the shell meant.
        let call_args = format!(r#"{{"filename": {}}}"#, serde_json::to_string(d).unwrap());
        let p = parse(&s(&["call", "browser_take_screenshot", &call_args]), false).unwrap();
        assert_eq!(args_of(&p)["filename"], d);
        assert_eq!(p.spec.image_path.as_deref(), Some(d));
        let p = parse(
            &s(&[
                "call",
                "browser_take_screenshot",
                r#"{"filename": "rel/x.png"}"#,
            ]),
            false,
        )
        .unwrap();
        assert_eq!(
            args_of(&p)["filename"],
            here.join("rel/x.png").to_str().unwrap()
        );
    }

    #[test]
    fn shot_usage_errors() {
        for (a, e) in [
            (
                &["shot", "--ref", "e1", "--region", "1,2,3,4"][..],
                "give one of ref, selector or region",
            ),
            (
                &["shot", "--ref", "e1", "--frame", "f2"][..],
                "shot --frame goes with --selector",
            ),
            (&["shot", "--scale", "9"][..], "scale must be from 1 to 4"),
            (&["shot", "--scale", "x"][..], "shot --scale needs a number"),
            (
                &["shot", "--scale", "2", "--hd"][..],
                "shot takes --scale N or --hd, not both",
            ),
            (
                &["shot", "--padding", "-1"][..],
                "padding must be 0 or more",
            ),
            (
                &["shot", "--region", "1,2,3"][..],
                "shot --region needs X,Y,W,H: four numbers in CSS pixels",
            ),
            (
                &["shot", "--region", "1,2,0,3"][..],
                "region width and height must be more than 0",
            ),
            (&["shot", "--selector"][..], "shot --selector needs a value"),
            (
                &["shot", "--wide"][..],
                "shot takes one optional PATH and these options",
            ),
            (
                &["shot", "/tmp/a.png", "/tmp/b.png"][..],
                "shot takes one optional PATH and these options",
            ),
        ] {
            match parse(&s(a), false) {
                Err(Early::Usage(t)) => {
                    assert_eq!(t, format!("{e} — usage: copper shot {SHOT_USAGE}"), "{a:?}");
                }
                other => panic!("{a:?}: {other:?}"),
            }
        }
        assert!(USAGE.contains("[--padding N] [--scale N | --hd]"));
    }

    #[test]
    fn a_picture_copper_saved_is_not_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(b"picture");
        let answer = |text: String| {
            json!({"result": {"content": [
                {"type": "image", "data": data, "mimeType": "image/png"},
                {"type": "text", "text": text},
            ]}})
        };
        // What `render` prints for each answer, and its exit code.
        let shown = |spec: &Spec, said: String| {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let none = || None;
            let launch = || Ok(());
            let mut env = Env {
                endpoint: &none,
                launch: &launch,
                launch_env: false,
                out: &mut out,
                err: &mut err,
            };
            let code = render(&mut env, answer(said), spec, false);
            (code, String::from_utf8(out).unwrap())
        };
        let file = dir.path().join("c11.png");
        let f = file.to_str().unwrap();
        let spec = parse(&s(&["shot", f]), false).unwrap().spec;
        let said = format!("Screenshot 4×2 PNG at scale 1 of the viewport. Saved to {f}. ### Page");
        assert_eq!(shown(&spec, said), (0, format!("{f}\n")));
        assert!(!file.exists(), "the CLI wrote what Copper saved");
        // Copper couldn't save: this command writes it.
        let said = format!(
            "Screenshot 4×2 PNG at scale 1 of the viewport. Couldn't save to {f}: denied. P"
        );
        assert_eq!(shown(&spec, said), (0, format!("{f}\n")));
        assert_eq!(std::fs::read(&file).unwrap(), b"picture");
        // `call` with a folder Copper saved into: its file, not the folder.
        let d = dir.path().to_str().unwrap();
        let call_args = format!(r#"{{"filename": {}}}"#, serde_json::to_string(d).unwrap());
        let spec = parse(&s(&["call", "browser_take_screenshot", &call_args]), false)
            .unwrap()
            .spec;
        let inside = dir.path().join("copper-20261117-131500.png");
        let said = format!(
            "Screenshot 4×2 PNG at scale 1 of the viewport. Saved to {}. ### Page",
            inside.display()
        );
        let (code, out) = shown(&spec, said);
        assert_eq!(code, 0);
        assert!(out.starts_with(&format!("{}\n", inside.display())), "{out}");
        assert!(!inside.exists() && dir.path().is_dir());
        // `CLI.savedShot`'s cases.
        let page = " ### Page\n- URL: about:blank";
        let asked =
            format!("Screenshot 1×1 PNG at scale 1 of the viewport. Saved to /tmp/a b.png.{page}");
        assert_eq!(
            saved_to(&[&asked], "/tmp/a b.png").as_deref(),
            Some("/tmp/a b.png")
        );
        let made = format!(
            "Screenshot 1×1 JPEG at scale 1 of the viewport. Saved to /tmp/shots/copper-20261118-110640-2.jpg.{page}"
        );
        let file = Some("/tmp/shots/copper-20261118-110640-2.jpg");
        assert_eq!(saved_to(&[&made], "/tmp/shots").as_deref(), file);
        assert_eq!(saved_to(&[&made], "/tmp/shots/").as_deref(), file);
        let failed = format!(
            "Screenshot 1×1 PNG at scale 1 of the viewport. Couldn't save to /tmp/a.png: no room.{page}"
        );
        assert_eq!(saved_to(&[&failed], "/tmp/a.png"), None);
        assert_eq!(saved_to(&[&made], "/tmp/other"), None);
        assert!(generated_name("copper-20261118-110640.png"));
        assert!(!generated_name("copper-20261118-110640-.png"));
        assert!(!generated_name("copper-2026111-110640.png"));
        assert!(!generated_name("shot.png"));
    }

    #[test]
    fn pins_controls() {
        let request = |a: &[&str]| parse(&s(a), false).unwrap().spec.request.unwrap();
        assert_eq!(request(&["pins"])["method"], "copper/pins");
        assert_eq!(request(&["pins"])["params"], json!({}));
        assert_eq!(request(&["pins", "status"])["params"], json!({}));
        assert_eq!(
            request(&["pins", "sleep", "off"])["params"],
            json!({"on": false})
        );
        assert_eq!(
            request(&["pins", "sleep", "on"])["params"],
            json!({"on": true})
        );
        assert!(matches!(
            parse(&s(&["pins", "sleep"]), false),
            Err(Early::Usage(t)) if t.starts_with("usage: copper pins")
        ));
    }

    #[test]
    fn claude_controls() {
        let params =
            |a: &[&str]| parse(&s(a), false).unwrap().spec.request.unwrap()["params"].clone();
        let p = parse(&s(&["claude"]), false).unwrap();
        assert_eq!(p.spec.request.unwrap()["method"], "copper/claude");
        assert_eq!(params(&["claude"]), json!({"op": "status"}));
        assert_eq!(params(&["claude", "signin"]), json!({"op": "signin"}));
        assert_eq!(
            params(&["claude", "paste", "abc#xyz"]),
            json!({"op": "paste", "code": "abc#xyz"})
        );
        for (a, e) in [
            (&["claude", "paste"][..], "claude paste needs CODE"),
            (
                &["claude", "signout", "x"][..],
                "claude signout takes no arguments",
            ),
            (&["claude", "frob"][..], "unknown claude command: frob"),
        ] {
            assert!(
                matches!(parse(&s(a), false), Err(Early::Usage(t)) if t.starts_with(e)),
                "{a:?}"
            );
        }
        assert_eq!(
            parse(&s(&["claude", "--help"]), false),
            Err(Early::HelpFor(CLAUDE_USAGE))
        );
        assert!(!ELSEWHERE.contains(&"claude"));
    }

    #[test]
    fn bitwarden_save_and_note() {
        let run = |a: &[&str], stdin: &str| bitwarden(&s(a), &mut stdin.as_bytes());
        let params = |a: &[&str], stdin: &str| run(a, stdin).unwrap().request.unwrap();
        let save = params(
            &["save", "-"],
            r#"{"url":"https://x.com","username":"me","password":"pw-1","totp":null,"extra":1}"#,
        );
        assert_eq!(
            save,
            json!({"jsonrpc": "2.0", "id": 1, "method": "copper/bitwarden",
                   "params": {"op": "save", "url": "https://x.com", "username": "me", "password": "pw-1"}})
        );
        let note = params(
            &["note", "-"],
            r#"{"name":"Codes","text":"1111","url":"x"}"#,
        );
        assert_eq!(
            note["params"],
            json!({"op": "note", "name": "Codes", "text": "1111"})
        );
        // Values only on stdin, one JSON object of at most 64 KiB.
        assert_eq!(
            run(&["save", "pw-1"], ""),
            Err(Early::Usage("bitwarden save reads its JSON from stdin: `copper bitwarden save -` (secrets are never accepted as arguments)".into()))
        );
        assert!(run(&["note"], "{}").is_err());
        assert_eq!(
            run(&["note", "-"], "[1]"),
            Err(Early::Usage(
                "bitwarden note - : stdin must be one JSON object".into()
            ))
        );
        let big = format!(r#"{{"name":"n","text":"{}"}}"#, "x".repeat(64 * 1024));
        assert!(run(&["note", "-"], &big).is_err());
        // The rest is the Mac's but not here yet; help is the save/note lines.
        assert_eq!(
            run(&[], ""),
            Err(Early::Usage(
                "bitwarden status is not available in this build yet".into()
            ))
        );
        assert_eq!(
            run(&["frobnicate"], ""),
            Err(Early::Usage(
                "unknown bitwarden command: frobnicate (see copper bitwarden --help)".into()
            ))
        );
        assert_eq!(run(&["--help"], ""), Err(Early::HelpFor(BITWARDEN_USAGE)));
        assert!(!ELSEWHERE.contains(&"bitwarden"));
        // --dry-run shows every key but no value.
        let full = params(
            &["save", "-"],
            r#"{"url":"x.com","username":"me","password":"pw-1","totp":"JBSWY3DP","notes":"n-2"}"#,
        );
        let dry = shown(&full).to_string();
        for secret in ["pw-1", "JBSWY3DP", "n-2"] {
            assert!(!dry.contains(secret), "{dry}");
        }
        assert!(dry.contains("x.com") && dry.contains("\"password\":\"…\""));
        assert!(!shown(&note).to_string().contains("1111"));
    }

    #[test]
    fn intelligence_controls() {
        let params =
            |a: &[&str]| parse(&s(a), false).unwrap().spec.request.unwrap()["params"].clone();
        let p = parse(&s(&["intelligence"]), false).unwrap();
        assert_eq!(
            p.spec.request.unwrap(),
            json!({"jsonrpc": "2.0", "id": 1, "method": "copper/intelligence", "params": {"op": "status"}})
        );
        assert_eq!(
            params(&[
                "intelligence",
                "set",
                "--jev",
                "ts-1",
                "--model",
                "opus",
                "--router-url",
                "https://gw"
            ]),
            json!({"op": "set", "jevKey": "ts-1", "tier": "opus", "routerURL": "https://gw"})
        );
        assert_eq!(
            params(&["intelligence", "refresh"]),
            json!({"op": "refresh"})
        );
        for (a, e) in [
            (
                &["intelligence", "set"][..],
                "intelligence set needs at least one of",
            ),
            (&["intelligence", "set", "--jev"][..], "--jev needs a value"),
            (
                &["intelligence", "set", "--nope", "x"][..],
                "unknown intelligence set option: --nope",
            ),
            (
                &["intelligence", "status", "x"][..],
                "intelligence status takes no arguments",
            ),
            (
                &["intelligence", "frob"][..],
                "unknown intelligence command: frob",
            ),
        ] {
            assert!(
                matches!(parse(&s(a), false), Err(Early::Usage(t)) if t.starts_with(e)),
                "{a:?}"
            );
        }
        assert_eq!(
            parse(&s(&["intelligence", "--help"]), false),
            Err(Early::HelpFor(INTELLIGENCE_USAGE))
        );
        assert!(!ELSEWHERE.contains(&"intelligence"));
    }

    #[test]
    fn downloads_verb() {
        let p = parse(&s(&["downloads"]), false).unwrap();
        let params = &p.spec.request.as_ref().unwrap()["params"];
        assert_eq!(params["name"], "browser_downloads");
        assert_eq!(args_of(&p), json!({}));
        let p = parse(&s(&["downloads", "--wait", "5"]), false).unwrap();
        assert_eq!(args_of(&p), json!({"wait": 5.0}));
        for bad in [
            &["downloads", "--wait"][..],
            &["downloads", "--wait", "soon"],
            &["downloads", "5"],
            &["downloads", "--wait", "5", "x"],
            &["downloads", "--for", "5"],
        ] {
            assert_eq!(
                parse(&s(bad), false),
                Err(Early::Usage("downloads takes [--wait SECONDS]".into())),
                "{bad:?}"
            );
        }
        assert!(USAGE.contains(
            "  downloads [--wait SECONDS]                downloads your tab started, and where the files are\n"
        ));
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

    #[test]
    fn a_handoff_call_may_wait_its_timeout_and_half_a_minute() {
        assert_eq!(handoff_wait(&json!({})), 45);
        assert_eq!(handoff_wait(&json!({"timeout": 600})), 630);
        assert_eq!(handoff_wait(&json!({"timeout": 9000})), 630);
        assert_eq!(handoff_wait(&json!({"timeout": 2.5})), 33);
        assert_eq!(handoff_wait(&json!({"timeout": -1})), 45);
    }
}
