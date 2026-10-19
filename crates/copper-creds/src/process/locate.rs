//! Finding a vault CLI (`bw`, `op`).
//!
//! - Linux: an override, then `~/.local/bin/<name>` (desktop sessions often
//!   lack it on `PATH`), then `$PATH`; a file counts only with an executable
//!   bit.
//! - Windows: an override, then `%PATH%` — each folder tried with each
//!   launchable `PATHEXT` extension in `PATHEXT` order, as `cmd.exe` does,
//!   but never the current folder — then the folders installers put these
//!   CLIs in (winget, npm, Scoop, Chocolatey, the vendors' own), for when the
//!   `PATH` Copper inherited predates the install. A file counts only with an
//!   extension `CreateProcess` can start: `.exe`, `.com`, `.bat`, `.cmd` (npm
//!   installs `bw.cmd` beside an extensionless `bw` shell script, which must
//!   not win).

use std::path::{Path, PathBuf};

/// The user's home: `$HOME`, or on Windows `%USERPROFILE%`.
pub(crate) fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// `~/x` → `<home>/x` (on Windows `~\x` too).
pub(crate) fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    let rest = raw
        .strip_prefix("~/")
        .or_else(|| raw.strip_prefix("~\\").filter(|_| cfg!(windows)));
    match (rest, home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(raw),
    }
}

/// Whether `path` names a batch file (`.bat`/`.cmd`), which Windows runs
/// through `cmd.exe`.
pub(crate) fn is_batch(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("bat") || e.eq_ignore_ascii_case("cmd"))
}

/// Extensions `CreateProcess` can start, lowercase.
#[cfg(any(windows, test))]
const LAUNCHABLE: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];

/// The launchable extensions of a `PATHEXT` value, in its order (`.vbs`,
/// `.ps1`… are dropped: they need a host program); the Windows default when
/// it names none.
#[cfg(any(windows, test))]
fn launchable_exts(pathext: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for ext in pathext.unwrap_or_default().split(';') {
        let ext = ext.trim().to_ascii_lowercase();
        if LAUNCHABLE.contains(&ext.as_str()) && !out.contains(&ext) {
            out.push(ext);
        }
    }
    if out.is_empty() {
        out = LAUNCHABLE.iter().map(|e| e.to_string()).collect();
    }
    out
}

/// The file names to try for `name`: itself when it already ends in a
/// launchable extension, else `name` + each extension.
#[cfg(any(windows, test))]
fn candidates(name: &str, exts: &[String]) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    if exts.iter().any(|e| lower.ends_with(e.as_str())) {
        return vec![name.to_string()];
    }
    exts.iter().map(|e| format!("{name}{e}")).collect()
}

#[cfg(windows)]
fn pathext() -> Vec<String> {
    launchable_exts(std::env::var("PATHEXT").ok().as_deref())
}

/// Whether `path` is a file this platform will run.
#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Whether `path` is a file this platform will run.
#[cfg(windows)]
pub(crate) fn is_executable(path: &Path) -> bool {
    let launchable = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| LAUNCHABLE.contains(&format!(".{}", e.to_ascii_lowercase()).as_str()));
    launchable && path.is_file()
}

/// An override path as given, when it is runnable. On Windows a path
/// without a launchable extension (`C:\tools\bw`) also tries `bw.exe`,
/// `bw.cmd`… in `PATHEXT` order.
pub(crate) fn resolve(path: &Path) -> Option<PathBuf> {
    if is_executable(path) {
        return Some(path.to_path_buf());
    }
    with_extensions(path)
}

#[cfg(windows)]
fn with_extensions(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    candidates(name, &pathext())
        .into_iter()
        .map(|c| path.with_file_name(c))
        .find(|p| is_executable(p))
}

#[cfg(not(windows))]
fn with_extensions(_path: &Path) -> Option<PathBuf> {
    None
}

/// The executable named `name` in the folders of a `PATH` value, the way a
/// shell finds it (`std::env::split_paths`: `:` on Unix, `;` and quoting
/// on Windows; empty entries skipped).
pub(crate) fn in_path(name: &str, path_var: Option<&str>) -> Option<PathBuf> {
    in_dirs(name, std::env::split_paths(path_var?))
}

fn in_dirs(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    #[cfg(windows)]
    let names = candidates(name, &pathext());
    #[cfg(not(windows))]
    let names = [name.to_string()];
    dirs.into_iter()
        .filter(|d| !d.as_os_str().is_empty())
        .flat_map(|d| names.iter().map(move |n| d.join(n)).collect::<Vec<_>>())
        .find(|p| is_executable(p))
}

/// Folders a vault CLI lands in without (yet) being on Copper's `PATH`.
#[cfg(not(windows))]
fn install_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    home.map(|h| h.join(".local/bin")).into_iter().collect()
}

