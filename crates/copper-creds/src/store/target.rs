//! Credential Manager target names for Copper's items, and the names of the
//! DPAPI files that hold secrets too big for Credential Manager.
//!
//! - A login: `Copper/<origin>/<user>` for the real store (what a person sees
//!   in Control Panel ▸ Credential Manager ▸ Windows Credentials), and
//!   `Copper@<tag>/<origin>/<user>` for any other tag (a test run's
//!   `Copper (test)`), so tags never share a prefix.
//! - A raw item ([`super::store_raw`]): `Copper:raw/<sha-256 of its
//!   attributes>`, so writing the same attributes again replaces it.
//!
//! Each component is escaped: everything but lowercase ASCII letters, digits
//! and `-._~@+:,;=!$'()` becomes `%XX` (UTF-8 bytes, uppercase hex). That
//! keeps `/` an unambiguous separator, keeps `*` (the `CredEnumerateW`
//! wildcard) out of every name, and keeps distinct values distinct even
//! though Credential Manager compares target names case-insensitively: an
//! uppercase letter is never written literally, and the only letters that
//! could differ by case are hex digits, which are always uppercase.

// Only Windows calls these; elsewhere they are compiled for their tests.
#![cfg_attr(not(windows), allow(dead_code))]

use sha2::{Digest, Sha256};

/// The first characters of every target name Copper writes.
pub(crate) const NAMESPACE: &str = "Copper";
/// `CredEnumerateW` filter for everything Copper wrote, any tag, raw or not.
pub(crate) const NAMESPACE_FILTER: &str = "Copper*";
/// Prefix of raw items' target names.
pub(crate) const RAW_PREFIX: &str = "Copper:raw/";
/// The tag whose logins sit directly under `Copper/`.
const DEFAULT_TAG: &str = super::APP;

/// Bytes written as themselves; everything else is `%XX`.
fn literal(b: u8) -> bool {
    b.is_ascii_lowercase()
        || b.is_ascii_digit()
        || matches!(
            b,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'@'
                | b'+'
                | b':'
                | b','
                | b';'
                | b'='
                | b'!'
                | b'$'
                | b'\''
                | b'('
                | b')'
        )
}

/// One component, escaped.
pub(crate) fn escape(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        if literal(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[usize::from(b >> 4)] as char);
            out.push(HEX[usize::from(b & 0xF)] as char);
        }
    }
    out
}

/// One escaped component back, or `None` when it is not exactly what
/// [`escape`] writes (a name some other program chose).
pub(crate) fn unescape(text: &str) -> Option<String> {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            let byte = (hi << 4) | lo;
            if literal(byte) {
                return None;
            }
            out.push(byte);
            i += 3;
        } else if literal(b) {
            out.push(b);
            i += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(out).ok()
}

/// `Copper/` or `Copper@<tag>/`.
pub(crate) fn tag_prefix(tag: &str) -> String {
    if tag == DEFAULT_TAG {
        format!("{NAMESPACE}/")
    } else {
        format!("{NAMESPACE}@{}/", escape(tag))
    }
}

/// The target name of `user`'s login on `origin` under `tag`.
pub(crate) fn login_target(tag: &str, origin: &str, user: &str) -> String {
    format!("{}{}/{}", tag_prefix(tag), escape(origin), escape(user))
}

/// `CredEnumerateW` filter for every login of `tag`.
pub(crate) fn tag_filter(tag: &str) -> String {
    format!("{}*", tag_prefix(tag))
}

/// `CredEnumerateW` filter for every login of `tag` on `origin`.
pub(crate) fn origin_filter(tag: &str, origin: &str) -> String {
    format!("{}{}/*", tag_prefix(tag), escape(origin))
}

/// A login target name back into `(tag, origin, user)`; `None` for anything
/// Copper did not write as a login. Case matters here even though it does
/// not to Credential Manager: a name is only ours in its exact form.
pub(crate) fn parse_login(target: &str) -> Option<(String, String, String)> {
    let rest = target.strip_prefix(NAMESPACE)?;
    let (tag, rest) = if let Some(rest) = rest.strip_prefix('/') {
        (DEFAULT_TAG.to_string(), rest)
    } else {
        let rest = rest.strip_prefix('@')?;
        let (tag, rest) = rest.split_once('/')?;
        let tag = unescape(tag)?;
        if tag == DEFAULT_TAG {
            return None;
        }
        (tag, rest)
    };
    let (origin, user) = rest.split_once('/')?;
    let origin = unescape(origin)?;
    let user = unescape(user)?;
    (!origin.is_empty()).then_some((tag, origin, user))
}

