//! The profiles inside one user-data root: which folders hold one, in
//! Chrome's order, and what the person named each (`Preferences`, then
//! `Local State`'s `profile.info_cache`). The same on Linux and Windows.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::copy::copy_read;

/// The profile folders (children with a Preferences file) under `root`, in
/// Chrome's order: Default first, then Profile 1, 2 … 10 compared as
/// numbers. Guest and System profiles hold nothing to move.
pub fn profiles(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "Guest Profile" && n != "System Profile")
        .filter(|n| root.join(n).join("Preferences").is_file())
        .collect();
    names.sort_by(|a, b| match (a == "Default", b == "Default") {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => natural(a, b),
    });
    names
}

/// The name a person gave a profile: `Preferences` → `profile.name`, else
/// the root's `Local State` → `profile.info_cache.<folder>.name` (newer
/// builds keep it only there).
pub fn profile_name(profile_dir: &Path) -> Option<String> {
    preferences_name(profile_dir).or_else(|| {
        let folder = profile_dir.file_name()?.to_str()?;
        let root = profile_dir.parent()?;
        info_cache(root)?.get(folder).and_then(|e| text(e, "name"))
    })
}

fn preferences_name(profile_dir: &Path) -> Option<String> {
    let data = copy_read(&profile_dir.join("Preferences")).ok()?;
    let v: Value = serde_json::from_slice(&data).ok()?;
    text(v.get("profile")?, "name")
}

/// What the profile picker shows for one profile: its name and picture.
/// Nothing else from `Local State` (no account e-mail, no sync state) is
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileInfo {
    /// The folder under the root ("Default", "Profile 1").
    pub folder: String,
    /// What the person named it, when the browser recorded a name.
    pub name: Option<String>,
    /// The built-in avatar Chromium shows for it
    /// (`chrome://theme/IDR_PROFILE_AVATAR_26`).
    pub avatar_icon: Option<String>,
    /// The account picture the browser saved in the profile folder
    /// (`Google Profile Picture.png`, `Edge Profile Picture.png`), when the
    /// file is there.
    pub picture: Option<PathBuf>,
}

/// Every profile under `root` (as [`profiles`] lists and orders them) with
/// its name and avatar.
pub fn profile_infos(root: &Path) -> Vec<ProfileInfo> {
    let cache = info_cache(root);
    profiles(root)
        .into_iter()
        .map(|folder| {
            let entry = cache.as_ref().and_then(|c| c.get(&folder));
            let dir = root.join(&folder);
            let picture = entry
                .and_then(|e| text(e, "gaia_picture_file_name"))
                .filter(|f| is_plain_file_name(f))
                .map(|f| dir.join(f))
                .filter(|p| p.is_file());
            ProfileInfo {
                name: preferences_name(&dir).or_else(|| entry.and_then(|e| text(e, "name"))),
                avatar_icon: entry.and_then(|e| text(e, "avatar_icon")),
                picture,
                folder,
            }
        })
        .collect()
}

/// `Local State` → `profile.info_cache`, read from a copy.
fn info_cache(root: &Path) -> Option<serde_json::Map<String, Value>> {
    let data = copy_read(&root.join("Local State")).ok()?;
    let mut v: Value = serde_json::from_slice(&data).ok()?;
    match v.get_mut("profile")?.get_mut("info_cache")?.take() {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// A trimmed, non-empty string field.
fn text(v: &Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// A bare file name: nothing in `Local State` may point outside the
/// profile folder.
fn is_plain_file_name(name: &str) -> bool {
    !name.contains(['/', '\\', ':']) && name != "." && name != ".."
}

/// Numbers compared as numbers, the rest without case
/// (`localizedStandardCompare` for the names Chrome uses).
pub fn natural(a: &str, b: &str) -> Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, text)) if *d == digit => text.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (x, y) = (chunks(a), chunks(b));
    for (l, r) in x.iter().zip(&y) {
        let ord = match (l.0, r.0) {
            (true, true) => {
                let (lt, rt) = (l.1.trim_start_matches('0'), r.1.trim_start_matches('0'));
                lt.len().cmp(&rt.len()).then_with(|| lt.cmp(rt))
            }
            _ => l.1.to_lowercase().cmp(&r.1.to_lowercase()),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    x.len().cmp(&y.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["Profile 10", "Profile 2", "Profile 1", "profile 3"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["Profile 1", "Profile 2", "profile 3", "Profile 10"]);
    }

    #[test]
    fn profile_order() {
        let root = std::env::temp_dir().join(format!("import-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in [
            "Profile 10",
            "Profile 2",
            "Default",
            "Profile 1",
            "Guest Profile",
            "System Profile",
            "Crashpad",
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            if name != "Crashpad" {
                std::fs::write(root.join(name).join("Preferences"), "{}").unwrap();
            }
        }
        assert_eq!(
            profiles(&root),
            ["Default", "Profile 1", "Profile 2", "Profile 10"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn picture_names_stay_in_the_folder() {
        assert!(is_plain_file_name("Google Profile Picture.png"));
        for bad in ["..", "../x.png", "..\\x.png", "C:x.png", "/etc/passwd"] {
            assert!(!is_plain_file_name(bad), "{bad}");
        }
    }
}
