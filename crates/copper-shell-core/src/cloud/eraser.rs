//! Settings › Cloud › Delete history on cloud, without a window (Mac:
//! `CloudHistoryEraser.swift`, `CloudHistoryDeleteCard`): the time range,
//! the site typed, the question asked before anything goes, and what the
//! card says while and after deleting. Each shell draws the card and runs
//! [`copper_cloud::SyncEngine::delete_history_until`] with [`selector`];
//! history on this device is never touched.
//!
//! Words are the Mac's with its platform nouns made portable ("this
//! device", "every device"). Lifted from copper-win's `cloud/model.rs`
//! with the same names, so Windows can `use` this module in its place.

use copper_cloud::Failure;
use copper_cloud::sync::Selector;
use copper_cloud::sync::delete::{self, Outcome};

/// The card's title (and its caption).
pub const TITLE: &str = "Delete history on cloud";
/// Under "Time range".
pub const RANGE_DETAIL: &str = "Visits on Copper Cloud, from every device signed in as you";
/// Under "Only this site".
pub const SITE_DETAIL: &str = "Optional; its subdomains too. Found by reading your history on this device — the cloud never looks inside it.";
/// While the cloud is asked whether it can delete history.
pub const ASKING: &str = "Asking the cloud what it can do…";
/// The cloud couldn't be asked.
pub const UNREACHABLE: &str = "Couldn't reach the cloud to ask whether it can delete history.";
/// The cloud can't delete history.
pub use copper_cloud::sync::delete::UNSUPPORTED;

/// The time range (`CloudHistoryEraser.Span`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Span {
    #[default]
    Hour,
    Day,
    Week,
    All,
}

impl Span {
    pub const ALL: [Span; 4] = [Span::Hour, Span::Day, Span::Week, Span::All];

    pub fn title(self) -> &'static str {
        match self {
            Span::Hour => "Last hour",
            Span::Day => "Last 24 hours",
            Span::Week => "Last 7 days",
            Span::All => "All time",
        }
    }

    pub fn seconds(self) -> Option<f64> {
        match self {
            Span::Hour => Some(3_600.0),
            Span::Day => Some(86_400.0),
            Span::Week => Some(604_800.0),
            Span::All => None,
        }
    }

    /// "the last hour"; `None` for all time.
    pub fn words(self) -> Option<String> {
        self.seconds()
            .map(|_| format!("the {}", self.title().to_lowercase()))
    }
}

/// The selector for a range and maybe a site: no site and all time is
/// everything — said, never assumed (`CloudHistoryEraser.selector`).
pub fn selector(span: Span, site: Option<&str>, now: f64) -> Selector {
    let mut s = Selector {
        since: span.seconds().map(|secs| now - secs),
        ..Selector::default()
    };
    if let Some(site) = site {
        s.hosts = vec![site.to_string()];
    }
    s.everything = !s.narrowed();
    s
}

/// The site typed, as it will be matched (`CloudHistoryDelete.site`);
/// `Err(())` when something is typed that isn't a site.
#[allow(clippy::result_unit_err)]
pub fn typed_site(typed: &str) -> Result<Option<String>, ()> {
    if typed.trim().is_empty() {
        return Ok(None);
    }
    delete::site(typed).map(Some).ok_or(())
}

/// The confirmation's question (`question`).
pub fn delete_question(span: Span, site: Option<&str>) -> String {
    match (site, span.words()) {
        (None, None) => "Delete all of your history on the cloud?".into(),
        (None, Some(w)) => format!("Delete {w} of history on the cloud?"),
        (Some(s), None) => format!("Delete every visit to {s} on the cloud?"),
        (Some(s), Some(w)) => format!("Delete visits to {s} from {w} on the cloud?"),
    }
}

/// The confirmation's message.
pub const DELETE_MESSAGE: &str = "Copper Cloud deletes them for every device signed in as you. History on this device — and what your other devices already have — stays as it is. This can't be undone.";

/// What a finished delete says.
pub fn deleted_text(deleted: usize, cut: bool) -> String {
    let mut s = format!(
        "Deleted {} visit{} from the cloud",
        group_digits(deleted),
        if deleted == 1 { "" } else { "s" }
    );
    if cut {
        let looked = delete::PAGE_LIMIT * delete::PAGE_SIZE;
        s.push_str(&format!(
            " — only the oldest {} were looked through",
            group_digits(looked)
        ));
    }
    s
}

