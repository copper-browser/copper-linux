//! `copper --mcp-stdio` (Fork/MCP/Bridge.swift): JSON-RPC lines on stdin,
//! each POSTed to the running Copper with agent.json's token and an
//! `X-Copper-Agent` identity, each answer written back as one compact line.
//! HTTP 202 (a notification) writes nothing. If Copper is not up it is
//! launched (the caller says how) and waited for, up to 12 s.

use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use copper_pagejs::render::compact_json;
use serde_json::{Value, json};

use crate::client;
use crate::config::{self, Endpoint, PATH};
use crate::who;

/// How long a launched Copper has to bring its port up.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(12);

/// A JSON-RPC error line for a request with an id.
pub fn error_line(id: &Value, text: &str) -> String {
    compact_json(
        &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": text } }),
    )
}

/// Every message that comes in gets the same refusal (requests with an id).
fn answer_all(input: &mut dyn BufRead, out: &mut dyn Write, text: &str) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(id) = m.get("id") {
            writeln!(out, "{}", error_line(id, text))?;
            out.flush()?;
        }
    }
    Ok(())
}

/// The bridge on the real stdin/stdout. `launch` starts Copper without
/// bringing a window forward (the app's own binary with no arguments);
/// it is called only when nothing answers on the port. Returns the exit code.
pub fn run(launch: &dyn Fn() -> io::Result<()>) -> i32 {
    let store = copper_core::Store::from_env();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut input = stdin.lock();
    let mut out = stdout.lock();
    match run_with(
        &mut input,
        &mut out,
        &mut io::stderr(),
        &|| config::endpoint(&store),
        launch,
    ) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("copper: {e}");
            1
        }
    }
}

/// The bridge over any streams; `endpoint` rereads agent.json (the token
/// may rotate while Copper starts).
pub fn run_with(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    log: &mut dyn Write,
    endpoint: &dyn Fn() -> Option<Endpoint>,
    launch: &dyn Fn() -> io::Result<()>,
) -> io::Result<()> {
    let Some(mut conf) = endpoint() else {
        writeln!(
            log,
            "copper: no agent.json yet — open Copper once and turn on Settings › Agents"
        )?;
        return answer_all(
            input,
            out,
            "Copper's agent server is not set up — open Copper, Settings › Agents, turn it on",
        );
    };
    if !conf.enabled {
        writeln!(log, "copper: agent server is off — Settings › Agents")?;
        return answer_all(
            input,
            out,
            "Copper's agent server is off — Settings › Agents › Let agents drive this window",
        );
    }
    if !client::reachable(conf.port) {
        writeln!(log, "copper: launching Copper…")?;
        if let Err(e) = launch() {
            writeln!(log, "copper: could not launch Copper: {e}")?;
        }
        let deadline = Instant::now() + LAUNCH_WAIT;
        while Instant::now() < deadline && !client::reachable(conf.port) {
            std::thread::sleep(Duration::from_millis(250));
        }
        if let Some(fresh) = endpoint() {
            conf = fresh;
        }
    }
    if !client::reachable(conf.port) {
        return answer_all(
            input,
            out,
            &format!("Copper did not come up on port {}", conf.port),
        );
    }

    let mut client_name = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed = serde_json::from_str::<Value>(trimmed).ok();
        if client_name.is_empty()
            && trimmed.contains("\"initialize\"")
            && let Some(name) = parsed
                .as_ref()
                .and_then(|o| o["params"]["clientInfo"]["name"].as_str())
        {
            client_name = name.to_string();
        }
        let headers = [
            ("Content-Type", "application/json".to_string()),
            ("Authorization", format!("Bearer {}", conf.token)),
            (
                "X-Copper-Agent",
                who::environment::header("copper-bridge", &client_name),
            ),
        ];
        let sent = client::request(
            conf.port,
            "POST",
            PATH,
            &headers,
            trimmed.as_bytes(),
            Duration::from_secs(120),
        );
        let (status, data) = match sent {
            Ok(r) => (r.status, r.body),
            Err(e) => {
                let id = parsed
                    .as_ref()
                    .and_then(|o| o.get("id").cloned())
                    .unwrap_or(Value::Null);
                (
                    500,
                    error_line(&id, &format!("Copper unreachable: {e}")).into_bytes(),
                )
            }
        };
        if status == 202 || data.is_empty() {
            continue;
        }
        // One line out per message, whatever the server's formatting.
        match serde_json::from_slice::<Value>(&data) {
            Ok(v) => writeln!(out, "{}", compact_json(&v))?,
            Err(_) => writeln!(out, "{}", String::from_utf8_lossy(&data))?,
        }
        out.flush()?;
    }
}
