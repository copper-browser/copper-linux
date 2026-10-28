//! `calendar.json`: the calendars' secret iCal addresses.
//!
//! ```json
//! { "feeds": [ { "url": "https://calendar.google.com/calendar/ical/…/private-…/basic.ics",
//!                "label": "Exowatt", "color": "#039be5" } ] }
//! ```
//!
//! It lives in Copper's data folder next to `cloud.json`
//! (`~/.local/share/copper/calendar.json`; `copper-<world>` with
//! `COPPER_WORLD`; `%LOCALAPPDATA%\Copper\Data\calendar.json` on Windows),
//! written atomically and 0600 on Unix through
//! [`copper_core::store::write_atomic`]. An address is a password to the
//! calendar: no `.bak` is kept (a removed one is gone for good), it is
//! never logged, and Copper Cloud never carries it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file's name in the data folder.
pub const FILE_NAME: &str = "calendar.json";

/// `<data>/calendar.json`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// One calendar.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    /// The secret iCal address. Never logged.
    pub url: String,
    /// What Settings and an error call it; empty: the calendar's own name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Its meetings' dot, `#rrggbb`; none: one of [`COLORS`] by its place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl std::fmt::Debug for Feed {
    // Never the address.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feed")
            .field("label", &self.label)
            .field("owner", &owner(&self.url))
            .field("color", &self.color)
            .finish_non_exhaustive()
    }
}

/// The dots of calendars with no colour of their own, in turn (Google
/// Calendar's peacock, basil, tangerine, grape, banana, flamingo).
pub const COLORS: [&str; 6] = [
    "#039be5", "#0b8043", "#f4511e", "#8e24aa", "#f6bf26", "#e67c73",
];

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    feeds: Vec<Feed>,
}

/// The calendars in `data_dir` (none when there is no file).
pub fn load(data_dir: &Path) -> Result<Vec<Feed>, String> {
    let p = path(data_dir);
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", p.display())),
    };
    let file: File =
        serde_json::from_slice(&bytes).map_err(|e| format!("{} doesn't read: {e}", p.display()))?;
    Ok(file.feeds)
}

/// Keep `feeds` in `data_dir` (atomic, 0600 on Unix).
pub fn save(data_dir: &Path, feeds: &[Feed]) -> Result<(), String> {
    let p = path(data_dir);
    let file = File {
        feeds: feeds.to_vec(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    copper_core::store::write_atomic(&p, &bytes, true, false).map_err(|e| e.to_string())
}

/// A pasted address, checked: `https://`, or plain `http://` to this
/// computer only (a test server). Trimmed; the error is in words.
pub fn address(text: &str) -> Result<String, String> {
    let text = text.trim();
    let url = url::Url::parse(text)
        .map_err(|_| "That isn't an address — paste the whole https:// address".to_string())?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => Ok(text.to_string()),
        "http" if local => Ok(text.to_string()),
        _ => Err("The address must start with https://".to_string()),
    }
}

/// Whose calendar an address reads, when its id is an email (a person's
/// own calendar: `…/ical/collin%40exowatt.com/private-…`): the attendee
/// whose answer counts.
pub fn owner(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let mut segments = url.path_segments()?;
    segments.find(|s| *s == "ical")?;
    let id = percent_encoding::percent_decode_str(segments.next()?)
        .decode_utf8()
        .ok()?
        .to_lowercase();
    id.contains('@').then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "https://calendar.google.com/calendar/ical/collin%40exowatt.com/private-SECRETTOKEN/basic.ics";

    fn sample() -> Vec<Feed> {
        vec![
            Feed {
                url: SECRET.into(),
                label: "Exowatt".into(),
                color: Some("#039be5".into()),
            },
            Feed {
                url: "https://calendar.google.com/calendar/ical/c_1%40group.calendar.google.com/private-x/basic.ics".into(),
                label: String::new(),
                color: None,
            },
        ]
    }

    #[test]
    fn saves_private_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), Vec::new());
        save(dir.path(), &sample()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), sample());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(dir.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Saving again leaves no backup holding an old address.
        save(dir.path(), &sample()[..1]).unwrap();
        assert!(!copper_core::store::backup_path(&path(dir.path())).exists());
        // The file is the shape Settings and hand edits share.
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["feeds"][0]["label"], "Exowatt");
        assert_eq!(v["feeds"][0]["color"], "#039be5");
    }

    #[test]
    fn a_broken_file_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), b"{not json").unwrap();
        assert!(load(dir.path()).is_err());
    }

    #[test]
    fn debug_never_shows_the_address() {
        let text = format!("{:?}", sample());
        assert!(!text.contains("SECRET"), "{text}");
        assert!(text.contains("collin@exowatt.com"), "{text}");
    }

    #[test]
    fn addresses_are_https_or_local() {
        assert_eq!(address(&format!("  {SECRET}\n")).unwrap(), SECRET);
        assert!(address("http://127.0.0.1:8080/test.ics").is_ok());
        assert!(address("http://calendar.google.com/calendar/ical/x/basic.ics").is_err());
        assert!(address("webcal://example.com/a.ics").is_err());
        assert!(address("calendar.google.com").is_err());
    }

    #[test]
    fn the_owner_is_the_calendars_email() {
        assert_eq!(owner(SECRET).as_deref(), Some("collin@exowatt.com"));
        assert_eq!(
            owner(
                "https://calendar.google.com/calendar/ical/Collin%40Exowatt.com/public/basic.ics"
            )
            .as_deref(),
            Some("collin@exowatt.com")
        );
        assert_eq!(owner("http://127.0.0.1:8080/test.ics"), None);
    }
}
