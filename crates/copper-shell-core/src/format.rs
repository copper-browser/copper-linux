//! Numbers and dates the way the Mac panels say them: `5.7 of 10.5 MB ·
//! 588 KB/s · 8 s left`, `Today`, `2 h ago`.
//!
//! Dates are on the user's local calendar, which is the platform's to know
//! (time zone, month names in the user's language): the date functions take
//! a [`Calendar`] (GTK: GLib's `GDateTime`).

/// A moment on the user's local calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i32,
    /// 1–12.
    pub month: u32,
    /// Day of the month, 1–31.
    pub day: u32,
    /// Day of the year, 1–366.
    pub day_of_year: u32,
    /// 0–23.
    pub hour: u32,
    /// 0–59.
    pub minute: u32,
}

/// The user's local calendar and language.
pub trait Calendar {
    /// `unix` (seconds) in the local time zone; `None` when out of range.
    fn civil(unix: f64) -> Option<Civil>;
    /// The full name of `month` (1–12) in the user's language (`October`).
    fn month_name(month: u32) -> String;
}

/// Bytes like macOS's file-style `ByteCountFormatter`: decimal units,
/// whole KB, one decimal for MB, two for GB.
pub fn bytes(n: u64) -> String {
    let n = n as f64;
    if n < 1000.0 {
        return if n as u64 == 1 {
            "1 byte".into()
        } else {
            format!("{} bytes", n as u64)
        };
    }
    if n < 1e6 {
        return format!("{} KB", (n / 1e3).round() as u64);
    }
    if n < 1e9 {
        return format!("{:.1} MB", n / 1e6);
    }
    if n < 1e12 {
        return format!("{:.2} GB", n / 1e9);
    }
    format!("{:.2} TB", n / 1e12)
}

/// `4.2 of 18.6 MB`: the unit once, when both figures share it.
pub fn part_of(done: u64, total: u64) -> String {
    let whole = bytes(total);
    let part = bytes(done);
    let unit = whole.rsplit(' ').next().unwrap_or("");
    match part.strip_suffix(unit) {
        Some(n) if !unit.is_empty() && unit != "bytes" => format!("{} of {whole}", n.trim()),
        _ => format!("{part} of {whole}"),
    }
}

/// A rate, or a dash before there is one.
pub fn speed(per_second: f64) -> String {
    if per_second > 0.0 {
        format!("{}/s", bytes(per_second as u64))
    } else {
        "—".into()
    }
}

/// `12 s`, `2 min`, `1 h 05 min` — round figures.
pub fn duration(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s < 60 {
        return format!("{s} s");
    }
    if s < 3600 {
        return format!("{} min", (s + 30) / 60);
    }
    format!("{} h {:02} min", s / 3600, (s % 3600) / 60)
}

/// The figures under a download.
pub fn download_line(d: &copper_core::downloads::LiveDownload, from: &str) -> String {
    use copper_core::downloads::DownloadState;
    match &d.state {
        DownloadState::Active => {
            let rate = speed(d.speed);
            let Some(total) = d.total else {
                return format!("{} so far · {rate}", bytes(d.received));
            };
            let left = if d.speed > 1.0 {
                format!(
                    "{} left",
                    duration(total.saturating_sub(d.received) as f64 / d.speed)
                )
            } else {
                "starting".into()
            };
            format!("{} · {rate} · {left}", part_of(d.received, total))
        }
        DownloadState::Finished => {
            let size = bytes(d.total.unwrap_or(d.received).max(d.received));
            if from.is_empty() {
                format!("{size} · Downloaded")
            } else {
                format!("{size} · {from}")
            }
        }
        DownloadState::Failed { .. } => {
            if d.received > 0 {
                format!("Failed at {}", bytes(d.received))
            } else {
                "Failed".into()
            }
        }
        DownloadState::Cancelled => "Cancelled".into(),
    }
}

/// `3 min ago`, `2 h ago`, `yesterday`, `5 days ago`.
pub fn ago<C: Calendar>(unix: f64, now: f64) -> String {
    let d = (now - unix).max(0.0);
    if d < 60.0 {
        return "just now".into();
    }
    if d < 3600.0 {
        return format!("{} min ago", (d / 60.0) as u64);
    }
    if d < 86400.0 {
        return format!("{} h ago", (d / 3600.0) as u64);
    }
    let days = (d / 86400.0) as u64;
    if days == 1 {
        return "yesterday".into();
    }
    if days < 30 {
        return format!("{days} days ago");
    }
    day::<C>(unix)
}

