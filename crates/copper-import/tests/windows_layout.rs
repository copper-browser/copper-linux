//! The Windows layout, on any host: a `%LOCALAPPDATA%`-shaped tree with
//! Chrome and an Arc package install, found by [`detect_in`] and read through
//! the profile and bookmark importers. Passwords on this layout come over
//! through the browser's own CSV export, never its profile.

use std::path::{Path, PathBuf};

use copper_import::passwords_csv::{self, PasswordsVia};
use copper_import::{BrowserKind, Layout, bookmarks, detect_in};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/flow-chrome")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A user-data root: the fixture's profiles, each with a `Preferences` so
/// it counts as a profile.
fn user_data(root: &Path) {
    copy_tree(&fixture(), root);
    for profile in ["Default", "Profile 1", "Profile 2"] {
        let prefs = root.join(profile).join("Preferences");
        if !prefs.exists() {
            std::fs::write(prefs, format!(r#"{{"profile":{{"name":"{profile}"}}}}"#)).unwrap();
        }
    }
}

#[test]
fn finds_and_reads_chrome_and_arc_on_the_windows_layout() {
    let base = std::env::temp_dir().join(format!("import-win-layout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    user_data(&base.join("Google").join("Chrome").join("User Data"));
    let arc = base
        .join("Packages")
        .join("TheBrowserCompany.Arc_ttt1ap7aakyb4")
        .join("LocalCache")
        .join("Local")
        .join("Arc")
        .join("User Data");
    user_data(&arc);
    // An empty Edge install: no profile, so not offered.
    std::fs::create_dir_all(base.join("Microsoft").join("Edge").join("User Data")).unwrap();

    let found = detect_in(Layout::Windows, &base);
    let names: Vec<&str> = found.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["Chrome", "Arc"]);
    assert_eq!(found[1].kind, BrowserKind::Arc);
    assert_eq!(found[1].root, arc);

    for browser in &found {
        assert_eq!(browser.profiles(), ["Default", "Profile 1", "Profile 2"]);
        let read = bookmarks::read(&browser.profile_dirs());
        assert!(read.count() > 0, "{} bookmarks", browser.name);
        let hint = passwords_csv::export_hint(browser.kind);
        assert_eq!(hint.page, "chrome://password-manager/settings");
        match passwords_csv::passwords_via(browser) {
            PasswordsVia::Profile => assert_eq!(std::env::consts::OS, "linux"),
            PasswordsVia::Export(h) => assert_eq!(h, hint),
        }
    }
    let _ = std::fs::remove_dir_all(&base);
}
