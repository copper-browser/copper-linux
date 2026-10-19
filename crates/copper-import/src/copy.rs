//! Private copies of another browser's files.
//!
//! The browser a file belongs to is usually running. On Linux nothing stops
//! a read, so a plain copy is taken. On Windows the source is opened with
//! every share flag (`FILE_SHARE_READ | FILE_SHARE_WRITE |
//! FILE_SHARE_DELETE`), so the copy never stands in the browser's way; a
//! browser that opened its file without sharing it still refuses, and that
//! refusal comes back as [`ImportError::InUse`] so the person can be asked
//! to close the browser and try again.

use std::io;
use std::path::{Path, PathBuf};

use crate::ImportError;

/// A private copy of `file` in the temp dir, removed on drop.
pub(crate) struct Copied {
    pub path: PathBuf,
}

impl Copied {
    pub fn of(file: &Path) -> io::Result<Self> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "copper-import-{}-{n}-{}",
            std::process::id(),
            file.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("file")
                .replace(' ', "_")
        ));
        // Constructed first, so a copy that fails halfway is removed too.
        let copied = Copied { path };
        copy_file(file, &copied.path)?;
        // A WAL beside a SQLite file holds its newest rows; copy it too.
        for side in ["-wal", "-journal"] {
            let from = with_suffix(file, side);
            if from.is_file() {
                let _ = copy_file(&from, &with_suffix(&copied.path, side));
            }
        }
        Ok(copied)
    }
}

impl Drop for Copied {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        for side in ["-wal", "-journal", "-shm"] {
            let _ = std::fs::remove_file(with_suffix(&self.path, side));
        }
    }
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut p = file.as_os_str().to_owned();
    p.push(suffix);
    PathBuf::from(p)
}

/// A file's bytes, read from a copy.
pub(crate) fn copy_read(file: &Path) -> io::Result<Vec<u8>> {
    let copy = Copied::of(file)?;
    std::fs::read(&copy.path)
}

/// A file's bytes read in place, sharing it with whoever else has it open
/// (a CSV the person exported, maybe still open in a spreadsheet).
pub(crate) fn read_shared(file: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    open_shared(file)?.read_to_end(&mut out)?;
    Ok(out)
}

#[cfg(windows)]
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    let mut source = open_shared(from)?;
    let mut target = std::fs::File::create(to)?;
    io::copy(&mut source, &mut target).map(drop)
}

#[cfg(not(windows))]
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::copy(from, to).map(drop)
}

#[cfg(windows)]
fn open_shared(file: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    // winnt.h: FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_SHARE_DELETE.
    const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_ALL)
        .open(file)
}

#[cfg(not(windows))]
fn open_shared(file: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(file)
}

/// Whether an open or read failed because another program holds the file:
/// `ERROR_SHARING_VIOLATION` (32) or `ERROR_LOCK_VIOLATION` (33) on Windows.
/// Linux has no mandatory locks, so never there.
pub(crate) fn is_in_use(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33))
}

/// The error a failed read of `file` (described as `what`) becomes.
pub(crate) fn read_error(what: impl Into<String>, e: &io::Error) -> ImportError {
    if is_in_use(e) {
        ImportError::InUse(what.into())
    } else {
        ImportError::Unreadable(what.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("import-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("History");
        std::fs::write(&src, b"main").unwrap();
        std::fs::write(with_suffix(&src, "-wal"), b"wal").unwrap();
        let copy = Copied::of(&src).unwrap();
        let path = copy.path.clone();
        assert_eq!(std::fs::read(&path).unwrap(), b"main");
        assert_eq!(std::fs::read(with_suffix(&path, "-wal")).unwrap(), b"wal");
        drop(copy);
        assert!(!path.exists() && !with_suffix(&path, "-wal").exists());
        assert!(Copied::of(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_use_only_for_sharing_errors() {
        let sharing = io::Error::from_raw_os_error(32);
        assert_eq!(is_in_use(&sharing), cfg!(windows));
        assert!(matches!(
            read_error("x", &io::Error::from(io::ErrorKind::NotFound)),
            ImportError::Unreadable(_)
        ));
        #[cfg(windows)]
        assert!(matches!(read_error("x", &sharing), ImportError::InUse(_)));
    }
}
