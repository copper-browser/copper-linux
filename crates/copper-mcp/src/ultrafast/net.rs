//! What a page asked its server (NetLog.swift's Jev part), on the page's own
//! log (`copper_pagejs::jev::NetEntry`, times in the page's ms).

use copper_pagejs::jev::NetEntry;

/// The calls a page makes for data, not for its looks.
pub const CALLS: [&str; 2] = ["xhr", "fetch"];

/// Analytics hosts: what they are sent is bookkeeping, never an answer.
pub const TRACKERS: &[&str] = &[
    "google-analytics.com",
    "googletagmanager.com",
    "doubleclick.net",
    "googlesyndication.com",
    "sentry.io",
    "segment.io",
    "segment.com",
    "mixpanel.com",
    "amplitude.com",
    "hotjar.com",
    "hotjar.io",
    "nr-data.net",
    "datadoghq.com",
    "browser-intake-datadoghq.com",
    "clarity.ms",
    "bat.bing.com",
    "fullstory.com",
    "facebook.net",
    "scorecardresearch.com",
    "amazon-adsystem.com",
    "fls-na.amazon.com",
];

/// How long a call that never said anything is waited for (an aborted xhr
/// may never end).
pub const EXPIRE_MS: f64 = 1500.0;

/// A page's own bookkeeping, never its answer: beacons and pings by kind,
/// the known analytics services by host, and Google's `log?format=json`.
/// Nothing by path words: a POST to a shop's /payments/collect is real.
pub fn noise(e: &NetEntry) -> bool {
    if ["beacon", "ping", "csp-report"].contains(&e.kind.as_str()) {
        return true;
    }
    let Ok(url) = url::Url::parse(&e.url) else {
        return false;
    };
    let Some(host) = url.host_str().map(str::to_lowercase) else {
        return false;
    };
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if TRACKERS.iter().any(|t| under(t)) {
        return true;
    }
    (under("google.com") || under("youtube.com"))
        && ["/log", "/gen_204", "/csi", "/jserror"].contains(&url.path().to_lowercase().as_str())
}

/// Still waited for: not back, no status yet, and asked less than
/// `expire_ms` ago. A call with a status is answered (a stream, a long poll).
pub fn pending(e: &NetEntry, now_ms: f64, expire_ms: f64) -> bool {
    !e.done && e.status.is_none() && now_ms - e.at < expire_ms
}

/// Nothing pending, and the last answer at least `linger_ms` ago (or no call at all).
pub fn quiet(calls: &[NetEntry], now_ms: f64, linger_ms: f64) -> bool {
    if calls.iter().any(|e| pending(e, now_ms, EXPIRE_MS)) {
        return false;
    }
    calls
        .iter()
        .filter_map(|e| e.answered_at)
        .reduce(f64::max)
        .is_none_or(|last| now_ms - last >= linger_ms)
}

/// The page's data calls after `after_seq`, less its bookkeeping.
pub fn data_calls(entries: &[NetEntry], after_seq: u64) -> Vec<NetEntry> {
    entries
        .iter()
        .filter(|e| e.seq > after_seq && CALLS.contains(&e.kind.as_str()) && !noise(e))
        .cloned()
        .collect()
}

/// Where the requests after `after_seq` that change things went: not
/// GET/HEAD/OPTIONS, not bookkeeping.
pub fn posts(entries: &[NetEntry], after_seq: u64) -> Vec<String> {
    entries
        .iter()
        .filter(|e| {
            e.seq > after_seq
                && (CALLS.contains(&e.kind.as_str()) || e.kind == "document")
                && !noise(e)
                && !["GET", "HEAD", "OPTIONS"].contains(&e.method.to_uppercase().as_str())
        })
        .map(|e| e.url.clone())
        .collect()
}

