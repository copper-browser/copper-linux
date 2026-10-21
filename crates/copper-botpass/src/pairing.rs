//! `botpass.json`: where BotPass is and the device token Copper acts with.
//!
//! It lives in Copper's data folder next to `cloud.json`
//! (`~/.local/share/copper/botpass.json`; `copper-<world>` with
//! `COPPER_WORLD`; `%LOCALAPPDATA%\Copper\Data\botpass.json` on Windows),
//! written atomically and 0600 on Unix through
//! [`copper_core::store::write_atomic`]. No `.bak` is kept: an old token has
//! no business outliving an unpair.

use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The file's name in the data folder.
pub const FILE_NAME: &str = "botpass.json";

/// `<data>/botpass.json`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// One pairing with a BotPass server.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Pairing {
    /// `https://host[:port]` (plain `http://` only for loopback).
    pub url: String,
    /// SHA-256 of the server's leaf certificate (DER), lowercase hex; `None`
    /// = the public authorities (or a loopback `http://` server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// The device token (`bpd_…`). Never logged, never printed.
    pub token: String,
    /// What BotPass calls this Copper.
    pub device_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_id: String,
    /// The bot this device acts for (none for a personal pairing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    /// A personal pairing acts for the owner, not a bot.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub personal: bool,
    /// When it was paired, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paired_at: Option<u64>,
    /// Keys a newer build wrote.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl std::fmt::Debug for Pairing {
    // Never the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pairing")
            .field("url", &self.url)
            .field("fingerprint", &self.fingerprint)
            .field("device_name", &self.device_name)
            .field("device_id", &self.device_id)
            .field("bot", &self.bot)
            .field("personal", &self.personal)
            .finish_non_exhaustive()
    }
}

/// The pairing in `data_dir`, if there is one.
pub fn load(data_dir: &Path) -> Result<Option<Pairing>, Error> {
    let p = path(data_dir);
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::File(format!("{}: {e}", p.display()))),
    };
    let pairing: Pairing = serde_json::from_slice(&bytes)
        .map_err(|e| Error::File(format!("{} doesn't read: {e}", p.display())))?;
    if pairing.token.trim().is_empty() || pairing.url.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(pairing))
}

/// Keep `pairing` in `data_dir` (atomic, 0600 on Unix).
pub fn save(data_dir: &Path, pairing: &Pairing) -> Result<(), Error> {
    let p = path(data_dir);
    let mut bytes = serde_json::to_vec_pretty(pairing).map_err(|e| Error::File(e.to_string()))?;
    bytes.push(b'\n');
    copper_core::store::write_atomic(&p, &bytes, true, false)
        .map_err(|e| Error::File(e.to_string()))?;
    // A backup from an older build would hold an older token.
    let _ = std::fs::remove_file(copper_core::store::backup_path(&p));
    Ok(())
}

/// Forget the pairing here. `Ok(true)` if there was one.
pub fn remove(data_dir: &Path) -> Result<bool, Error> {
    let p = path(data_dir);
    let _ = std::fs::remove_file(copper_core::store::backup_path(&p));
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::File(format!("{}: {e}", p.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Pairing {
        Pairing {
            url: "https://127.0.0.1:17491".into(),
            fingerprint: Some("ab".repeat(32)),
            token: "bpd_dev_SECRET-TOKEN-BODY".into(),
            device_name: "Copper on box".into(),
            device_id: "dev".into(),
            bot: Some("copper".into()),
            bot_id: Some("b1".into()),
            personal: false,
            paired_at: Some(1),
            extra: Map::new(),
        }
    }

    #[test]
    fn saves_private_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
        save(dir.path(), &sample()).unwrap();
        assert_eq!(load(dir.path()).unwrap(), Some(sample()));
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
        // Saving again leaves no backup holding a token.
        save(dir.path(), &sample()).unwrap();
        assert!(!copper_core::store::backup_path(&path(dir.path())).exists());
        assert!(remove(dir.path()).unwrap());
        assert!(!remove(dir.path()).unwrap());
        assert_eq!(load(dir.path()).unwrap(), None);
    }

    #[test]
    fn debug_never_shows_the_token() {
        let text = format!("{:?}", sample());
        assert!(!text.contains("SECRET"), "{text}");
        assert!(text.contains("Copper on box"));
    }

    #[test]
    fn a_broken_file_is_an_error_not_a_pairing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), b"{not json").unwrap();
        assert!(matches!(load(dir.path()), Err(Error::File(_))));
    }
}
