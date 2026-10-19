//! Served bundles (contract §3): one JSON document,
//! `{"format":"copper-addon-bundle/1","manifest":{…},"files":[{path,size,sha256,data}]}`,
//! whose SHA-256 is the version's identity.
//!
//! [`verify_bundle`] checks, in order: the byte size, the SHA-256 of the exact
//! bytes, the format string, the manifest (valid, and the same `id` /
//! `version` / `slot` as the `/v1/addons` entry), then every file (path rule,
//! size, its own SHA-256, limits). Any failure discards the bundle.
//! [`Bundle::write_to`] unpacks into a directory atomically.

use super::manifest::{Manifest, ManifestError};
use super::resolved::ResolvedAddon;
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The served format string.
pub const FORMAT: &str = "copper-addon-bundle/1";
/// At most this many files.
pub const MAX_FILES: usize = 1000;
/// At most this many decoded bytes over all files.
pub const MAX_DECODED_BYTES: u64 = 10 * 1024 * 1024;
/// At most this many served (JSON) bytes.
pub const MAX_SERVED_BYTES: usize = 14 * 1024 * 1024;
/// Longest bundle path.
pub const MAX_PATH: usize = 200;

/// Why a bundle was refused or could not be written.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("the bundle is {size} bytes; the limit is {limit}")]
    TooLarge { size: u64, limit: u64 },
    #[error("the bundle's SHA-256 is {actual}, expected {expected}")]
    Sha256Mismatch { expected: String, actual: String },
    #[error("the bundle doesn't read: {0}")]
    Json(String),
    #[error("unknown bundle format {0:?}")]
    Format(String),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("the bundle's {field} is {actual:?}, expected {expected:?}")]
    Mismatch {
        field: &'static str,
        expected: String,
        actual: String,
    },
    #[error("bad file path {0:?}")]
    Path(String),
    #[error("{path}: {why}")]
    File { path: String, why: String },
    #[error("the bundle has {0} files; the limit is 1000")]
    TooManyFiles(usize),
    #[error("the entry {0:?} is not in the bundle")]
    MissingEntry(String),
    #[error("{} already holds a different bundle", .0.display())]
    ExistsDifferent(PathBuf),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl BundleError {
    /// A short code for logs and status lines.
    pub fn code(&self) -> &'static str {
        match self {
            BundleError::TooLarge { .. } => "too_large",
            BundleError::Sha256Mismatch { .. } => "sha256",
            BundleError::Json(_) => "json",
            BundleError::Format(_) => "format",
            BundleError::Manifest(_) => "manifest",
            BundleError::Mismatch { .. } => "mismatch",
            BundleError::Path(_) => "path",
            BundleError::File { .. } => "file",
            BundleError::TooManyFiles(_) => "too_many_files",
            BundleError::MissingEntry(_) => "entry",
            BundleError::ExistsDifferent(_) => "exists",
            BundleError::Io { .. } => "io",
        }
    }

    fn io(path: &Path, source: std::io::Error) -> Self {
        BundleError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// 64 lowercase hex digits.
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// The bundle path rule: `^[A-Za-z0-9._@-]+(/[A-Za-z0-9._@-]+)*$`, no `.` or
/// `..` segment, at most 200 characters.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'@' | b'-'))
        })
}

/// One file of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleFile {
    pub path: String,
    pub sha256: String,
    pub data: Vec<u8>,
}

/// A verified bundle.
#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    /// SHA-256 of the served bytes.
    pub sha256: String,
    pub manifest: Manifest,
    /// Sorted by path.
    pub files: Vec<BundleFile>,
}

/// Verify served bytes against a `/v1/addons` entry (see the module docs).
pub fn verify_bundle(
    bytes: &[u8],
    expected_sha: &str,
    expected: &ResolvedAddon,
) -> Result<Bundle, BundleError> {
    verify_bundle_for(
        bytes,
        expected_sha,
        &expected.id,
        &expected.version,
        &expected.slot,
    )
}

