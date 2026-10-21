//! `copper botpass pair|status|unpair` — pairing with BotPass from the
//! command line, with no window (what `bp connect copper` runs).
//!
//! The pairing is kept in this profile's `botpass.json` (see
//! [`crate::pairing`]); a running Copper picks it up on its next fill. No
//! token is ever printed.
//!
//! Exit codes: 0 done (status: paired and the token works, locked or not),
//! 1 BotPass refused or couldn't be reached (status: not paired, revoked or
//! unreachable), 2 usage.

use crate::client::Server;
use crate::error::Error;
use crate::pairing::{self, Pairing};
use crate::source::default_device_name;
use serde_json::{Value, json};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const USAGE: &str = "Usage: copper botpass <command> [--json]

  pair --server URL --code CODE [--fingerprint FP] [--name NAME]
  pair 'botpass://host:port/#c=CODE&fp=FP' [--name NAME]
                     pair this Copper with BotPass as one of its bots, with no window:
                     writes this profile's botpass.json (0600). CODE comes from
                     `bp pair <bot>` or `bp connect copper`; FP is the server's TLS
                     certificate SHA-256 (bp status / GET /v1/status). Re-pairing replaces
                     the old pairing.
  status             the server, this device, the bot, and whether BotPass answers
                     (unlocked, locked, revoked); never the token
  unpair             forget the pairing here (remove the device in BotPass too,
                     to revoke its token)

  --server URL       https://host:port (plain http only for this machine)
  --code CODE        the pairing code, ABCD-EFGH (10 minutes, single use)
  --fingerprint FP   SHA-256 of BotPass's certificate, hex (colons allowed)
  --name NAME        what BotPass calls this Copper (default: Copper on <host>)
  --json             print one JSON object: {\"ok\":true,…} or
                     {\"ok\":false,\"error\",\"message\"}

COPPER_WORLD=<name> picks the profile, as for the browser. Exit codes: 0 done, 1 BotPass
refused or couldn't be reached (status: not paired or not working), 2 usage.
";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verb {
    #[default]
    Help,
    Pair,
    Status,
    Unpair,
}

/// What one `copper botpass …` asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Command {
    pub verb: Verb,
    pub server: Option<String>,
    pub code: Option<String>,
    pub fingerprint: Option<String>,
    pub name: Option<String>,
    pub json: bool,
}

/// Whether the first word, past the CLI's own flags, is `botpass`.
pub fn is_botpass(args: &[String]) -> bool {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--json" | "--dry-run" | "--launch" => i += 1,
            "--tab" => i += 2,
            word => return word == "botpass",
        }
    }
    false
}

/// Read a `copper [flags] botpass …` command line.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let at = args
        .iter()
        .position(|a| a == "botpass")
        .map_or(0, |at| at + 1);
    let mut cmd = Command {
        json: args[..at.saturating_sub(1).min(args.len())]
            .iter()
            .any(|a| a == "--json"),
        ..Default::default()
    };
    let mut words: Vec<String> = vec![];
    let mut it = args[at.min(args.len())..].iter().cloned();
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
            "--server" | "--url" => cmd.server = Some(value("--server")?),
            "--code" => cmd.code = Some(value("--code")?),
            "--fingerprint" | "--trust" => cmd.fingerprint = Some(value("--fingerprint")?),
            "--name" => cmd.name = Some(value("--name")?),
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
        Some("status") => Verb::Status,
        Some("unpair") => Verb::Unpair,
        Some(other) => return Err(format!("unknown botpass command: {other}")),
    };
    // `pair CODE` works as well as `pair --code CODE`, and so does a
    // `botpass://host/#c=CODE&fp=FP` link (flags win over what it carries).
    if let Some(code) = words.next() {
        if cmd.verb != Verb::Pair || cmd.code.is_some() {
            return Err(format!("unexpected argument: {code}"));
        }
        match crate::client::parse_link(&code) {
            Some(link) => {
                cmd.server.get_or_insert(link.server);
                cmd.code = Some(link.code);
                if cmd.fingerprint.is_none() {
                    cmd.fingerprint = link.fingerprint;
                }
            }
            None => cmd.code = Some(code),
        }
    }
    if let Some(extra) = words.next() {
        return Err(format!("unexpected argument: {extra}"));
    }
    if cmd.verb == Verb::Pair {
        if cmd.server.is_none() {
            return Err("pair needs --server URL".into());
        }
        if cmd.code.is_none() {
            return Err("pair needs --code CODE".into());
        }
    }
    Ok(cmd)
}

/// `copper botpass …` on the real environment (this `COPPER_WORLD`'s data
/// folder).
pub fn run(args: &[String]) -> i32 {
    let data = copper_core::store::Paths::from_env().data;
    let stdout = io::stdout();
    let stderr = io::stderr();
    run_with(args, &data, &mut stdout.lock(), &mut stderr.lock())
}

