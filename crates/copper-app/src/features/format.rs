//! Numbers and dates the way the Mac panels say them: `5.7 of 10.5 MB ·
//! 588 KB/s · 8 s left`, `Today`, `2 h ago`.

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
pub fn ago(unix: f64, now: f64) -> String {
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
    day(unix)
}

fn local(unix: f64) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(unix as i64).ok()
}

/// The time of day (`15:41`).
pub fn clock(unix: f64) -> String {
    local(unix)
        .and_then(|d| d.format("%H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// The local calendar day of `unix`, as a sortable number.
pub fn day_number(unix: f64) -> i64 {
    local(unix)
        .map(|d| d.year() as i64 * 1000 + d.day_of_year() as i64)
        .unwrap_or(0)
}

/// `Today`, `Yesterday`, or `12 October`.
pub fn day(unix: f64) -> String {
    let now = copper_core::date::now_unix();
    let (d, today) = (day_number(unix), day_number(now));
    if d == today {
        return "Today".into();
    }
    if d == day_number(now - 86400.0) {
        return "Yesterday".into();
    }
    local(unix)
        .and_then(|t| {
            let same_year = local(now).map(|n| n.year()) == Some(t.year());
            t.format(if same_year { "%-d %B" } else { "%-d %B %Y" })
                .ok()
        })
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(ago(1000.0, 1030.0), "just now");
        assert_eq!(ago(1000.0, 1000.0 + 600.0), "10 min ago");
        assert_eq!(ago(0.0, 7200.0), "2 h ago");
        assert_eq!(ago(0.0, 86400.0 * 1.5), "yesterday");
        assert_eq!(ago(0.0, 86400.0 * 5.0), "5 days ago");
    }
}