/// A delete stopped while deleting: what went before the stop (the rest of
/// the matches stayed).
pub fn stopped_text(deleted: usize) -> String {
    format!(
        "Stopped after deleting {} visit{} from the cloud",
        group_digits(deleted),
        if deleted == 1 { "" } else { "s" }
    )
}

/// How a delete ended: `Ok` is the card's result line, `Err` its problem.
/// Stopped before anything went is a result, not a problem (Mac).
pub fn finished(r: Result<Outcome, Failure>) -> Result<String, String> {
    match r {
        Ok(o) if o.stopped => Ok(stopped_text(o.deleted)),
        Ok(o) => Ok(deleted_text(o.deleted, o.cut)),
        Err(f) if f.code == "stopped" => Ok(f.message),
        Err(f) => Err(f.message),
    }
}

/// 1234567 → "1,234,567" (`formatted()`).
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The delete card's status line (`status`).
pub fn delete_status(bad_site: bool, working: bool, looking: bool, result: Option<&str>) -> String {
    if bad_site {
        "That isn't a site — try example.com".into()
    } else if working {
        if looking {
            "Looking through your history on this device…".into()
        } else {
            "Deleting…".into()
        }
    } else {
        result
            .unwrap_or("History on this device stays as it is.")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_delete() {
        let now = 1_000_000.0;
        let all = selector(Span::All, None, now);
        assert!(all.everything && all.valid());
        let hour = selector(Span::Hour, None, now);
        assert_eq!(hour.since, Some(now - 3_600.0));
        assert!(!hour.everything && hour.valid());
        let site = selector(Span::All, Some("example.com"), now);
        assert!(site.scans() && !site.everything && site.valid());
        assert_eq!(typed_site(""), Ok(None));
        assert_eq!(typed_site("Example.com"), Ok(Some("example.com".into())));
        assert_eq!(typed_site("not a site"), Err(()));
        assert_eq!(
            delete_question(Span::All, None),
            "Delete all of your history on the cloud?"
        );
        assert_eq!(
            delete_question(Span::Day, None),
            "Delete the last 24 hours of history on the cloud?"
        );
        assert_eq!(
            delete_question(Span::All, Some("x.com")),
            "Delete every visit to x.com on the cloud?"
        );
        assert_eq!(
            delete_question(Span::Week, Some("x.com")),
            "Delete visits to x.com from the last 7 days on the cloud?"
        );
        assert_eq!(deleted_text(1, false), "Deleted 1 visit from the cloud");
        assert_eq!(
            deleted_text(1234, false),
            "Deleted 1,234 visits from the cloud"
        );
        assert!(deleted_text(2, true).contains("only the oldest 500,000 were looked through"));
        assert_eq!(
            stopped_text(5000),
            "Stopped after deleting 5,000 visits from the cloud"
        );
        assert_eq!(
            stopped_text(1),
            "Stopped after deleting 1 visit from the cloud"
        );
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1000), "1,000");
        assert_eq!(
            delete_status(true, false, false, None),
            "That isn't a site — try example.com"
        );
        assert_eq!(
            delete_status(false, true, true, None),
            "Looking through your history on this device…"
        );
        assert_eq!(delete_status(false, true, false, None), "Deleting…");
        assert_eq!(
            delete_status(false, false, false, None),
            "History on this device stays as it is."
        );
        assert_eq!(delete_status(false, false, false, Some("Done")), "Done");
    }

    #[test]
    fn finished_words() {
        let done = Outcome {
            deleted: 3,
            ..Outcome::default()
        };
        assert_eq!(
            finished(Ok(done.clone())),
            Ok("Deleted 3 visits from the cloud".into())
        );
        let stopped = Outcome {
            stopped: true,
            ..done
        };
        assert_eq!(
            finished(Ok(stopped)),
            Ok("Stopped after deleting 3 visits from the cloud".into())
        );
        assert_eq!(
            finished(Err(delete::Stop::failure())),
            Ok(delete::STOPPED.into())
        );
        assert_eq!(
            finished(Err(Failure::new(0, "network", "offline"))),
            Err("offline".into())
        );
    }
}
