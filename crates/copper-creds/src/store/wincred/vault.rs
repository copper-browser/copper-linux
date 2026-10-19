//! Secrets too big for Credential Manager (over 2560 bytes): sealed with
//! DPAPI and kept in a file, with only a pointer in Credential Manager.
//!
//! Why a file rather than chunking across several credentials: one item
//! stays one credential (written, renamed and deleted atomically, never half
//! a secret if Copper dies mid-write), Credential Manager's list stays
//! readable, and DPAPI gives the same protection Credential Manager does
//! (the user's logon key; another user or an offline copy of the disk can't
//! open it).
//!
//! - Folder: `%LOCALAPPDATA%\Copper\Data\vault\` (per user, never roams).
//! - File: `<sha-256 of the target name>.dpapi` (see `target::vault_file`) —
//!   the name says nothing about the site, and the same item always lands in
//!   the same file, so an update overwrites and nothing piles up.
//! - Contents: `MAGIC` then a `CryptProtectData` blob, user scope, with
//!   `ENTROPY` + the target name as optional entropy: a file copied over
//!   another item's will not open there.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::w;
use zeroize::{Zeroize, Zeroizing};

use crate::store::StoreError;
use crate::store::target::{is_vault_file, vault_file};

/// First bytes of every vault file (format version 1).
const MAGIC: &[u8] = b"CopperVault\x01";
/// DPAPI optional entropy, before the target name. Changing it orphans
/// every sealed secret.
const ENTROPY: &[u8] = b"com.copperbrowser.Copper.vault.v1\0";

fn failed(what: &str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Service(format!("vault {what}: {e}"))
}

/// The folder big secrets are sealed into.
pub(super) struct Vault {
    dir: Option<PathBuf>,
}

impl Vault {
    /// `%LOCALAPPDATA%\Copper\Data\vault`.
    pub fn standard() -> Self {
        Vault {
            dir: local_app_data().map(|d| d.join("Copper").join("Data").join("vault")),
        }
    }

    /// A vault in `dir` (tests).
    #[cfg(test)]
    pub fn at(dir: PathBuf) -> Self {
        Vault { dir: Some(dir) }
    }

    fn dir(&self) -> Result<&Path, StoreError> {
        self.dir
            .as_deref()
            .ok_or_else(|| StoreError::Service("no local app data folder for the vault".into()))
    }

    /// Seal `secret` for the item `target`; the file name to point at.
    ///
    /// Written to a temporary file and renamed over the old one, so a reader
    /// sees the old secret or the new one, never a torn file. No ACL is set:
    /// the folder sits in the user's profile, which only the user (and
    /// SYSTEM/Administrators) can read, and the contents are DPAPI-sealed to
    /// the user anyway.
    pub fn seal(&self, target: &str, secret: &[u8]) -> Result<String, StoreError> {
        let dir = self.dir()?;
        std::fs::create_dir_all(dir).map_err(|e| failed("folder", e))?;
        let sealed = protect(secret, &entropy(target))?;
        let mut contents = Vec::with_capacity(MAGIC.len() + sealed.len());
        contents.extend_from_slice(MAGIC);
        contents.extend_from_slice(&sealed);
        let name = vault_file(target);
        let path = dir.join(&name);
        let temp = dir.join(format!("{name}.{}.tmp", std::process::id()));
        let written = std::fs::write(&temp, &contents).and_then(|()| std::fs::rename(&temp, &path));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(failed("write", e));
        }
        Ok(name)
    }

    /// The secret sealed for `target` in the file `name`.
    pub fn open(&self, target: &str, name: &str) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        if !is_vault_file(name) {
            return Err(StoreError::Service("a vault pointer is malformed".into()));
        }
        let contents = std::fs::read(self.dir()?.join(name)).map_err(|e| failed("read", e))?;
        let sealed = contents
            .strip_prefix(MAGIC)
            .ok_or_else(|| StoreError::Service("a vault file is not Copper's".into()))?;
        unprotect(sealed, &entropy(target))
    }

    /// Remove the file `target`'s secret would be sealed in, if any.
    pub fn remove(&self, target: &str) {
        let Ok(dir) = self.dir() else {
            return;
        };
        match std::fs::remove_file(dir.join(vault_file(target))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("vault: could not remove a sealed secret: {e}"),
        }
    }

    /// Whether a sealed file exists for `target` (tests).
    #[cfg(test)]
    pub fn holds(&self, target: &str) -> bool {
        self.dir
            .as_ref()
            .is_some_and(|d| d.join(vault_file(target)).is_file())
    }
}

