//! Safe wrappers over the Credential Manager calls the Windows store uses:
//! `CredReadW`, `CredWriteW`, `CredDeleteW`, `CredEnumerateW`, generic
//! credentials only.
//!
//! Every credential blob handed back by Windows is copied into a
//! [`Zeroizing`] buffer and the system's copy is zeroed before `CredFree`, so
//! no secret lingers in freed memory — including the blobs `CredEnumerateW`
//! returns while Copper only wanted names and attributes.

use windows::Win32::Foundation::{ERROR_NO_SUCH_LOGON_SESSION, ERROR_NOT_FOUND, FILETIME};
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_MAX_ATTRIBUTES, CRED_MAX_CREDENTIAL_BLOB_SIZE,
    CRED_MAX_GENERIC_TARGET_NAME_LENGTH, CRED_MAX_STRING_LENGTH, CRED_MAX_USERNAME_LENGTH,
    CRED_MAX_VALUE_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_PRESERVE_CREDENTIAL_BLOB,
    CRED_TYPE_GENERIC, CREDENTIAL_ATTRIBUTEW, CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree,
    CredReadW, CredWriteW,
};
use windows::core::{PCWSTR, PWSTR};
use zeroize::{Zeroize, Zeroizing};

use crate::store::StoreError;

/// The biggest secret Credential Manager keeps itself (2560 bytes); a
/// bigger one goes to a DPAPI file (see `vault`).
pub(super) const MAX_BLOB: usize = CRED_MAX_CREDENTIAL_BLOB_SIZE as usize;
/// The most attributes one credential carries (64).
pub(super) const MAX_ATTRIBUTES: usize = CRED_MAX_ATTRIBUTES as usize;

/// One generic credential as read back.
pub(super) struct Cred {
    pub target: String,
    /// Keyword → value, in the order Windows returned them.
    pub attributes: Vec<(String, Vec<u8>)>,
    pub blob: Zeroizing<Vec<u8>>,
}

impl Cred {
    /// An attribute's value as text (`None` when absent or not UTF-8).
    pub fn attribute(&self, keyword: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k == keyword)
            .and_then(|(_, v)| std::str::from_utf8(v).ok())
    }
}

/// What to write.
pub(super) struct NewCred<'a> {
    pub target: &'a str,
    /// Shown in Credential Manager; cut to fit.
    pub comment: &'a str,
    /// Shown in Credential Manager; cut to fit, empty = none.
    pub user_name: &'a str,
    pub attributes: &'a [(String, Vec<u8>)],
    /// The secret; `None` keeps the existing credential's blob untouched
    /// (`CRED_PRESERVE_CREDENTIAL_BLOB`) — only the attributes change.
    pub blob: Option<&'a [u8]>,
}

fn not_found(e: &windows::core::Error) -> bool {
    e.code() == ERROR_NOT_FOUND.to_hresult()
}

/// A failed call in the store's terms: no credential store for this logon
/// session (a service or network logon) means passwords are off.
fn failed(call: &str, e: &windows::core::Error) -> StoreError {
    if e.code() == ERROR_NO_SUCH_LOGON_SESSION.to_hresult() {
        StoreError::Unavailable(format!(
            "this logon session has no credential store ({call})"
        ))
    } else {
        StoreError::Service(format!("{call}: {}", e.message()))
    }
}

/// `s` as a NUL-terminated UTF-16 buffer, cut to at most `max` code units
/// (on a character boundary) and to its first NUL.
fn wide(s: &str, max: usize) -> Vec<u16> {
    let s = s.split('\0').next().unwrap_or_default();
    let mut out: Vec<u16> = Vec::with_capacity(s.len() + 1);
    for c in s.chars() {
        let mut units = [0u16; 2];
        let units = c.encode_utf16(&mut units);
        if out.len() + units.len() > max {
            break;
        }
        out.extend_from_slice(units);
    }
    out.push(0);
    out
}

/// A target name as UTF-16, refused rather than cut when too long.
fn target_wide(target: &str) -> Result<Vec<u16>, StoreError> {
    let units = target.encode_utf16().count();
    if units > CRED_MAX_GENERIC_TARGET_NAME_LENGTH as usize || target.contains('\0') {
        return Err(StoreError::Service(
            "the name is too long for Credential Manager".into(),
        ));
    }
    Ok(wide(target, units))
}

