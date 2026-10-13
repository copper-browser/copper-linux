//! One canvas's Yjs history on disk (Mac: `CanvasStore`): `updates.log` —
//! each update as a 4-byte big-endian length and its bytes, in the order the
//! page made them or the room handed them over — and, once that grows past
//! 2 MB or 500 entries, `snapshot.bin` (the whole document as one update,
//! from the page's `exportState()`) with the log cut back to what came after
//! it. Opening a canvas is the snapshot, then every logged update on top:
//! applying an update twice is harmless in Yjs, so the cut only ever errs on
//! the side of keeping too much.
//!
//! Folder: `<data>/canvas/<key>/`, 0700; files 0600. Writes are appended
//! synchronously (small, and never lost to a crash).

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const MAX_ENTRIES: usize = 500;
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

thread_local! {
    static OPEN: RefCell<HashMap<String, Rc<CanvasStore>>> = RefCell::new(HashMap::new());
}

pub struct CanvasStore {
    pub key: String,
    folder: PathBuf,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    entries: usize,
    bytes: usize,
    counted: bool,
    remote_seen: Vec<u64>,
    /// Changes made here while the room isn't confirmed.
    pending: usize,
}

/// A short-lived fingerprint of an update.
pub fn fingerprint(data: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    data.len().hash(&mut h);
    data.hash(&mut h);
    h.finish()
}

fn safe_key(key: &str) -> String {
    key.replace('/', "_").replace("..", "_")
}

impl CanvasStore {
    /// The store for `key` under `root` (`<data>/canvas`).
    pub fn open(root: &Path, key: &str) -> Rc<CanvasStore> {
        OPEN.with(|o| {
            o.borrow_mut()
                .entry(key.to_string())
                .or_insert_with(|| {
                    Rc::new(CanvasStore {
                        key: key.to_string(),
                        folder: root.join(safe_key(key)),
                        state: RefCell::default(),
                    })
                })
                .clone()
        })
    }

    fn log(&self) -> PathBuf {
        self.folder.join("updates.log")
    }

    fn snapshot(&self) -> PathBuf {
        self.folder.join("snapshot.bin")
    }

    fn ensure_folder(&self) {
        if fs::create_dir_all(&self.folder).is_ok() {
            let _ = fs::set_permissions(&self.folder, fs::Permissions::from_mode(0o700));
        }
    }

    /// The snapshot (if any) and the updates logged after it.
    pub fn load(&self) -> (Option<Vec<u8>>, Vec<Vec<u8>>) {
        self.ensure_folder();
        let base = fs::read(self.snapshot()).ok().filter(|b| !b.is_empty());
        let updates = read_log(&self.log());
        let mut s = self.state.borrow_mut();
        s.entries = updates.len();
        s.bytes = updates.iter().map(|u| u.len() + 4).sum();
        s.counted = true;
        (base, updates)
    }

    /// Keep one update.
    pub fn append(&self, update: &[u8]) {
        if update.is_empty() {
            return;
        }
        if !self.state.borrow().counted {
            let _ = self.load();
        }
        self.ensure_folder();
        let mut record = Vec::with_capacity(update.len() + 4);
        record.extend_from_slice(&(update.len() as u32).to_be_bytes());
        record.extend_from_slice(update);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.log());
        match file.and_then(|mut f| f.write_all(&record)) {
            Ok(()) => {
                let mut s = self.state.borrow_mut();
                s.entries += 1;
                s.bytes += record.len();
            }
            Err(e) => tracing::error!(error = %e, key = %self.key, "canvas log"),
        }
    }

    /// An update from the room: kept once, even if the room sends it twice.
    pub fn append_remote(&self, update: &[u8]) {
        let hash = fingerprint(update);
        {
            let mut s = self.state.borrow_mut();
            if s.remote_seen.contains(&hash) {
                return;
            }
            s.remote_seen.push(hash);
            let n = s.remote_seen.len();
            if n > 128 {
                s.remote_seen.drain(..n - 128);
            }
        }
        self.append(update);
    }

    pub fn entries(&self) -> usize {
        self.state.borrow().entries
    }

    pub fn bytes(&self) -> usize {
        self.state.borrow().bytes
    }

    pub fn pending(&self) -> usize {
        self.state.borrow().pending
    }

    pub fn set_pending(&self, n: usize) {
        self.state.borrow_mut().pending = n;
    }

    pub fn wants_compaction(&self) -> bool {
        let s = self.state.borrow();
        s.entries > MAX_ENTRIES || s.bytes > MAX_BYTES
    }

    /// `state` becomes the snapshot; the first `dropping` log entries go.
    pub fn compact(&self, state: &[u8], dropping: usize) {
        if state.is_empty() {
            return;
        }
        self.ensure_folder();
        if let Err(e) = copper_core::store::write_atomic(&self.snapshot(), state, true, false) {
            tracing::error!(error = %e, key = %self.key, "canvas snapshot");
            return;
        }
        let left: Vec<Vec<u8>> = read_log(&self.log()).into_iter().skip(dropping).collect();
        let mut bytes = Vec::new();
        for u in &left {
            bytes.extend_from_slice(&(u.len() as u32).to_be_bytes());
            bytes.extend_from_slice(u);
        }
        if let Err(e) = copper_core::store::write_atomic(&self.log(), &bytes, true, false) {
            tracing::error!(error = %e, key = %self.key, "canvas log rewrite");
        }
        let mut s = self.state.borrow_mut();
        s.entries = left.len();
        s.bytes = bytes.len();
    }

    /// Everything on disk for this canvas goes.
    pub fn erase(&self) {
        let _ = fs::remove_dir_all(&self.folder);
        let mut s = self.state.borrow_mut();
        s.entries = 0;
        s.bytes = 0;
        OPEN.with(|o| o.borrow_mut().remove(&self.key));
    }

    pub fn describe(&self) -> serde_json::Value {
        let snap = fs::metadata(self.snapshot()).map(|m| m.len()).unwrap_or(0);
        let s = self.state.borrow();
        serde_json::json!({"key": self.key, "entries": s.entries, "bytes": s.bytes, "pending": s.pending, "snapshot": snap})
    }
}

/// Every whole record in a log; a torn tail is cut off the file.
fn read_log(path: &Path) -> Vec<Vec<u8>> {
    let Ok(data) = fs::read(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 4 <= data.len() {
        let len = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        if len == 0 || at + 4 + len > data.len() {
            break;
        }
        out.push(data[at + 4..at + 4 + len].to_vec());
        at += 4 + len;
    }
    if at < data.len() {
        let _ = copper_core::store::write_atomic(path, &data[..at], true, false);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_round_trip_compact_and_torn_tail() {
        let root = std::env::temp_dir().join(format!("copper-canvas-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let s = CanvasStore::open(&root, "k1");
        s.append(b"one");
        s.append(b"two");
        s.append_remote(b"three");
        s.append_remote(b"three");
        let (snap, ups) = s.load();
        assert!(snap.is_none());
        assert_eq!(
            ups,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
        s.compact(b"STATE", 2);
        let (snap, ups) = s.load();
        assert_eq!(snap.as_deref(), Some(&b"STATE"[..]));
        assert_eq!(ups, vec![b"three".to_vec()]);
        // A torn tail is dropped.
        let mut f = OpenOptions::new()
            .append(true)
            .open(root.join("k1/updates.log"))
            .unwrap();
        f.write_all(&[0, 0, 0, 9, 1, 2]).unwrap();
        let (_, ups) = s.load();
        assert_eq!(ups.len(), 1);
        s.erase();
        assert!(!root.join("k1").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
