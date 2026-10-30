//! The Today sweep (Mac: `Fork/Sections.swift` `sweep`; Settings › Tabs ›
//! "Archive Today after", `sections.archive`): a Today row nobody has
//! looked at for the window (12, 24 or 48 hours; never) is closed the
//! ordinary way, so it waits in Reopen Closed Tab.
//!
//! Never the row on screen, a pin, a Saved row (a row in a folder is
//! saved), a blank tab or a canvas. Swept on launch, every half hour, and
//! when a space comes on screen — only the current space, so the spaces you
//! are not looking at are not swept behind your back
//! ([`crate::Shell::archive_today`]).

use copper_core::browser::Tab;
use copper_core::{TabId, UnixTime};
use std::time::Duration;

/// The first sweep, once the session is back (`Sections.begin`).
pub const FIRST: Duration = Duration::from_secs(2);
/// And every half hour after.
pub const EVERY: Duration = Duration::from_secs(30 * 60);

/// Tab `t` goes in this sweep: a Today row not looked at for `hours`.
pub fn stale(t: &Tab, active: Option<&TabId>, now: UnixTime, hours: f64) -> bool {
    !t.is_pinned()
        && !t.saved
        && t.group.is_none()
        && active != Some(&t.id)
        && !crate::clear::is_blank(&t.url)
        && crate::canvas::id_from_url(&t.url).is_none()
        && now.0 - t.seen.0 > hours * 3600.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str, hours_ago: f64, now: UnixTime) -> Tab {
        Tab {
            id: TabId::new(),
            url: url.into(),
            title: String::new(),
            favicon: None,
            pin: None,
            space: "s".into(),
            group: None,
            saved: false,
            seen: UnixTime(now.0 - hours_ago * 3600.0),
            split: None,
            sleeping: true,
            private: false,
            extra: Default::default(),
        }
    }

    #[test]
    fn only_old_loose_today_rows_go() {
        let now = UnixTime(1_800_000_000.0);
        let old = tab("https://a.test/", 25.0, now);
        assert!(stale(&old, None, now, 24.0));
        assert!(!stale(&old, None, now, 48.0));
        assert!(!stale(&tab("https://a.test/", 23.0, now), None, now, 24.0));
        assert!(!stale(&old, Some(&old.id), now, 24.0), "the row on screen");
        let kept = [
            Tab {
                pin: Some("A".into()),
                ..old.clone()
            },
            Tab {
                saved: true,
                ..old.clone()
            },
            Tab {
                group: Some("g".into()),
                ..old.clone()
            },
            tab("", 25.0, now),
            tab("about:blank", 25.0, now),
            tab("copper://canvas/abc", 25.0, now),
        ];
        for t in kept {
            assert!(!stale(&t, None, now, 24.0), "{} must stay", t.url);
        }
    }
}
