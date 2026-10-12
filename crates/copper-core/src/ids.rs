//! Identifiers and secrets, minted the way the Mac mints them.

use std::io::Read;

/// A fresh UUID in Swift's `UUID().uuidString` form: uppercase, hyphenated.
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}

/// True if `text` parses as a UUID (any case), as Swift's `UUID(uuidString:)`.
pub fn is_uuid(text: &str) -> bool {
    text.len() == 36 && uuid::Uuid::parse_str(text).is_ok()
}

/// `n` bytes from the operating system's random source.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if !filled {
        // /dev/urandom is always there on Linux; this is belt and braces.
        for chunk in buf.chunks_mut(16) {
            let u = uuid::Uuid::new_v4();
            let bytes = u.as_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
    buf
}

/// A bearer token as `MCP.freshToken()` makes one: 32 random bytes, lowercase
/// hex (64 characters).
pub fn fresh_token() -> String {
    random_bytes(32)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let u = new_uuid();
        assert!(is_uuid(&u));
        assert_eq!(u, u.to_uppercase());
        let t = fresh_token();
        assert_eq!(t.len(), 64);
        assert!(
            t.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(fresh_token(), t);
    }
}