/// `copper botpass …` against any data folder and streams.
pub fn run_with(args: &[String], data: &Path, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let cmd = match parse(args) {
        Ok(c) => c,
        Err(text) => {
            let _ = writeln!(err, "copper: {text}");
            let _ = writeln!(err, "(copper botpass help shows the commands)");
            return 2;
        }
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = writeln!(err, "copper: {e}");
            return 1;
        }
    };
    let mut io = Io {
        cmd: &cmd,
        out,
        err,
    };
    match cmd.verb {
        Verb::Help => {
            let _ = write!(io.out, "{USAGE}");
            0
        }
        Verb::Pair => rt.block_on(pair(&mut io, data)),
        Verb::Status => rt.block_on(status(&mut io, data)),
        Verb::Unpair => unpair(&mut io, data),
    }
}

struct Io<'a> {
    cmd: &'a Command,
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
}

impl Io<'_> {
    fn json(&mut self, v: &Value) {
        let _ = writeln!(
            self.out,
            "{}",
            serde_json::to_string_pretty(v).unwrap_or_default()
        );
    }

    fn fail(&mut self, e: &Error, exit: i32) -> i32 {
        if self.cmd.json {
            let mut v = json!({"ok": false, "error": e.code(), "message": e.to_string()});
            if let Some(seen) = e.seen() {
                v["fingerprint"] = json!(seen);
            }
            self.json(&v);
        } else {
            let _ = writeln!(self.err, "copper: {e}");
            if let Some(seen) = e.seen() {
                let _ = writeln!(
                    self.err,
                    "The server showed this certificate (SHA-256):\n  {seen}\nIf it matches BotPass's (bp status), run again with --fingerprint {seen}"
                );
            }
        }
        exit
    }
}

fn described(p: &Pairing, path: &Path) -> Value {
    json!({
        "server": p.url,
        "fingerprint": p.fingerprint,
        "device_id": p.device_id,
        "device_name": p.device_name,
        "bot": p.bot,
        "personal": p.personal,
        "path": path.display().to_string(),
    })
}

async fn pair(io: &mut Io<'_>, data: &Path) -> i32 {
    let cmd = io.cmd;
    let server = match Server::new(
        cmd.server.as_deref().unwrap_or_default(),
        cmd.fingerprint.as_deref(),
    ) {
        Ok(s) => s,
        Err(e) => return io.fail(&e, 2),
    };
    let name = cmd.name.clone().unwrap_or_else(default_device_name);
    let p = match server
        .pair(cmd.code.as_deref().unwrap_or_default(), &name)
        .await
    {
        Ok(p) => p,
        Err(e) => return io.fail(&e, 1),
    };
    if let Err(e) = pairing::save(data, &p) {
        return io.fail(&e, 1);
    }
    let path: PathBuf = pairing::path(data);
    if cmd.json {
        let mut v = described(&p, &path);
        v["ok"] = json!(true);
        io.json(&v);
    } else {
        let who = match (&p.bot, p.personal) {
            (_, true) => " (personal)".to_string(),
            (Some(b), _) => format!(" as bot {b}"),
            _ => String::new(),
        };
        let _ = writeln!(io.out, "Paired with BotPass at {}{who}.", p.url);
        let _ = writeln!(io.out, "This device: {} ({})", p.device_name, p.device_id);
        let _ = writeln!(io.out, "Kept in {}", path.display());
        if server.pin().is_none() && p.url.starts_with("https:") {
            let _ = writeln!(
                io.err,
                "copper: note: no fingerprint given; BotPass's certificate is trusted through the public authorities"
            );
        }
    }
    0
}