/// A NUL-terminated wide string Windows handed back, strictly.
///
/// # Safety
/// `p` is null or points at a NUL-terminated UTF-16 string.
unsafe fn read_wide(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a NUL-terminated string.
    unsafe { p.to_string() }.ok()
}

/// Copy one credential out of Windows' memory and zero its blob there.
///
/// # Safety
/// `c` points at a valid `CREDENTIALW` from `CredReadW`/`CredEnumerateW`
/// that has not been freed yet.
unsafe fn take(c: *mut CREDENTIALW) -> Option<Cred> {
    // SAFETY: valid per the caller.
    let c = unsafe { &mut *c };
    let blob = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
        Zeroizing::new(Vec::new())
    } else {
        // SAFETY: Windows gives `CredentialBlobSize` bytes at
        // `CredentialBlob`, owned by this allocation until `CredFree`.
        let system = unsafe {
            std::slice::from_raw_parts_mut(c.CredentialBlob, c.CredentialBlobSize as usize)
        };
        let copy = Zeroizing::new(system.to_vec());
        system.zeroize();
        copy
    };
    if c.Type != CRED_TYPE_GENERIC {
        return None;
    }
    // SAFETY: TargetName is a NUL-terminated string of this credential.
    let target = unsafe { read_wide(c.TargetName) }?;
    let mut attributes = Vec::with_capacity(c.AttributeCount as usize);
    if !c.Attributes.is_null() {
        // SAFETY: `AttributeCount` entries at `Attributes`.
        let list = unsafe { std::slice::from_raw_parts(c.Attributes, c.AttributeCount as usize) };
        for a in list {
            // SAFETY: Keyword is a NUL-terminated string.
            let Some(keyword) = (unsafe { read_wide(a.Keyword) }) else {
                continue;
            };
            let value = if a.Value.is_null() || a.ValueSize == 0 {
                Vec::new()
            } else {
                // SAFETY: `ValueSize` bytes at `Value`.
                unsafe { std::slice::from_raw_parts(a.Value, a.ValueSize as usize) }.to_vec()
            };
            attributes.push((keyword, value));
        }
    }
    Some(Cred {
        target,
        attributes,
        blob,
    })
}

/// The generic credential named `target`, if there is one.
pub(super) fn read(target: &str) -> Result<Option<Cred>, StoreError> {
    let name = target_wide(target)?;
    let mut found: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; `found` receives an allocation we
    // free below.
    match unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut found) } {
        Ok(()) => {
            // SAFETY: `found` is the live credential CredReadW returned; it is
            // freed exactly once, right after.
            let cred = unsafe { take(found) };
            unsafe { CredFree(found as *const _) };
            Ok(cred)
        }
        Err(e) if not_found(&e) => Ok(None),
        Err(e) => Err(failed("CredReadW", &e)),
    }
}

/// Every generic credential whose target name matches `filter` (a prefix
/// ending in `*`). Credential Manager matches case-insensitively, so callers
/// check the names they get.
pub(super) fn enumerate(filter: &str) -> Result<Vec<Cred>, StoreError> {
    let filter = target_wide(filter)?;
    let mut count = 0u32;
    let mut list: *mut *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `filter` is NUL-terminated; `list` receives an array we free
    // below.
    match unsafe { CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut list) } {
        Ok(()) => {}
        Err(e) if not_found(&e) => return Ok(Vec::new()),
        Err(e) => return Err(failed("CredEnumerateW", &e)),
    }
    let mut out = Vec::with_capacity(count as usize);
    if !list.is_null() {
        // SAFETY: `count` credential pointers at `list`, all live until the
        // single CredFree of the array below.
        let items = unsafe { std::slice::from_raw_parts(list, count as usize) };
        for &c in items {
            // SAFETY: a live credential of the array, freed below.
            if !c.is_null()
                && let Some(cred) = unsafe { take(c) }
            {
                out.push(cred);
            }
        }
        // SAFETY: the array CredEnumerateW returned, freed once.
        unsafe { CredFree(list as *const _) };
    }
    Ok(out)
}