/// Folders a vault CLI lands in without (yet) being on Copper's `PATH`:
/// winget's links, npm's global bin (`bw.cmd`), Scoop's shims, Chocolatey's
/// bin, and the vendors' documented manual-install folders.
#[cfg(windows)]
fn install_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let local = env("LOCALAPPDATA").or_else(|| home.map(|h| h.join("AppData\\Local")));
    let roaming = env("APPDATA").or_else(|| home.map(|h| h.join("AppData\\Roaming")));
    let scoop = env("SCOOP").or_else(|| home.map(|h| h.join("scoop")));
    let mut dirs = Vec::new();
    dirs.extend(local.map(|d| d.join("Microsoft\\WinGet\\Links")));
    dirs.extend(roaming.map(|d| d.join("npm")));
    dirs.extend(scoop.map(|d| d.join("shims")));
    dirs.extend(env("ProgramData").map(|d| d.join("chocolatey\\bin")));
    if let Some(pf) = env("ProgramFiles") {
        dirs.push(pf.join("1Password CLI"));
        dirs.push(pf.join("Bitwarden CLI"));
    }
    dirs
}

/// `name` where this platform's users install it; see the module docs for
/// the order.
pub(crate) fn find_tool(
    name: &str,
    home: Option<&Path>,
    path_var: Option<&str>,
) -> Option<PathBuf> {
    if cfg!(windows) {
        in_path(name, path_var).or_else(|| in_dirs(name, install_dirs(home)))
    } else {
        in_dirs(name, install_dirs(home)).or_else(|| in_path(name, path_var))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathext_keeps_launchable_extensions_in_order() {
        assert_eq!(
            launchable_exts(Some(
                ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC;.PY;.PYW;.CPL"
            )),
            [".com", ".exe", ".bat", ".cmd"]
        );
        assert_eq!(launchable_exts(Some(".CMD; .exe ;.cmd")), [".cmd", ".exe"]);
        assert_eq!(
            launchable_exts(Some(".PS1;.VBS")),
            [".com", ".exe", ".bat", ".cmd"]
        );
        assert_eq!(launchable_exts(None), [".com", ".exe", ".bat", ".cmd"]);
    }

    #[test]
    fn candidates_add_extensions_unless_one_is_there() {
        let exts = launchable_exts(None);
        assert_eq!(
            candidates("bw", &exts),
            ["bw.com", "bw.exe", "bw.bat", "bw.cmd"]
        );
        assert_eq!(candidates("op.EXE", &exts), ["op.EXE"]);
        assert_eq!(candidates("bw.js", &exts).len(), 4);
    }

    #[test]
    fn batch_files_and_tilde() {
        assert!(is_batch(Path::new("C:/x/bw.CMD")));
        assert!(is_batch(Path::new("run.bat")));
        assert!(!is_batch(Path::new("op.exe")));
        assert!(!is_batch(Path::new("bw")));
        let home = Path::new("/h");
        assert_eq!(expand_tilde("~/bin/bw", Some(home)), home.join("bin/bw"));
        assert_eq!(expand_tilde("~/bin/bw", None), PathBuf::from("~/bin/bw"));
        assert_eq!(
            expand_tilde("/abs/bw", Some(home)),
            PathBuf::from("/abs/bw")
        );
        assert_eq!(
            expand_tilde("~\\bin\\bw.exe", Some(home)),
            if cfg!(windows) {
                home.join("bin\\bw.exe")
            } else {
                PathBuf::from("~\\bin\\bw.exe")
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_lookup_honours_pathext_and_skips_scripts() {
        let dir = std::env::temp_dir().join(format!("copper-locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let npm = dir.join("npm");
        let other = dir.join("other dir;x");
        for d in [&npm, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        // npm's layout: an extensionless sh script, a .cmd and a .ps1.
        for f in ["bw", "bw.cmd", "bw.ps1"] {
            std::fs::write(npm.join(f), "x").unwrap();
        }
        std::fs::write(other.join("op.exe"), "x").unwrap();
        let path_var = std::env::join_paths([&npm, &other])
            .unwrap()
            .into_string()
            .unwrap();
        assert_eq!(in_path("bw", Some(&path_var)), Some(npm.join("bw.cmd")));
        assert_eq!(in_path("op", Some(&path_var)), Some(other.join("op.exe")));
        assert_eq!(in_path("bw.cmd", Some(&path_var)), Some(npm.join("bw.cmd")));
        assert!(!is_executable(&npm.join("bw")));
        assert!(!is_executable(&npm.join("bw.ps1")));
        assert_eq!(resolve(&npm.join("bw")), Some(npm.join("bw.cmd")));
        assert_eq!(resolve(&other.join("op.EXE")), Some(other.join("op.EXE")));
        assert_eq!(resolve(&other.join("nope")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