async fn status(io: &mut Io<'_>, data: &Path) -> i32 {
    let path = pairing::path(data);
    let p = match pairing::load(data) {
        Ok(Some(p)) => p,
        Ok(None) => {
            if io.cmd.json {
                io.json(&json!({"ok": true, "paired": false, "path": path.display().to_string()}));
            } else {
                let _ = writeln!(
                    io.out,
                    "BotPass: not paired (run bp connect copper, or copper botpass pair)"
                );
            }
            return 1;
        }
        Err(e) => return io.fail(&e, 1),
    };
    // Is it there, is it locked, and is the token still good? Listing for
    // an origin nothing is granted on answers all three without a secret.
    let probe = async {
        let server = Server::for_pairing(&p)?;
        let st = server.status().await?;
        let token = match server.accounts(&p.token, "https://botpass.invalid").await {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        Ok::<_, Error>((st, token))
    };
    let (state, version, exit) = match probe.await {
        Ok((st, Ok(()))) => (if st.locked { "locked" } else { "unlocked" }, st.version, 0),
        Ok((st, Err(Error::Locked))) => ("locked", st.version, 0),
        Ok((st, Err(Error::Unpaired))) => ("revoked", st.version, 1),
        Ok((st, Err(e))) => {
            let _ = writeln!(io.err, "copper: {e}");
            ("error", st.version, 1)
        }
        Err(e) => {
            if !io.cmd.json {
                let _ = writeln!(io.err, "copper: {e}");
            }
            ("unreachable", String::new(), 1)
        }
    };
    if io.cmd.json {
        let mut v = described(&p, &path);
        v["ok"] = json!(true);
        v["paired"] = json!(true);
        v["state"] = json!(state);
        v["version"] = json!(version);
        io.json(&v);
    } else {
        let bot = p
            .bot
            .as_deref()
            .map(|b| format!(" as bot {b}"))
            .unwrap_or_default();
        let _ = writeln!(io.out, "BotPass: paired with {}{bot} — {state}", p.url);
        let _ = writeln!(
            io.out,
            "Certificate: {}",
            match &p.fingerprint {
                Some(f) => format!("pinned ({f})"),
                None if p.url.starts_with("http:") => "none (loopback http)".into(),
                None => "public authorities".into(),
            }
        );
        let _ = writeln!(io.out, "This device: {} ({})", p.device_name, p.device_id);
        let _ = writeln!(io.out, "Kept in {}", path.display());
        if state == "locked" {
            let _ = writeln!(io.out, "BotPass is locked — run bp unlock");
        }
        if state == "revoked" {
            let _ = writeln!(
                io.out,
                "BotPass no longer knows this device — pair again with bp connect copper"
            );
        }
    }
    exit
}

fn unpair(io: &mut Io<'_>, data: &Path) -> i32 {
    let before = pairing::load(data).ok().flatten();
    match pairing::remove(data) {
        Ok(had) => {
            if io.cmd.json {
                io.json(&json!({"ok": true, "removed": had}));
            } else if had {
                let id = before.map(|p| p.device_id).unwrap_or_default();
                let _ = writeln!(io.out, "Forgot the BotPass pairing here.");
                if !id.is_empty() {
                    let _ = writeln!(
                        io.out,
                        "BotPass still lists device {id}; remove it there to revoke its token."
                    );
                }
            } else {
                let _ = writeln!(io.out, "BotPass: not paired.");
            }
            0
        }
        Err(e) => io.fail(&e, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn recognises_the_word() {
        assert!(is_botpass(&args("botpass pair")));
        assert!(is_botpass(&args("--json botpass status")));
        assert!(!is_botpass(&args("cloud status")));
        assert!(!is_botpass(&args("--tab botpass status")));
    }

    #[test]
    fn parses_pair() {
        let c = parse(&args(
            "botpass pair --server https://h:1 --code ABCD-EFGH --fingerprint=AB --name x --json",
        ))
        .unwrap();
        assert_eq!(c.verb, Verb::Pair);
        assert_eq!(c.server.as_deref(), Some("https://h:1"));
        assert_eq!(c.code.as_deref(), Some("ABCD-EFGH"));
        assert_eq!(c.fingerprint.as_deref(), Some("AB"));
        assert_eq!(c.name.as_deref(), Some("x"));
        assert!(c.json);
        let c = parse(&args("botpass pair ABCD-EFGH --server h")).unwrap();
        assert_eq!(c.code.as_deref(), Some("ABCD-EFGH"));
        let c = parse(&args(
            "botpass pair botpass://bp.lan:7443/#c=WXYZ-2345&fp=ab",
        ))
        .unwrap();
        assert_eq!(c.server.as_deref(), Some("https://bp.lan:7443"));
        assert_eq!(c.code.as_deref(), Some("WXYZ-2345"));
        assert_eq!(c.fingerprint.as_deref(), Some("ab"));
        assert!(parse(&args("botpass pair --server h")).is_err());
        assert!(parse(&args("botpass pair --code X")).is_err());
        assert!(parse(&args("botpass status extra")).is_err());
        assert!(parse(&args("botpass frob")).is_err());
        assert!(parse(&args("botpass pair --bogus")).is_err());
        assert_eq!(parse(&args("botpass")).unwrap().verb, Verb::Help);
        assert!(parse(&args("--json botpass status")).unwrap().json);
    }

    #[test]
    fn usage_exits_2_and_status_unpaired_exits_1() {
        let dir = tempfile::tempdir().unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            run_with(&args("botpass pair"), dir.path(), &mut out, &mut err),
            2
        );
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            run_with(
                &args("botpass status --json"),
                dir.path(),
                &mut out,
                &mut err
            ),
            1
        );
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["paired"], json!(false));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            run_with(&args("botpass unpair"), dir.path(), &mut out, &mut err),
            0
        );
    }
}