/// `200 GetShoppingResults`, `pending batchexecute`: a call for the trace.
pub fn brief(e: &NetEntry) -> String {
    let outcome = if e.error.is_some() {
        "failed".to_string()
    } else if !e.done {
        "pending".to_string()
    } else {
        e.status.map_or_else(|| "?".into(), |s| s.to_string())
    };
    let last = url::Url::parse(&e.url).map_or_else(
        |_| e.url.clone(),
        |u| {
            let path = u.path().trim_end_matches('/');
            let last = path.rsplit('/').next().unwrap_or("");
            if last.is_empty() {
                "/".into()
            } else {
                last.to_string()
            }
        },
    );
    format!("{outcome} {}", last.chars().take(48).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, method: &str, url: &str, kind: &str) -> NetEntry {
        NetEntry {
            seq,
            method: method.into(),
            url: url.into(),
            kind: kind.into(),
            ..NetEntry::default()
        }
    }

    #[test]
    fn waits_only_on_what_is_live() {
        let now = 10_000.0;
        let mut open = entry(
            1,
            "POST",
            "https://www.google.com/_/FlightsFrontendUi/data/batchexecute",
            "xhr",
        );
        open.at = now - 500.0;
        assert!(pending(&open, now, EXPIRE_MS));
        // An aborted call that never ends stops counting after 1.5 s.
        open.at = now - 2000.0;
        assert!(!pending(&open, now, EXPIRE_MS));
        // A call with a status is answered, open stream or not.
        let mut streaming = entry(2, "GET", "https://x.test/stream", "fetch");
        streaming.status = Some(200);
        streaming.at = now;
        assert!(!pending(&streaming, now, EXPIRE_MS));
        // Quiet: nothing pending and the last answer 150 ms ago.
        streaming.answered_at = Some(now - 50.0);
        assert!(!quiet(std::slice::from_ref(&streaming), now, 150.0));
        streaming.answered_at = Some(now - 200.0);
        assert!(quiet(std::slice::from_ref(&streaming), now, 150.0) && quiet(&[], now, 150.0));
    }

    #[test]
    fn bookkeeping_is_not_an_answer() {
        let n = |method: &str, url: &str, kind: &str| noise(&entry(1, method, url, kind));
        assert!(n(
            "POST",
            "https://www.google.com/log?format=json&hasfast=true",
            "xhr"
        ));
        assert!(n(
            "POST",
            "https://www.google-analytics.com/g/collect?v=2",
            "fetch"
        ));
        assert!(n("POST", "https://x.test/anything", "beacon"));
        assert!(!n(
            "POST",
            "https://www.google.com/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/GetShoppingResults",
            "xhr"
        ));
        assert!(!n("POST", "http://127.0.0.1:4171/api/review", "fetch"));
        // Path words and stats. hosts are not bookkeeping: these POSTs change things.
        assert!(!n("POST", "https://shop.test/payments/collect", "fetch"));
        assert!(!n("POST", "https://stats.shop.test/api/orders", "xhr"));
        assert!(!n("POST", "https://shop.test/track", "fetch"));
        assert!(n(
            "POST",
            "https://o1.ingest.sentry.io/api/1/envelope/",
            "fetch"
        ));
        assert!(n("GET", "https://www.google.com/gen_204?atyp=i", "xhr"));
    }

    #[test]
    fn data_calls_posts_and_briefs() {
        let mut shopping = entry(
            3,
            "POST",
            "https://www.google.com/_/FlightsFrontendUi/data/travel.frontend.flights.FlightsFrontendService/GetShoppingResults",
            "xhr",
        );
        let entries = vec![
            entry(1, "POST", "https://x.test/old", "fetch"),
            entry(2, "GET", "https://x.test/style.css", "stylesheet"),
            shopping.clone(),
            entry(4, "POST", "https://www.google.com/log?format=json", "xhr"),
            entry(5, "GET", "https://x.test/api/list", "fetch"),
            entry(6, "POST", "https://x.test/checkout", "document"),
        ];
        let seqs: Vec<u64> = data_calls(&entries, 1).iter().map(|e| e.seq).collect();
        assert_eq!(seqs, [3, 5]);
        assert_eq!(
            posts(&entries, 1),
            [shopping.url.clone(), "https://x.test/checkout".into()]
        );
        assert_eq!(brief(&shopping), "pending GetShoppingResults");
        shopping.done = true;
        shopping.status = Some(200);
        assert_eq!(brief(&shopping), "200 GetShoppingResults");
        shopping.error = Some("cancelled".into());
        assert_eq!(brief(&shopping), "failed GetShoppingResults");
    }
}
