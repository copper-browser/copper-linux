//! A page's web process went away: the record in `crashes.json`
//! (Mac: `Fork/Crashes.swift`). The page itself shows "This page stopped"
//! with Reload ([`crate::shell::pages::PageSlot::show_crash`]).
//!
//! The app survived, so the entry is a `fault` (the Mac's word for a crash
//! it lived through), with `source: "web-process"` and the reason.

use crate::shell::Shell;
use copper_core::TabId;
use copper_core::crashes::{CrashEntry, CrashIndex};
use copper_core::date::IsoDate;
use copper_engine::CrashReason;

/// How many entries `crashes.json` keeps.
pub const KEEP: usize = 100;

/// The entry for one web-process crash.
pub fn entry(url: &str, reason: CrashReason, now: IsoDate) -> CrashEntry {
    let host = crate::shell::host_of(url);
    CrashEntry {
        id: format!("web-{}-{}", now.unix(), std::process::id()),
        kind: "fault".into(),
        time: now,
        source: Some("web-process".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        os: Some(os_name()),
        reason: Some(
            match reason {
                CrashReason::Crashed => "crashed",
                CrashReason::ExceededMemoryLimit => "exceeded-memory-limit",
                CrashReason::TerminatedByApi => "terminated",
            }
            .into(),
        ),
        // The host, never the whole address: a crash log is shared.
        trail: (!host.is_empty()).then(|| vec![format!("page {host}")]),
        ..Default::default()
    }
}

/// Add `entry` to an index, newest last, keeping [`KEEP`].
pub fn append(index: &mut CrashIndex, entry: CrashEntry) {
    index.entries.push(entry);
    if index.entries.len() > KEEP {
        let drop = index.entries.len() - KEEP;
        index.entries.drain(..drop);
    }
}

/// Record a crash for `tab` (reads the file, appends, saves off the main
/// thread).
pub fn record(shell: &Shell, _tab: &TabId, url: &str, reason: CrashReason) {
    let mut index = match shell.store().load::<CrashIndex>() {
        Ok(l) => l.value,
        Err(e) => {
            tracing::warn!(error = %e, "crashes.json unreadable; starting a new one");
            CrashIndex::default()
        }
    };
    append(&mut index, entry(url, reason, IsoDate::now()));
    shell.saver().schedule(index);
    tracing::warn!(?reason, "web process crashed");
}

fn os_name() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        })
        .unwrap_or_else(|| "Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_faults_without_the_full_address() {
        let e = entry("https://mail.example.com/inbox?secret=1", CrashReason::Crashed, IsoDate(1_700_000_000));
        assert_eq!(e.kind, "fault");
        assert!(!e.fatal());
        assert_eq!(e.reason.as_deref(), Some("crashed"));
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("secret"), "{json}");
        assert!(json.contains("mail.example.com"));
    }

    #[test]
    fn index_is_capped_and_round_trips_with_the_mac_file() {
        let mac = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/mac/crashes.json")).unwrap();
        let mut index: CrashIndex = serde_json::from_slice(&mac).unwrap();
        let before = index.entries.len();
        append(&mut index, entry("https://a.b", CrashReason::ExceededMemoryLimit, IsoDate::now()));
        assert_eq!(index.entries.len(), (before + 1).min(KEEP));
        for _ in 0..(KEEP + 5) {
            append(&mut index, entry("https://a.b", CrashReason::Crashed, IsoDate::now()));
        }
        assert_eq!(index.entries.len(), KEEP);
        let again: CrashIndex = serde_json::from_str(&serde_json::to_string(&index).unwrap()).unwrap();
        assert_eq!(again.entries.len(), KEEP);
    }
}
