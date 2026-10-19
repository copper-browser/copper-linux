//! Running a vault CLI (`bw`, `op`): a clean environment built by the caller,
//! secrets only through the environment or stdin (never argv), a timeout,
//! and nothing of it logged beyond the subcommand name.
//!
//! On Windows the child gets no console window (`CREATE_NO_WINDOW`: Copper
//! is a GUI app, and `bw.cmd`/`op.exe` would otherwise flash one), its
//! arguments are checked before a batch file (`bw.cmd` from npm) sees them,
//! and its whole process tree — `cmd.exe` *and* the `node.exe` it starts —
//! ends with the call ([`job`]).

#[cfg(windows)]
mod job;
mod locate;
mod private;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub(crate) use locate::{expand_tilde, find_tool, home_dir, resolve};
pub(crate) use private::{private_dir, read_private, write_private};

/// What a child said.
pub(crate) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: i32,
}

pub(crate) enum RunError {
    TimedOut,
    Spawn(std::io::Error),
}

/// Whether environment variable names `a` and `b` are the same variable:
/// exact on Unix, ASCII case-insensitive on Windows (where `Path` is `PATH`).
fn same_key(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn key_starts(key: &str, prefix: &str) -> bool {
    key.get(..prefix.len())
        .is_some_and(|head| same_key(head, prefix))
}

/// Copper's environment with `drop` keys (and any key a `drop_prefix`
/// starts) removed — so a shell's session or token never leaks into a child.
/// A variable that isn't valid Unicode is left out rather than panicking.
pub(crate) fn scrubbed_env(drop: &[&str], drop_prefix: &[&str]) -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| {
            !drop.iter().any(|d| same_key(k, d)) && !drop_prefix.iter().any(|p| key_starts(k, p))
        })
        .collect()
}

/// Refuse arguments a Windows command line can't carry faithfully.
///
/// NUL ends the command line and a line break ends a batch file's command,
/// so neither may appear. For a batch file (`bw.cmd`) the standard library
/// already quotes and escapes for `cmd.exe` (Rust ≥ 1.77, the CVE-2024-24576
/// fix) and refuses what it can't escape; a `"` is refused here as well,
/// because how the script re-parses `%*` for the program behind it is not
/// ours to control. Copper never puts a secret in an argument; these are
/// ids, emails, server URLs and subcommands.
#[cfg_attr(not(windows), allow(dead_code))]
fn check_args(program: &Path, args: &[String]) -> std::io::Result<()> {
    let batch = locate::is_batch(program);
    for a in args {
        let bad = a.contains(['\0', '\r', '\n']) || (batch && a.contains('"'));
        if bad {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "an argument contains a character the command line can't carry",
            ));
        }
    }
    Ok(())
}

/// Run `path args…` with exactly `env`, feed `stdin`, wait at most `timeout`
/// (the child is killed when it runs over).
pub(crate) async fn execute(
    path: &Path,
    args: &[String],
    env: &BTreeMap<String, String>,
    stdin: Option<&[u8]>,
    timeout: Duration,
) -> Result<Output, RunError> {
    let mut command = Command::new(path);
    command
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        check_args(path, args).map_err(RunError::Spawn)?;
        command.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    let mut child = command.spawn().map_err(RunError::Spawn)?;
    // Held to the end of this call: closing it ends the child's tree.
    #[cfg(windows)]
    let _tree = job::KillTree::adopt(&child);
    let mut input = child.stdin.take();
    let feed = stdin.map(<[u8]>::to_vec).unwrap_or_default();
    // Fed off this future so a big batch can't deadlock against full stdout;
    // a child that exits without reading must not take us down (no SIGPIPE
    // in Rust: the write just fails).
    let writer = tokio::spawn(async move {
        if let Some(mut pipe) = input.take() {
            if !feed.is_empty() {
                let _ = pipe.write_all(&feed).await;
            }
            let _ = pipe.shutdown().await;
        }
    });
    let waited = tokio::time::timeout(timeout, child.wait_with_output()).await;
    writer.abort();
    match waited {
        Ok(Ok(out)) => Ok(Output {
            stdout: out.stdout,
            stderr: out.stderr,
            status: out.status.code().unwrap_or(-1),
        }),
        Ok(Err(e)) => Err(RunError::Spawn(e)),
        // Dropping the child (kill_on_drop) ends it.
        Err(_) => Err(RunError::TimedOut),
    }
}

/// Trailing newlines off (`trimTrailingNewlines`).
pub(crate) fn trim_newlines(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\n', '\r'])
        .trim_start_matches(['\n', '\r'])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_prefixes_match_whole_heads() {
        assert!(key_starts("OP_SESSION_ABC", "OP_SESSION"));
        assert!(!key_starts("OP_SESS", "OP_SESSION"));
        assert!(same_key("BW_SESSION", "BW_SESSION"));
        assert_eq!(same_key("bw_session", "BW_SESSION"), cfg!(windows));
        assert_eq!(key_starts("op_session_x", "OP_SESSION"), cfg!(windows));
    }

    #[test]
    fn arguments_a_command_line_cannot_carry() {
        let exe = Path::new("op.exe");
        let cmd = Path::new("bw.CMD");
        let ok = |s: &str| vec![s.to_string()];
        assert!(check_args(exe, &ok("https://vault.example.com/?a=1&b=%20")).is_ok());
        assert!(check_args(cmd, &ok("https://vault.example.com/?a=1&b=%20")).is_ok());
        assert!(check_args(exe, &ok("say \"hi\"")).is_ok());
        assert!(check_args(cmd, &ok("say \"hi\"")).is_err());
        for bad in ["a\nb", "a\rb", "a\0b"] {
            assert!(check_args(exe, &ok(bad)).is_err(), "{bad:?}");
            assert!(check_args(cmd, &ok(bad)).is_err(), "{bad:?}");
        }
    }
}