fn entropy(target: &str) -> Vec<u8> {
    let mut e = ENTROPY.to_vec();
    e.extend_from_slice(target.as_bytes());
    e
}

/// `FOLDERID_LocalAppData`, else `%LOCALAPPDATA%`.
fn local_app_data() -> Option<PathBuf> {
    // SAFETY: a known folder id and default flags; the returned string is
    // freed with CoTaskMemFree exactly once.
    let known = unsafe {
        SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)
            .ok()
            .and_then(|p| {
                let s = p.to_string().ok();
                CoTaskMemFree(Some(p.0 as *const _));
                s
            })
    };
    known
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
}

fn blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, StoreError> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len()).map_err(|_| failed("seal", "the secret is too big"))?,
        // Only read by DPAPI.
        pbData: data.as_ptr() as *mut u8,
    })
}

/// Take DPAPI's output buffer: copied out, zeroed, freed.
///
/// # Safety
/// `out` was filled by a successful CryptProtectData/CryptUnprotectData.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Zeroizing<Vec<u8>> {
    if out.pbData.is_null() {
        return Zeroizing::new(Vec::new());
    }
    // SAFETY: DPAPI allocated `cbData` bytes at `pbData` with LocalAlloc.
    let system = unsafe { std::slice::from_raw_parts_mut(out.pbData, out.cbData as usize) };
    let copy = Zeroizing::new(system.to_vec());
    system.zeroize();
    // SAFETY: freed once, with the allocator DPAPI documents.
    unsafe { LocalFree(Some(HLOCAL(out.pbData as *mut _))) };
    copy
}

/// `CryptProtectData`, user scope, never a UI.
fn protect(secret: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let input = blob(secret)?;
    let extra = blob(entropy)?;
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point into live slices; `out` receives a
    // LocalAlloc buffer handed to `take`.
    unsafe {
        CryptProtectData(
            &input,
            w!("Copper password"),
            Some(&extra),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| failed("seal", e.message()))?;
        Ok(take(out))
    }
}

/// `CryptUnprotectData` with the same entropy.
fn unprotect(sealed: &[u8], entropy: &[u8]) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let input = blob(sealed)?;
    let extra = blob(entropy)?;
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&extra),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| failed("open", e.message()))?;
        Ok(take(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("copper-vault-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn dpapi_round_trip_is_bound_to_the_entropy() {
        let sealed = protect(b"hunter2", b"one").unwrap();
        assert!(!sealed.windows(7).any(|w| w == b"hunter2"));
        assert_eq!(unprotect(&sealed, b"one").unwrap().as_slice(), b"hunter2");
        assert!(unprotect(&sealed, b"two").is_err());
    }

    #[test]
    fn files_seal_open_and_go() {
        let dir = scratch("files");
        let v = Vault::at(dir.clone());
        let big = vec![0x5Au8; 10_000];
        let name = v.seal("Copper/example.com/ann", &big).unwrap();
        assert!(v.holds("Copper/example.com/ann"));
        assert_eq!(
            v.open("Copper/example.com/ann", &name).unwrap().as_slice(),
            big.as_slice()
        );
        // Another item's file does not open as this one.
        assert!(v.open("Copper/example.com/bob", &name).is_err());
        assert!(v.open("Copper/example.com/ann", "../evil.dpapi").is_err());
        // Overwrite in place; no temporary left behind.
        v.seal("Copper/example.com/ann", b"short").unwrap();
        assert_eq!(
            v.open("Copper/example.com/ann", &name).unwrap().as_slice(),
            b"short"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        v.remove("Copper/example.com/ann");
        v.remove("Copper/example.com/ann");
        assert!(!v.holds("Copper/example.com/ann"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_standard_vault_is_under_local_app_data() {
        let v = Vault::standard();
        let dir = v.dir().unwrap();
        assert!(dir.ends_with("Copper\\Data\\vault"), "{}", dir.display());
    }
}
