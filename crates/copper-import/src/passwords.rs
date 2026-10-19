//! Chromium's saved passwords on Linux (`Import.swift` `Chromium.read`).
//!
//! `Login Data` is SQLite; each `password_value` is `v10` or `v11` and then
//! AES-128-CBC (IV of sixteen spaces, PKCS#7). The key is
//! PBKDF2-HMAC-SHA1(password, "saltysalt", 1 iteration, 16 bytes):
//!
//! - `v10`: the password is the fixed `peanuts` (no keyring, or
//!   `--password-store=basic`);
//! - `v11`: the password is the browser's own Secret Service item ("Chrome
//!   Safe Storage", `application=chrome`). Chromium also tries the empty
//!   password for v11 (data written while the keyring was unavailable).
//!
//! The file is copied before it is read. Secrets are held in
//! [`ImportedLogin`] only, whose `Debug` never prints the password.
//!
//! Linux only: on Windows Copper never opens another browser's
//! `Login Data`; passwords come over through the browser's own CSV export
//! ([`crate::passwords_csv`]).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use rusqlite::{Connection, OpenFlags};

pub use crate::login::ImportedLogin;
use crate::{Browser, Copied, ImportError, chromium_to_unix};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// A derived AES-128 key.
pub type Key = [u8; 16];

/// PBKDF2-HMAC-SHA1 with Chromium's Linux salt and one iteration.
pub fn derive(password: &[u8]) -> Key {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", 1, &mut key);
    key
}

/// The keys a `Login Data` (or `Cookies`) value may be under.
#[derive(Clone)]
pub struct Keys {
    pub v10: Key,
    /// From the Secret Service; `None` when the keyring has no item.
    pub v11: Option<Key>,
    /// Chromium's fallback for v11 written without a keyring.
    pub v11_empty: Key,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("v11", &self.v11.is_some())
            .finish_non_exhaustive()
    }
}

impl Keys {
    /// v10 only (and the empty v11 fallback).
    pub fn basic() -> Self {
        Keys {
            v10: derive(b"peanuts"),
            v11: None,
            v11_empty: derive(b""),
        }
    }

    /// With the v11 password as the keyring holds it.
    pub fn with_v11_password(password: &[u8]) -> Self {
        Keys {
            v11: Some(derive(password)),
            ..Self::basic()
        }
    }

    /// Read the browser's key from the Secret Service: its libsecret v2
    /// schema item for `application`, then any item with that attribute.
    /// A keyring without one gives [`Keys::basic`]; a keyring that can't be
    /// reached is an error, since v11 rows would silently be lost.
    pub async fn for_browser(browser: &Browser) -> Result<Self, ImportError> {
        use copper_creds::store::lookup;
        let refused = |_| ImportError::KeyRefused(browser.name.clone());
        let app = browser.application.as_str();
        let mut secret = lookup(&[
            ("xdg:schema", "chrome_libsecret_os_crypt_password_v2"),
            ("application", app),
        ])
        .await
        .map_err(refused)?;
        if secret.is_none() {
            secret = lookup(&[("application", app)]).await.map_err(refused)?;
        }
        Ok(match secret {
            Some(password) => Self::with_v11_password(&password),
            None => Self::basic(),
        })
    }
}

/// `v10`/`v11` + AES-128-CBC. Unprefixed values are plaintext (very old
/// profiles). Newer builds may put a 32-byte hash before the value.
pub fn unwrap(blob: &[u8], keys: &Keys) -> Option<String> {
    let (tag, body) = match blob.get(..3) {
        Some(b"v10") => ("v10", &blob[3..]),
        Some(b"v11") => ("v11", &blob[3..]),
        _ => return String::from_utf8(blob.to_vec()).ok(),
    };
    let candidates: Vec<&Key> = if tag == "v10" {
        vec![&keys.v10]
    } else {
        keys.v11
            .iter()
            .chain(std::iter::once(&keys.v11_empty))
            .collect()
    };
    for key in candidates {
        let Some(plain) = decrypt(body, key) else {
            continue;
        };
        if let Ok(text) = String::from_utf8(plain.clone()) {
            return Some(text);
        }
        if plain.len() > 32
            && let Ok(text) = String::from_utf8(plain[32..].to_vec())
        {
            return Some(text);
        }
    }
    None
}

