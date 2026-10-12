//! Time-based one-time codes from a stored seed (`TOTP.swift`): an
//! `otpauth://totp/...?secret=...&digits=6&period=30&algorithm=SHA1` URI or
//! a bare base32 secret. `None` for anything else, so the caller can ask the
//! vault's CLI instead.

use hmac::{Hmac, Mac};
use url::Url;

macro_rules! mac_with {
    ($digest:ty, $key:expr, $message:expr) => {{
        let mut mac = <Hmac<$digest>>::new_from_slice($key).expect("hmac takes any key length");
        mac.update($message);
        mac.finalize().into_bytes().to_vec()
    }};
}

/// The code for `seed` at `unix` seconds.
pub fn code(seed: &str, unix: f64) -> Option<String> {
    let mut secret = seed.trim().to_string();
    let mut digits: u32 = 6;
    let mut period: f64 = 30.0;
    let mut algorithm = "SHA1".to_string();
    let lower = secret.to_lowercase();
    if lower.starts_with("otpauth://") {
        let url = Url::parse(&secret).ok()?;
        if url.host_str()?.to_lowercase() != "totp" {
            return None;
        }
        let mut found = None;
        for (name, value) in url.query_pairs() {
            match name.to_lowercase().as_str() {
                "secret" => found = Some(value.to_string()),
                "digits" => digits = value.parse().unwrap_or(digits),
                "period" => period = value.parse().unwrap_or(period),
                "algorithm" => algorithm = value.to_uppercase(),
                _ => {}
            }
        }
        secret = found?;
    } else if lower.starts_with("steam://") {
        return None;
    }
    let key = base32(&secret)?;
    if key.is_empty() || !(6..=8).contains(&digits) || period <= 0.0 || !period.is_finite() {
        return None;
    }
    let counter = ((unix / period).floor() as u64).to_be_bytes();
    let mac = match algorithm.as_str() {
        "SHA1" => mac_with!(sha1::Sha1, &key, &counter),
        "SHA256" => mac_with!(sha2::Sha256, &key, &counter),
        "SHA512" => mac_with!(sha2::Sha512, &key, &counter),
        _ => return None,
    };
    let offset = (mac[mac.len() - 1] & 0x0f) as usize;
    let binary = (u32::from(mac[offset] & 0x7f) << 24)
        | (u32::from(mac[offset + 1]) << 16)
        | (u32::from(mac[offset + 2]) << 8)
        | u32::from(mac[offset + 3]);
    let value = binary % 10u32.pow(digits);
    Some(format!("{value:0width$}", width = digits as usize))
}

/// The code now.
pub fn code_now(seed: &str) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    code(seed, now)
}

fn base32(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = 0u32;
    let mut buffer: u32 = 0;
    let mut out = Vec::new();
    for c in text.to_uppercase().chars() {
        if matches!(c, '=' | ' ' | '-') {
            continue;
        }
        let v = ALPHABET.iter().position(|&a| a as char == c)? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((buffer >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 appendix B; seed "12345678901234567890" in base32.
    const SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn rfc6238() {
        assert_eq!(code(SEED, 59.0).as_deref(), Some("287082"));
        let uri = format!("otpauth://totp/x?secret={SEED}&digits=8");
        assert_eq!(code(&uri, 59.0).as_deref(), Some("94287082"));
        assert_eq!(code(&uri, 1111111109.0).as_deref(), Some("07081804"));
        assert_eq!(code(&uri, 2000000000.0).as_deref(), Some("69279037"));
    }

    #[test]
    fn refuses_what_it_does_not_know() {
        assert!(code("steam://ABC", 0.0).is_none());
        assert!(code("otpauth://hotp/x?secret=GEZD", 0.0).is_none());
        assert!(code("not base32!", 0.0).is_none());
        assert!(code("otpauth://totp/x?secret=GEZD&algorithm=MD5", 0.0).is_none());
    }
}