/// [`verify_bundle`] against bare `id` / `version` / `slot` (a staged or
/// installed entry).
pub fn verify_bundle_for(
    bytes: &[u8],
    expected_sha: &str,
    id: &str,
    version: &str,
    slot: &str,
) -> Result<Bundle, BundleError> {
    check_size(bytes)?;
    let actual = sha256_hex(bytes);
    if !actual.eq_ignore_ascii_case(expected_sha.trim()) {
        return Err(BundleError::Sha256Mismatch {
            expected: expected_sha.to_string(),
            actual,
        });
    }
    let bundle = parse_checked(bytes, actual)?;
    for (field, want, got) in [
        ("id", id, bundle.manifest.id.as_str()),
        ("version", version, bundle.manifest.version.as_str()),
        ("slot", slot, bundle.manifest.slot.as_str()),
    ] {
        if want != got {
            return Err(BundleError::Mismatch {
                field,
                expected: want.to_string(),
                actual: got.to_string(),
            });
        }
    }
    Ok(bundle)
}

/// Parse and check served bytes with no expectation about which addon they
/// are (a bundle file opened from disk). Its SHA-256 is computed.
pub fn parse_bundle(bytes: &[u8]) -> Result<Bundle, BundleError> {
    check_size(bytes)?;
    parse_checked(bytes, sha256_hex(bytes))
}

fn check_size(bytes: &[u8]) -> Result<(), BundleError> {
    if bytes.len() > MAX_SERVED_BYTES {
        return Err(BundleError::TooLarge {
            size: bytes.len() as u64,
            limit: MAX_SERVED_BYTES as u64,
        });
    }
    Ok(())
}

