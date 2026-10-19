//! Small private files beside a vault CLI's data (a persisted session
//! token): never the keychain, never logged.
//!
//! On Unix they are created 0600 in a 0700 folder before a byte is written.
//! On Windows there are no mode bits to set and none are needed: the files
//! live under the user's profile (`%LOCALAPPDATA%\Copper\…`), whose
//! inherited ACL already grants only the user, SYSTEM and Administrators.

use std::path::Path;

/// Create `dir` (and its parents) private to the user.
pub(crate) fn private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        // Per-user profile ACL; see the module docs.
        std::fs::create_dir_all(dir)
    }
}

/// Write `value` to `file`, created 0600 (its folder 0700) before a byte is
/// written on Unix.
#[cfg(unix)]
pub(crate) fn write_private(file: &Path, value: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = file.parent() {
        private_dir(dir)?;
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

/// Write `value` to `file`, its folder created first. No permission bits
/// on Windows: the per-user profile ACL covers it (see the module docs).
#[cfg(not(unix))]
pub(crate) fn write_private(file: &Path, value: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        private_dir(dir)?;
    }
    std::fs::write(file, value.as_bytes())
}

/// A private file's trimmed contents, `None` when absent or empty.
pub(crate) fn read_private(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    let value = text.trim().to_string();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_files_round_trip() {
        let dir = std::env::temp_dir().join(format!("copper-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("a").join("session");
        assert_eq!(read_private(&file), None);
        write_private(&file, "tok-1\n").unwrap();
        assert_eq!(read_private(&file).as_deref(), Some("tok-1"));
        write_private(&file, "t2").unwrap();
        assert_eq!(read_private(&file).as_deref(), Some("t2"));
        write_private(&file, "  ").unwrap();
        assert_eq!(read_private(&file), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&file), 0o600);
            assert_eq!(mode(&dir.join("a")), 0o700);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