/// Create or replace a generic credential, persisted for this user on this
/// machine (`CRED_PERSIST_LOCAL_MACHINE`: it survives sign-out, never roams).
pub(super) fn write(new: &NewCred<'_>) -> Result<(), StoreError> {
    if new.attributes.len() > MAX_ATTRIBUTES {
        return Err(StoreError::Service(
            "too many attributes for Credential Manager".into(),
        ));
    }
    if new.blob.is_some_and(|b| b.len() > MAX_BLOB) {
        return Err(StoreError::Service(
            "the secret is too big for Credential Manager".into(),
        ));
    }
    let mut target = target_wide(new.target)?;
    let mut comment = wide(new.comment, CRED_MAX_STRING_LENGTH as usize - 1);
    let mut user_name = wide(new.user_name, CRED_MAX_USERNAME_LENGTH as usize - 1);
    let mut keywords: Vec<Vec<u16>> = Vec::with_capacity(new.attributes.len());
    for (k, v) in new.attributes {
        if k.is_empty()
            || k.contains('\0')
            || k.encode_utf16().count() > CRED_MAX_STRING_LENGTH as usize
            || v.len() > CRED_MAX_VALUE_SIZE as usize
        {
            return Err(StoreError::Service(format!(
                "attribute {k:?} does not fit Credential Manager (keyword ≤ 256 characters, value ≤ 256 bytes)"
            )));
        }
        keywords.push(wide(k, usize::MAX));
    }
    // The values are only read by CredWriteW; the `*mut` is the struct's
    // shape, not a licence to write.
    let mut attributes: Vec<CREDENTIAL_ATTRIBUTEW> = new
        .attributes
        .iter()
        .zip(keywords.iter_mut())
        .map(|((_, v), k)| CREDENTIAL_ATTRIBUTEW {
            Keyword: PWSTR(k.as_mut_ptr()),
            Flags: 0,
            ValueSize: v.len() as u32,
            Value: v.as_ptr() as *mut u8,
        })
        .collect();
    let (blob_ptr, blob_len, flags) = match new.blob {
        Some(b) => (b.as_ptr() as *mut u8, b.len() as u32, 0),
        None => (std::ptr::null_mut(), 0, CRED_PRESERVE_CREDENTIAL_BLOB),
    };
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target.as_mut_ptr()),
        Comment: PWSTR(comment.as_mut_ptr()),
        LastWritten: FILETIME::default(),
        CredentialBlobSize: blob_len,
        CredentialBlob: blob_ptr,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: attributes.len() as u32,
        Attributes: if attributes.is_empty() {
            std::ptr::null_mut()
        } else {
            attributes.as_mut_ptr()
        },
        TargetAlias: PWSTR::null(),
        UserName: if new.user_name.is_empty() {
            PWSTR::null()
        } else {
            PWSTR(user_name.as_mut_ptr())
        },
    };
    // SAFETY: every pointer in `cred` points into a buffer above that
    // outlives this call; CredWriteW copies what it keeps.
    unsafe { CredWriteW(&cred, flags) }.map_err(|e| failed("CredWriteW", &e))
}

/// Delete the generic credential named `target`; `false` when there was none.
pub(super) fn delete(target: &str) -> Result<bool, StoreError> {
    let name = target_wide(target)?;
    // SAFETY: `name` is NUL-terminated.
    match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
        Ok(()) => Ok(true),
        Err(e) if not_found(&e) => Ok(false),
        Err(e) => Err(failed("CredDeleteW", &e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_are_cut_on_character_boundaries() {
        assert_eq!(wide("ab", 10), vec![b'a' as u16, b'b' as u16, 0]);
        assert_eq!(wide("abc", 2), vec![b'a' as u16, b'b' as u16, 0]);
        // A surrogate pair is never split.
        assert_eq!(wide("a🔑", 2), vec![b'a' as u16, 0]);
        assert_eq!(wide("a\0b", 10), vec![b'a' as u16, 0]);
        assert!(target_wide("x\0y").is_err());
        assert!(target_wide(&"x".repeat(40_000)).is_err());
    }
}
