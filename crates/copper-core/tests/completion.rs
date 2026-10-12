//! Field completion ranking (a port of `History.suggestions` /
//! `History.completion`), plus open-tab rows.

use copper_core::date::SwiftDate;
use copper_core::history::{History, Suggestion, SuggestionKind, completion};
use copper_core::{Browser, OpenOptions};

const DAY: f64 = 86_400.0;

fn now() -> SwiftDate {
    SwiftDate(800_000_000.0)
}

fn ago(days: f64) -> SwiftDate {
    SwiftDate(now().0 - days * DAY)
}

fn history() -> History {
    let mut h = History::new();
    for _ in 0..5 {
        h.record_at("https://github.com/rust-lang/rust", "Rust", ago(1.0));
    }
    h.record_at("https://www.gitlab.com/", "GitLab", ago(60.0));
    h.record_at("https://cloud-ems.dev.exowatt.test/site/1", "EMS", ago(2.0));
    h.record_at("https://example.test/blog/post", "A blog post", ago(0.5));
    h.record_at("https://digit.test/", "Digit", ago(0.1));
    h
}

fn keys(rows: &[Suggestion]) -> Vec<&str> {
    rows.iter().map(|r| r.key.as_str()).collect()
}

#[test]
fn front_door_before_the_room_inside() {
    let h = history();
    let rows = h.suggestions_at("git", 5, now());
    // Visiting github.com/rust-lang/rust credited github.com five times too.
    assert_eq!(rows[0].key, "github.com");
    assert_eq!(rows[1].key, "github.com/rust-lang/rust");
    assert!(
        rows.iter()
            .all(|r| r.kind == SuggestionKind::Visited || r.kind == SuggestionKind::Known)
    );
    assert!(keys(&rows).contains(&"gitlab.com"));
    // Visited github.com means the known-sites "github.com" is not offered twice.
    assert_eq!(
        keys(&rows).iter().filter(|k| **k == "github.com").count(),
        1
    );
}

#[test]
fn recent_and_frequent_beats_old() {
    let mut h = History::new();
    h.record_at("https://alpha.test/", "Old", ago(120.0));
    h.record_at("https://alps.test/", "New", ago(0.0));
    let rows = h.suggestions_at("al", 5, now());
    assert_eq!(keys(&rows)[..2], ["alps.test", "alpha.test"]);
}

#[test]
fn host_labels_match_but_paths_do_not() {
    let h = history();
    // "exo" starts a host label of cloud-ems.dev.exowatt.test.
    let rows = h.suggestions_at("exo", 5, now());
    assert!(keys(&rows).contains(&"cloud-ems.dev.exowatt.test"));
    // "blog" is only in a path: no match.
    assert!(
        h.suggestions_at("blog", 5, now())
            .iter()
            .all(|r| r.kind == SuggestionKind::Known)
    );
    // One letter inside a host is not a match ("i" in digit.test)…
    assert!(!keys(&h.suggestions_at("i", 10, now())).contains(&"digit.test"));
    // …two letters are.
    assert!(keys(&h.suggestions_at("gi", 10, now())).contains(&"digit.test"));
}

#[test]
fn known_sites_only_where_history_is_silent() {
    let h = History::new();
    let rows = h.suggestions_at("lin", 5, now());
    assert_eq!(rows[0].key, "linear.app");
    assert_eq!(rows[0].kind, SuggestionKind::Known);
    assert!(keys(&rows).contains(&"linkedin.com"));
    let mut h = History::new();
    h.record_at("https://linode.test/", "Linode", ago(300.0));
    let rows = h.suggestions_at("lin", 5, now());
    assert_eq!(
        rows[0].key, "linode.test",
        "anything visited outranks a known site"
    );
}

#[test]
fn empty_field_proposes_nothing() {
    assert!(history().suggestions_at("   ", 5, now()).is_empty());
}

#[test]
fn inline_completion() {
    let h = history();
    let rows = h.suggestions_at("https://www.Git", 5, now());
    assert_eq!(
        completion("https://www.Git", &rows).as_deref(),
        Some("hub.com")
    );
    assert_eq!(completion("g", &rows), None, "needs two characters");
    assert_eq!(
        completion("github.com", &rows).as_deref(),
        Some("/rust-lang/rust")
    );
    assert_eq!(completion("zz", &rows), None);
}

#[test]
fn open_tabs_and_history_together() {
    let mut b = Browser::new();
    let id = b.open_tab(
        "https://github.com/pulls",
        OpenOptions {
            title: "Pull requests".into(),
            ..Default::default()
        },
    );
    b.open_tab(
        "https://secret.test/",
        OpenOptions {
            title: "Private".into(),
            private: true,
            ..Default::default()
        },
    );
    let tabs = b.open_tab_suggestions("git", 5);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].kind, SuggestionKind::Open);
    assert_eq!(tabs[0].tab.as_ref(), Some(&id));
    assert!(
        b.open_tab_suggestions("secret", 5).is_empty(),
        "private tabs are not offered"
    );
    let titled = b.open_tab_suggestions("pull", 5);
    assert_eq!(titled[0].tab.as_ref(), Some(&id));
}

#[test]
fn history_file_cap_and_order() {
    let h = history();
    let file = h.to_file(now());
    assert_eq!(file.len(), h.len());
    // Most frecent first: github.com (5 visits yesterday) leads.
    assert_eq!(file[0].key, "github.com");
    let back = History::from_file(file);
    assert_eq!(back.len(), h.len());
    // Search hides untitled front-door credits and is newest first.
    let traces = back.search("");
    assert!(
        traces
            .iter()
            .all(|t| !(t.title.is_empty() && !t.key.contains('/')))
    );
    assert_eq!(traces[0].key, "digit.test");
    let top = back.top_sites(3, now());
    assert_eq!(top[0].key, "github.com");
}
