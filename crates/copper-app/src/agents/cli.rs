//! What `copper` does before GTK starts when it is not a browser launch:
//!
//! * `copper --mcp-stdio` — the stdio bridge (Mac `Bridge.swift`): JSON-RPC
//!   lines to the running Copper's MCP endpoint, launching Copper when
//!   nothing answers.
//! * `copper --cli <verb> …` and `copper <verb> …` — the command-line
//!   client (`copper_mcp::cli`): `copper tabs`, `copper health`,
//!   `copper snapshot`…
//!
//! Neither opens a display. Everything else (`--version`, addresses,
//! `--screenshot`…) is the app's.

use std::io;
use std::process::{Command, Stdio};

/// The verbs `copper_mcp::cli` answers (its `make_request`), with the
/// Mac's verbs it says are not in this build yet and `help`.
pub const VERBS: &[&str] = &[
    "tabs",
    "open",
    "use",
    "show",
    "go",
    "signin",
    "autofill",
    "observe",
    "run",
    "step",
    "extract",
    "snapshot",
    "text",
    "find",
    "shot",
    "eval",
    "click",
    "type",
    "key",
    "tools",
    "health",
    "call",
    "cloud-history",
    "help",
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

/// Run a bridge or CLI invocation and return its exit code, or `None` for
/// an ordinary launch.
pub fn dispatch(args: &[String]) -> Option<i32> {
    if args.iter().any(|a| a == "--mcp-stdio") {
        return Some(copper_mcp::bridge::run(&launch));
    }
    if let Some(at) = args.iter().position(|a| a == "--cli") {
        return Some(copper_mcp::cli::run(&args[at + 1..]));
    }
    if is_cli(args) {
        return Some(copper_mcp::cli::run(args));
    }
    None
}

/// Whether the first word, past the CLI's own flags, is a CLI verb.
pub fn is_cli(args: &[String]) -> bool {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        match a.as_str() {
            "--json" | "--dry-run" | "--launch" => i += 1,
            "--tab" => i += 2,
            word => return VERBS.contains(&word),
        }
    }
    false
}

/// Start Copper for the bridge: this binary with no arguments, detached
/// from the bridge's process group and streams (an MCP client closing the
/// bridge must not take the browser with it).
fn launch() -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(words: &[&str]) -> Vec<String> {
        words.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn verbs_go_to_the_cli_and_addresses_to_the_app() {
        assert!(is_cli(&v(&["tabs"])));
        assert!(is_cli(&v(&["--json", "health"])));
        assert!(is_cli(&v(&["--tab", "ab12", "snapshot", "--interactive"])));
        assert!(!is_cli(&v(&["example.com"])));
        assert!(!is_cli(&v(&["--version"])));
        assert!(!is_cli(&v(&[])));
        assert!(!is_cli(&v(&["--tab"])));
        assert_eq!(dispatch(&v(&["--version"])), None);
        assert_eq!(dispatch(&v(&["rust lifetimes"])), None);
    }

    #[test]
    fn every_cli_verb_is_known_here() {
        for verb in [
            "tabs",
            "open",
            "use",
            "show",
            "go",
            "signin",
            "autofill",
            "observe",
            "run",
            "step",
            "extract",
            "snapshot",
            "text",
            "find",
            "shot",
            "eval",
            "click",
            "type",
            "key",
            "tools",
            "health",
            "call",
            "cloud-history",
        ] {
            // A verb copper_mcp::cli parses must route there, not open a tab.
            let parsed = copper_mcp::cli::make_request(verb, &[]);
            let unknown = matches!(&parsed, Err(copper_mcp::cli::Early::Usage(t)) if t.starts_with("unknown command"));
            assert!(!unknown, "{verb} is not a cli verb");
            assert!(VERBS.contains(&verb));
        }
    }
}