/// The time of day (`15:41`).
pub fn clock<C: Calendar>(unix: f64) -> String {
    C::civil(unix)
        .map(|d| format!("{:02}:{:02}", d.hour, d.minute))
        .unwrap_or_default()
}

/// The local calendar day of `unix`, as a sortable number.
pub fn day_number<C: Calendar>(unix: f64) -> i64 {
    C::civil(unix)
        .map(|d| i64::from(d.year) * 1000 + i64::from(d.day_of_year))
        .unwrap_or(0)
}

/// `Today`, `Yesterday`, or `12 October` (`12 October 2025` in another year).
pub fn day<C: Calendar>(unix: f64) -> String {
    let now = copper_core::date::now_unix();
    let (d, today) = (day_number::<C>(unix), day_number::<C>(now));
    if d == today {
        return "Today".into();
    }
    if d == day_number::<C>(now - 86400.0) {
        return "Yesterday".into();
    }
    let Some(t) = C::civil(unix) else {
        return String::new();
    };
    let month = C::month_name(t.month);
    if C::civil(now).map(|n| n.year) == Some(t.year) {
        format!("{} {month}", t.day)
    } else {
        format!("{} {month} {}", t.day, t.year)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTC with English month names, enough to test the wording.
    enum Utc {}

    impl Calendar for Utc {
        fn civil(unix: f64) -> Option<Civil> {
            let secs = unix.floor() as i64;
            let (days, rest) = (secs.div_euclid(86400), secs.rem_euclid(86400));
            // Howard Hinnant's days-to-civil.
            let z = days + 719_468;
            let era = z.div_euclid(146_097);
            let doe = z - era * 146_097;
            let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
            let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
            let mp = (5 * doy + 2) / 153;
            let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
            let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
            let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            let before =
                [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][month as usize - 1];
            let day_of_year = before + day + u32::from(leap && month > 2);
            Some(Civil {
                year,
                month,
                day,
                day_of_year,
                hour: (rest / 3600) as u32,
                minute: (rest % 3600 / 60) as u32,
            })
        }

        fn month_name(month: u32) -> String {
            const NAMES: [&str; 12] = [
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December",
            ];
            NAMES[month as usize - 1].into()
        }
    }

    #[test]
    fn dates_on_the_calendar() {
        // 2024-03-01 15:41 UTC, a leap year.
        let t = 1_709_307_660.0;
        assert_eq!(clock::<Utc>(t), "15:41");
        assert_eq!(day_number::<Utc>(t), 2_024_061);
        assert_eq!(day::<Utc>(t), "1 March 2024");
        assert_eq!(ago::<Utc>(t, t + 86400.0 * 40.0), "1 March 2024");
        let now = copper_core::date::now_unix();
        assert_eq!(day::<Utc>(now), "Today");
        assert_eq!(day::<Utc>(now - 86400.0), "Yesterday");
    }

    #[test]
    fn bytes_like_the_mac() {
        assert_eq!(bytes(0), "0 bytes");
        assert_eq!(bytes(4), "4 bytes");
        assert_eq!(bytes(588_000), "588 KB");
        assert_eq!(bytes(5_700_000), "5.7 MB");
        assert_eq!(bytes(1_230_000_000), "1.23 GB");
        assert_eq!(part_of(4_200_000, 18_600_000), "4.2 of 18.6 MB");
        assert_eq!(part_of(900_000, 18_600_000), "900 KB of 18.6 MB");
        assert_eq!(part_of(3, 4), "3 bytes of 4 bytes");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(8.2), "8 s");
        assert_eq!(duration(95.0), "2 min");
        assert_eq!(duration(3900.0), "1 h 05 min");
        assert_eq!(speed(0.0), "—");
        assert_eq!(speed(588_000.0), "588 KB/s");
    }

    #[test]
    fn ago_words() {
        assert_eq!(ago::<Utc>(1000.0, 1030.0), "just now");
        assert_eq!(ago::<Utc>(1000.0, 1000.0 + 600.0), "10 min ago");
        assert_eq!(ago::<Utc>(0.0, 7200.0), "2 h ago");
        assert_eq!(ago::<Utc>(0.0, 86400.0 * 1.5), "yesterday");
        assert_eq!(ago::<Utc>(0.0, 86400.0 * 5.0), "5 days ago");
    }
}