fn parse_checked(bytes: &[u8], sha256: String) -> Result<Bundle, BundleError> {
    let doc: Value = serde_json::from_slice(bytes).map_err(|e| BundleError::Json(e.to_string()))?;
    let Some(doc) = doc.as_object() else {
        return Err(BundleError::Json("not a JSON object".into()));
    };
    match doc.get("format").and_then(Value::as_str) {
        Some(FORMAT) => {}
        other => return Err(BundleError::Format(other.unwrap_or("").to_string())),
    }
    let manifest = Manifest::from_value(doc.get("manifest").cloned().unwrap_or(Value::Null))?;
    manifest.validate()?;
    let Some(entries) = doc.get("files").and_then(Value::as_array) else {
        return Err(BundleError::Json("no files array".into()));
    };
    if entries.len() > MAX_FILES {
        return Err(BundleError::TooManyFiles(entries.len()));
    }
    let mut files = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    let mut total: u64 = 0;
    for entry in entries {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| BundleError::Json("a file has no path".into()))?;
        if !valid_path(path) {
            return Err(BundleError::Path(path.to_string()));
        }
        // Case-insensitive file systems (Mac, Windows) would merge these.
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err(BundleError::File {
                path: path.into(),
                why: "listed twice".into(),
            });
        }
        let bad = |why: &str| BundleError::File {
            path: path.into(),
            why: why.into(),
        };
        let data = entry
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("no data"))?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|_| bad("data is not base64"))?;
        if let Some(size) = entry.get("size")
            && size.as_u64() != Some(data.len() as u64)
        {
            return Err(bad("size doesn't match the data"));
        }
        total += data.len() as u64;
        if total > MAX_DECODED_BYTES {
            return Err(BundleError::TooLarge {
                size: total,
                limit: MAX_DECODED_BYTES,
            });
        }
        let want = entry
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("no sha256"))?;
        let got = sha256_hex(&data);
        if !got.eq_ignore_ascii_case(want) {
            return Err(bad("sha256 doesn't match the data"));
        }
        files.push(BundleFile {
            path: path.to_string(),
            sha256: got,
            data,
        });
    }
    // A file may not also be a folder of another (`a` and `a/b`).
    for f in &files {
        let mut prefix = f.path.to_ascii_lowercase();
        prefix.push('/');
        if seen
            .range(prefix.clone()..)
            .next()
            .is_some_and(|p| p.starts_with(&prefix))
        {
            return Err(BundleError::File {
                path: f.path.clone(),
                why: "is both a file and a folder".into(),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    if !files.iter().any(|f| f.path == manifest.entry) {
        return Err(BundleError::MissingEntry(manifest.entry.clone()));
    }
    Ok(Bundle {
        sha256,
        manifest,
        files,
    })
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl Bundle {
    /// The file at `path`.
    pub fn file(&self, path: &str) -> Option<&BundleFile> {
        self.files
            .binary_search_by(|f| f.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.files[i])
    }

    /// Unpack into `dir` atomically: every file goes into a temporary sibling
    /// folder (fsynced where the platform allows), which is then renamed to
    /// `dir`. A `dir` that already holds exactly these files is left alone
    /// (idempotent); one that holds anything else is refused
    /// ([`BundleError::ExistsDifferent`]) and never overwritten.
    pub fn write_to(&self, dir: &Path) -> Result<(), BundleError> {
        if dir.exists() {
            return if self.matches_dir(dir) {
                Ok(())
            } else {
                Err(BundleError::ExistsDifferent(dir.to_path_buf()))
            };
        }
        let parent = dir.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent).map_err(|e| BundleError::io(parent, e))?;
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = parent.join(format!(
            ".{name}.tmp-{}-{}",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let result = self.write_tree(&tmp).and_then(|()| {
            sync_dir(parent);
            fs::rename(&tmp, dir).map_err(|e| BundleError::io(dir, e))
        });
        match result {
            Ok(()) => {
                sync_dir(parent);
                Ok(())
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&tmp);
                // Someone else unpacked the same bundle first.
                if dir.exists() && self.matches_dir(dir) {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn write_tree(&self, root: &Path) -> Result<(), BundleError> {
        fs::create_dir(root).map_err(|e| BundleError::io(root, e))?;
        let mut dirs = BTreeSet::new();
        for f in &self.files {
            let path = root.join(&f.path);
            if let Some(parent) = path.parent()
                && parent != root
            {
                fs::create_dir_all(parent).map_err(|e| BundleError::io(parent, e))?;
                dirs.insert(parent.to_path_buf());
            }
            write_synced(&path, &f.data).map_err(|e| BundleError::io(&path, e))?;
        }
        for d in dirs.iter().rev() {
            sync_dir(d);
        }
        sync_dir(root);
        Ok(())
    }

    /// Whether `dir` holds exactly this bundle's files (no more, no fewer,
    /// same bytes).
    pub fn matches_dir(&self, dir: &Path) -> bool {
        let mut found = Vec::new();
        if list_files(dir, "", &mut found).is_err() {
            return false;
        }
        found.sort();
        found.len() == self.files.len()
            && self.files.iter().zip(&found).all(|(f, p)| {
                f.path == *p
                    && fs::read(dir.join(&f.path)).is_ok_and(|b| sha256_hex(&b) == f.sha256)
            })
    }
}

fn list_files(dir: &Path, prefix: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type()?;
        if kind.is_dir() {
            list_files(&entry.path(), &rel, out)?;
        } else {
            // Files and anything else (a symlink counts against a match).
            out.push(rel);
        }
    }
    Ok(())
}

fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    let _ = file.sync_all();
    Ok(())
}

/// fsync a folder where the platform allows it (not Windows).
fn sync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::testkit;
    use serde_json::json;

    fn good() -> (Vec<u8>, String, ResolvedAddon) {
        let manifest = testkit::manifest_json("example.sidebar", "1.0.0");
        let bytes = testkit::served_bundle(
            &manifest,
            &[
                ("index.html", b"<!doctype html><title>x</title>".as_slice()),
                ("js/app.js", b"console.log(1)"),
            ],
        );
        let sha = sha256_hex(&bytes);
        let resolved = testkit::resolved_for(&manifest, &bytes);
        (bytes, sha, resolved)
    }

    #[test]
    fn verifies_a_good_bundle() {
        let (bytes, sha, resolved) = good();
        let b = verify_bundle(&bytes, &sha, &resolved).unwrap();
        assert_eq!(b.sha256, sha);
        assert_eq!(b.files.len(), 2);
        assert_eq!(b.file("js/app.js").unwrap().data, b"console.log(1)");
        assert!(b.file("nope").is_none());
        assert_eq!(parse_bundle(&bytes).unwrap().sha256, sha);
    }

    #[test]
    fn refuses_a_wrong_sha() {
        let (bytes, _, resolved) = good();
        let e = verify_bundle(&bytes, &"0".repeat(64), &resolved).unwrap_err();
        assert_eq!(e.code(), "sha256");
    }

    #[test]
    fn refuses_size_over_cap() {
        let (_, _, resolved) = good();
        let big = vec![b' '; MAX_SERVED_BYTES + 1];
        let e = verify_bundle(&big, &sha256_hex(&big), &resolved).unwrap_err();
        assert_eq!(e.code(), "too_large");
    }

    fn tampered(edit: impl FnOnce(&mut Value)) -> BundleError {
        let (bytes, _, resolved) = good();
        let mut doc: Value = serde_json::from_slice(&bytes).unwrap();
        edit(&mut doc);
        let bytes = serde_json::to_vec(&doc).unwrap();
        verify_bundle(&bytes, &sha256_hex(&bytes), &resolved).unwrap_err()
    }

    #[test]
    fn refuses_bad_contents() {
        assert_eq!(tampered(|d| d["format"] = json!("zip")).code(), "format");
        assert_eq!(
            tampered(|d| d["files"][0]["path"] = json!("../index.html")).code(),
            "path"
        );
        assert_eq!(
            tampered(|d| d["files"][0]["path"] = json!("a/./b.html")).code(),
            "path"
        );
        assert_eq!(
            tampered(|d| d["files"][1]["sha256"] = json!("0".repeat(64))).code(),
            "file"
        );
        assert_eq!(
            tampered(|d| d["files"][1]["size"] = json!(3)).code(),
            "file"
        );
        assert_eq!(
            tampered(|d| d["files"][1]["path"] = json!("INDEX.html")).code(),
            "file",
            "case-insensitive duplicate"
        );
        assert_eq!(
            tampered(|d| d["manifest"]["version"] = json!("2.0.0")).code(),
            "mismatch"
        );
        assert_eq!(
            tampered(|d| d["manifest"]["id"] = json!("other.addon")).code(),
            "mismatch"
        );
        assert_eq!(
            tampered(|d| d["manifest"]["schema"] = json!("x")).code(),
            "manifest"
        );
        assert_eq!(
            tampered(|d| d["manifest"]["entry"] = json!("missing.html")).code(),
            "entry"
        );
        assert_eq!(
            tampered(|d| {
                let f = d["files"][1].clone();
                let mut g = f.clone();
                g["path"] = json!("js/app.js/inner.js");
                d["files"].as_array_mut().unwrap().push(g);
            })
            .code(),
            "file"
        );
    }

    #[test]
    fn write_to_is_atomic_and_idempotent() {
        let (bytes, sha, resolved) = good();
        let b = verify_bundle(&bytes, &sha, &resolved).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp
            .path()
            .join("bundles")
            .join("example.sidebar")
            .join(&sha);
        b.write_to(&dir).unwrap();
        assert_eq!(
            fs::read(dir.join("js/app.js")).unwrap(),
            b"console.log(1)".to_vec()
        );
        assert!(b.matches_dir(&dir));
        // No temporary left beside it.
        let siblings: Vec<_> = fs::read_dir(dir.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings.len(), 1, "{siblings:?}");
        // Again: fine, nothing changes.
        b.write_to(&dir).unwrap();
        // A different tree there is refused, never overwritten.
        fs::write(dir.join("extra.txt"), b"x").unwrap();
        assert_eq!(b.write_to(&dir).unwrap_err().code(), "exists");
        assert!(dir.join("extra.txt").exists());
        fs::remove_file(dir.join("extra.txt")).unwrap();
        fs::write(dir.join("index.html"), b"changed").unwrap();
        assert_eq!(b.write_to(&dir).unwrap_err().code(), "exists");
    }

    #[test]
    fn paths() {
        for ok in ["index.html", "a/b/c.js", "@x/y-z_1.css", ".well-known/x"] {
            assert!(valid_path(ok), "{ok}");
        }
        for bad in [
            "", "/a", "a/", "a//b", "./a", "a/../b", "..", "a b", "a\\b", "é.html", "__x/../y",
        ] {
            assert!(!valid_path(bad), "{bad}");
        }
        assert!(!valid_path(&"a".repeat(201)));
    }
}