/// The target name of a raw item with these attributes (order and
/// duplicates do not matter; a later value for a key wins).
pub(crate) fn raw_target(attributes: &[(&str, &str)]) -> String {
    let canonical: std::collections::BTreeMap<&str, &str> = attributes.iter().copied().collect();
    let mut hash = Sha256::new();
    for (k, v) in canonical {
        hash.update((k.len() as u64).to_le_bytes());
        hash.update(k.as_bytes());
        hash.update((v.len() as u64).to_le_bytes());
        hash.update(v.as_bytes());
    }
    format!("{RAW_PREFIX}{}", hex(&hash.finalize()))
}

/// The DPAPI file that holds the secret of the item named `target`, when
/// the secret is too big for Credential Manager: a hash, so the name says
/// nothing about the site and the same item always lands in the same file.
pub(crate) fn vault_file(target: &str) -> String {
    format!("{}.dpapi", hex(&Sha256::digest(target.as_bytes())))
}

/// Whether `name` is something [`vault_file`] produces (and so safe to join
/// onto the vault folder: no separators, no `..`).
pub(crate) fn is_vault_file(name: &str) -> bool {
    name.strip_suffix(".dpapi").is_some_and(|stem| {
        stem.len() == 64 && stem.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_read_plainly() {
        assert_eq!(
            login_target("Copper", "example.com", "ann@example.com"),
            "Copper/example.com/ann@example.com"
        );
        assert_eq!(
            login_target("Copper", "localhost:3000", ""),
            "Copper/localhost:3000/"
        );
        assert_eq!(
            login_target("Copper (test)", "a.b", "c"),
            "Copper@%43opper%20(test)/a.b/c"
        );
        assert_eq!(tag_filter("Copper"), "Copper/*");
        assert_eq!(origin_filter("Copper", "x.org"), "Copper/x.org/*");
    }

    #[test]
    fn separators_wildcards_and_case_are_escaped() {
        let t = login_target("Copper", "example.com", "Ann/B*%");
        assert_eq!(t, "Copper/example.com/%41nn%2F%42%2A%25");
        assert!(!t[7..].contains('*'));
        // Differ only by case: still different once Credential Manager
        // folds case.
        let a = login_target("Copper", "example.com", "ann");
        let b = login_target("Copper", "example.com", "Ann");
        assert!(!a.eq_ignore_ascii_case(&b));
        let c = login_target("Copper", "example.com", "é");
        let d = login_target("Copper", "example.com", "É");
        assert!(!c.eq_ignore_ascii_case(&d));
    }

    #[test]
    fn names_round_trip() {
        for (tag, origin, user) in [
            ("Copper", "example.com", "ann"),
            ("Copper", "example.com", ""),
            ("Copper", "[::1]:8080", "Ann Smith/ops*\\%\u{0}\n"),
            ("Copper (test)", "accounts.example.com", "ünï©ødé 🔑"),
            ("copper", "x.y", "z"),
            ("", "x.y", "z"),
        ] {
            let t = login_target(tag, origin, user);
            assert_eq!(
                parse_login(&t),
                Some((tag.to_string(), origin.to_string(), user.to_string())),
                "{t}"
            );
        }
    }

    #[test]
    fn foreign_names_are_not_ours() {
        for t in [
            "Copper",
            "Copper/",
            "Copper/example.com",
            "Copper//ann",
            "Copper/Example.com/ann",
            "Copper/example.com/a/b",
            "Copper/example.com/%2",
            "Copper/example.com/%2f",
            "Copper/example.com/%61",
            "Copper/example.com/%FF",
            "Copper@%43opper/x/y",
            "Copper:raw/abc",
            "copper/example.com/ann",
            "git:https://github.com",
        ] {
            assert_eq!(parse_login(t), None, "{t}");
        }
    }

    #[test]
    fn raw_names_follow_the_attributes() {
        let a = raw_target(&[("application", "chrome"), ("xdg:schema", "s")]);
        let b = raw_target(&[("xdg:schema", "s"), ("application", "chrome")]);
        assert_eq!(a, b);
        assert!(a.starts_with(RAW_PREFIX));
        assert_eq!(a.len(), RAW_PREFIX.len() + 64);
        assert_ne!(a, raw_target(&[("application", "chromium")]));
        // Length-prefixed: no ambiguity from moving bytes between key and value.
        assert_ne!(raw_target(&[("ab", "c")]), raw_target(&[("a", "bc")]));
    }

    #[test]
    fn vault_files_are_plain_hashes() {
        let f = vault_file("Copper/example.com/ann");
        assert!(is_vault_file(&f), "{f}");
        assert_ne!(f, vault_file("Copper/example.com/bob"));
        for bad in [
            "../x.dpapi",
            "x.dpapi",
            &f.to_uppercase(),
            &f.replace(".dpapi", ".bin"),
            &format!("..\\{f}"),
        ] {
            assert!(!is_vault_file(bad), "{bad}");
        }
    }
}
