//! Running a vault CLI (`bw`, `op`): a clean environment built by the caller,
//! secrets only through the environment or stdin (never argv), a timeout,
//! and nothing of it logged beyond the subcommand name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

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

/// The executable named `name`, walking `$PATH` the way a shell would.
pub(crate) fn in_path(name: &str, path_var: Option<&str>) -> Option<PathBuf> {
    path_var?
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .find(|p| is_executable(p))
}

pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `~/x` → `$HOME/x`.
pub(crate) fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    match (raw.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(raw),
    }
}

/// Copper's environment with `drop` keys (and any key a `drop_prefix`
/// starts) removed — so a shell's session or token never leaks into a child.
pub(crate) fn scrubbed_env(drop: &[&str], drop_prefix: &[&str]) -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(k, _)| {
            !drop.contains(&k.as_str()) && !drop_prefix.iter().any(|p| k.starts_with(p))
        })
        .collect()
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
    let mut child = Command::new(path)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(RunError::Spawn)?;
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

/// Write `value` to `file`, created 0600 (its folder 0700) before a byte is
/// written. Never the keychain, never logged.
pub(crate) fn write_private(file: &Path, value: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    if let Some(dir) = file.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(file)?;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))?;
    f.write_all(value.as_bytes())
}

/// A 0600 file's trimmed contents, `None` when absent or empty.
pub(crate) fn read_private(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let value = text.trim().to_string();
    (!value.is_empty()).then_some(value)
}