fn decrypt(body: &[u8], key: &Key) -> Option<Vec<u8>> {
    if body.is_empty() || !body.len().is_multiple_of(16) {
        return None;
    }
    let iv = [b' '; 16];
    let mut buf = body.to_vec();
    let n = Aes128CbcDec::new(key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?
        .len();
    buf.truncate(n);
    Some(buf)
}

/// The inverse of [`unwrap`] for one tag: what Chromium writes (tests and
/// fixture builders).
pub fn wrap(plain: &[u8], key: &Key, tag: &str) -> Vec<u8> {
    use aes::cipher::BlockEncryptMut;
    type Enc = cbc::Encryptor<aes::Aes128>;
    let iv = [b' '; 16];
    let n = plain.len();
    let mut buf = plain.to_vec();
    buf.resize(n + 16 - n % 16, 0);
    let sealed = Enc::new(key.into(), &iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, n)
        .expect("room for the padding")
        .to_vec();
    let mut out = tag.as_bytes().to_vec();
    out.extend(sealed);
    out
}

/// What a read found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Found {
    pub logins: Vec<ImportedLogin>,
    /// Sites the person told the browser never to save for.
    pub never: Vec<String>,
    /// Rows whose value couldn't be decrypted (wrong or missing key).
    pub undecryptable: usize,
}

/// Count saved passwords without the key.
pub fn count(profile_dirs: &[PathBuf]) -> usize {
    profile_dirs
        .iter()
        .filter_map(|p| rows(&p.join("Login Data")).ok())
        .map(|rows| rows.iter().filter(|r| !r.never).count())
        .sum()
}

/// Every profile's `Login Data`, decrypted. An error only when no file at
/// all could be read.
pub fn read(profile_dirs: &[PathBuf], keys: &Keys) -> Result<Found, ImportError> {
    let mut found = Found::default();
    let mut seen = HashSet::new();
    let mut read_any = false;
    for profile in profile_dirs {
        let Ok(rows) = rows(&profile.join("Login Data")) else {
            continue;
        };
        read_any = true;
        for row in rows {
            let host = copper_creds::site::host_of(&row.origin);
            if host.is_empty() {
                continue;
            }
            if row.never {
                found.never.push(host);
                continue;
            }
            let Some(password) = unwrap(&row.blob, keys) else {
                found.undecryptable += 1;
                continue;
            };
            if password.is_empty() || !seen.insert(format!("{host}\u{1}{}", row.user)) {
                continue;
            }
            found.logins.push(ImportedLogin {
                host,
                user: row.user,
                password,
                used: row.used,
            });
        }
    }
    if !read_any {
        return Err(ImportError::Unreadable("saved passwords".into()));
    }
    Ok(found)
}

struct Row {
    origin: String,
    user: String,
    blob: Vec<u8>,
    never: bool,
    used: Option<f64>,
}

fn rows(file: &Path) -> Result<Vec<Row>, ImportError> {
    let bad = || ImportError::Unreadable(file.display().to_string());
    if !file.is_file() {
        return Err(bad());
    }
    let copy = Copied::of(file).map_err(|_| bad())?;
    let db = Connection::open_with_flags(&copy.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| bad())?;
    let mut st = db
        .prepare(
            "SELECT origin_url, username_value, password_value, blacklisted_by_user, date_last_used \
             FROM logins",
        )
        .map_err(|_| bad())?;
    let rows = st
        .query_map([], |r| {
            Ok(Row {
                origin: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                user: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                blob: r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
                never: r.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0,
                used: chromium_to_unix(r.get::<_, Option<i64>>(4)?.unwrap_or(0)),
            })
        })
        .map_err(|_| bad())?;
    Ok(rows.flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // Chromium's v10 key on Linux, as the many decrypt scripts print it.
        let k = derive(b"peanuts");
        assert_eq!(
            k.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "fd621fe5a2b402539dfa147ca9272778"
        );
        let keys = Keys::with_v11_password(b"s3cret");
        let v10 = wrap(b"hunter2", &keys.v10, "v10");
        assert_eq!(unwrap(&v10, &keys).as_deref(), Some("hunter2"));
        let v11 = wrap(b"pa55", keys.v11.as_ref().unwrap(), "v11");
        assert_eq!(unwrap(&v11, &keys).as_deref(), Some("pa55"));
        assert_eq!(unwrap(&v11, &Keys::basic()), None);
        let empty = wrap(b"nokeyring", &keys.v11_empty, "v11");
        assert_eq!(unwrap(&empty, &keys).as_deref(), Some("nokeyring"));
        let mut hashed = vec![0xffu8; 32];
        hashed.extend(b"after-hash");
        assert_eq!(
            unwrap(&wrap(&hashed, &keys.v10, "v10"), &keys).as_deref(),
            Some("after-hash")
        );
        assert_eq!(unwrap(b"plain", &keys).as_deref(), Some("plain"));
        assert!(
            !format!(
                "{:?}",
                ImportedLogin {
                    host: "h".into(),
                    user: "u".into(),
                    password: "SECRET".into(),
                    used: None
                }
            )
            .contains("SECRET")
        );
    }
}
