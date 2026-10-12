//! The three ways the Mac writes a moment, as distinct types.
//!
//! macOS Copper uses three date encodings, chosen per file (and sometimes per
//! field). Mixing them up silently shifts a date by 31 years, so each gets its
//! own type and the conversion is explicit:
//!
//! | Type | JSON | Where |
//! |---|---|---|
//! | [`SwiftDate`] | number, seconds since 2001-01-01T00:00:00Z | Swift `Date` with the default `JSONEncoder` strategy: `history.json` `last`, `downloads.json` `date`, `updates.json`, `claude.json` `expires`, `hidden.json` |
//! | [`UnixTime`] | number, seconds since 1970-01-01T00:00:00Z | fields declared as `Double` in Swift: `session.json` `seen`, `trails.json`, `cloud.json` `sync.pushedThrough`, `cloud-sync.json` `remote`, cloud `tabs` docs |
//! | [`IsoDate`] | string `YYYY-MM-DDTHH:MM:SSZ` | Swift `Date` with `.iso8601`: `canvas/canvases.json`, `crashes/crashes.json`, `cloud-intelligence.json` `fetched_at` |
//!
//! All three serialize transparently, so a value read from a Mac file is
//! written back unchanged.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds between the Unix epoch and Swift's reference date (2001-01-01).
pub const SWIFT_REFERENCE_UNIX: f64 = 978_307_200.0;

/// Unix seconds right now, as a float.
pub fn now_unix() -> f64 {
    system_to_unix(SystemTime::now())
}

fn system_to_unix(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

fn unix_to_system(secs: f64) -> SystemTime {
    if secs >= 0.0 {
        UNIX_EPOCH + Duration::from_secs_f64(secs)
    } else {
        UNIX_EPOCH - Duration::from_secs_f64(-secs)
    }
}

/// A Swift `Date` under the default `JSONEncoder` strategy: seconds since
/// 2001-01-01T00:00:00Z, as a JSON number.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SwiftDate(pub f64);

impl SwiftDate {
    /// Now.
    pub fn now() -> Self {
        Self::from_unix(now_unix())
    }
    /// From Unix seconds.
    pub fn from_unix(unix: f64) -> Self {
        SwiftDate(unix - SWIFT_REFERENCE_UNIX)
    }
    /// As Unix seconds.
    pub fn unix(self) -> f64 {
        self.0 + SWIFT_REFERENCE_UNIX
    }
    /// As a [`SystemTime`].
    pub fn system_time(self) -> SystemTime {
        unix_to_system(self.unix())
    }
    /// From a [`SystemTime`].
    pub fn from_system_time(t: SystemTime) -> Self {
        Self::from_unix(system_to_unix(t))
    }
}

/// Seconds since the Unix epoch as a JSON number (Swift `Double` fields such
/// as `timeIntervalSince1970`).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixTime(pub f64);

impl UnixTime {
    /// Now.
    pub fn now() -> Self {
        UnixTime(now_unix())
    }
    /// As a [`SystemTime`].
    pub fn system_time(self) -> SystemTime {
        unix_to_system(self.0)
    }
    /// From a [`SystemTime`].
    pub fn from_system_time(t: SystemTime) -> Self {
        UnixTime(system_to_unix(t))
    }
    /// The same moment as a [`SwiftDate`].
    pub fn to_swift(self) -> SwiftDate {
        SwiftDate::from_unix(self.0)
    }
}

/// A Swift `Date` under the `.iso8601` strategy: `"2026-10-02T14:21:57Z"`.
///
/// Swift's `ISO8601DateFormatter` writes whole seconds in UTC with a `Z` and
/// **refuses fractional seconds when decoding**, so this type keeps whole Unix
/// seconds and always writes that exact shape. Parsing is lenient (fractions
/// are truncated, `+hh:mm` offsets are applied) so server-supplied strings
/// still read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct IsoDate(pub i64);

impl IsoDate {
    /// Now, truncated to the second.
    pub fn now() -> Self {
        IsoDate(now_unix().floor() as i64)
    }
    /// Unix seconds.
    pub fn unix(self) -> i64 {
        self.0
    }
    /// Parse `YYYY-MM-DDTHH:MM:SS[.fff](Z|±hh:mm|±hhmm)`.
    pub fn parse(text: &str) -> Option<Self> {
        parse_rfc3339(text).map(IsoDate)
    }
}

impl fmt::Display for IsoDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secs = self.0;
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            (rem % 3600) / 60,
            rem % 60
        )
    }
}

impl Serialize for IsoDate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IsoDate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        IsoDate::parse(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("not an ISO 8601 date: {text:?}")))
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date for a day number since 1970-01-01 (H. Hinnant).
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn parse_rfc3339(text: &str) -> Option<i64> {
    let b = text.trim().as_bytes();
    let num = |s: &[u8]| -> Option<i64> {
        if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(s).ok()?.parse().ok()
    };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(&b[0..4])?, num(&b[5..7])?, num(&b[8..10])?);
    let (h, mi, s) = (num(&b[11..13])?, num(&b[14..16])?, num(&b[17..19])?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut rest = &b[19..];
    if rest.first() == Some(&b'.') {
        let digits = rest[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        rest = &rest[1 + digits..];
    }
    let offset = match rest {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), tail @ ..] => {
            let (oh, om) = match tail {
                [h1, h2, b':', m1, m2] => (num(&[*h1, *h2])?, num(&[*m1, *m2])?),
                [h1, h2, m1, m2] => (num(&[*h1, *h2])?, num(&[*m1, *m2])?),
                [h1, h2] => (num(&[*h1, *h2])?, 0),
                _ => return None,
            };
            let o = oh * 3600 + om * 60;
            if *sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo as u32, d as u32);
    Some(days * 86_400 + h * 3600 + mi * 60 + s - offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trip() {
        for s in [
            "2026-10-02T14:21:57Z",
            "1970-01-01T00:00:00Z",
            "2000-02-29T23:59:59Z",
        ] {
            assert_eq!(IsoDate::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(IsoDate::parse("1970-01-01T00:00:00Z").unwrap().0, 0);
        assert_eq!(
            IsoDate::parse("2026-10-07T02:08:58.424713Z")
                .unwrap()
                .to_string(),
            "2026-10-07T02:08:58Z"
        );
        assert_eq!(
            IsoDate::parse("2026-10-07T04:08:58+02:00")
                .unwrap()
                .to_string(),
            "2026-10-07T02:08:58Z"
        );
        assert!(IsoDate::parse("yesterday").is_none());
    }

    #[test]
    fn swift_reference() {
        // 2001-01-01T00:00:00Z
        assert_eq!(
            IsoDate::parse("2001-01-01T00:00:00Z").unwrap().0 as f64,
            SWIFT_REFERENCE_UNIX
        );
        let d = SwiftDate(813_521_284.427064);
        assert!((SwiftDate::from_unix(d.unix()).0 - d.0).abs() < 1e-6);
    }
}
