//! What the field and the card offer for what was typed — no widgets, so
//! the ranking is unit-tested (Mac: `Omnibox.swift`, `Fork/CommandBar.swift`
//! `offers`).
//!
//! Nothing here touches the network: rows come from open tabs, history,
//! bookmarks and commands already in memory, and the "Search for …" row is
//! only an address until Return is pressed.

use copper_core::TabId;
use copper_core::address;
use copper_core::bookmarks::BookmarkHit;
use copper_core::history::{self, Suggestion};

/// What Return does with a row.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Load this address.
    Go(String),
    /// Select this tab.
    Switch(TabId),
    /// Run this command.
    Command(String),
}

/// What a row is, for its mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The typed text itself, as an address.
    Address,
    /// The typed text, searched for.
    Search,
    Tab,
    Bookmark,
    Visited,
    Command,
}

/// One row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub title: String,
    /// The quiet second part (address, or the command's shortcut).
    pub detail: String,
    /// What Return does, said on the right ("Switch to Tab").
    pub hint: String,
    pub action: Action,
    /// For a site mark: the page's address.
    pub url: Option<String>,
    /// A symbolic icon (commands).
    pub icon: Option<String>,
}

/// A command as the palette sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandRow {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub shortcut: Option<String>,
}

/// An open tab as the palette sees it (empty field: these, in order).
#[derive(Debug, Clone, PartialEq)]
pub struct OpenRow {
    pub id: TabId,
    pub title: String,
    pub url: String,
    /// Another space's name, when it is not in this one.
    pub elsewhere: Option<String>,
}

/// Which card is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Editing the selected tab's address (`Ctrl+L`).
    Edit,
    /// Where a new tab should go (`Ctrl+T`).
    Launch,
    /// `Ctrl+K`: tabs, commands, places.
    Palette,
}

/// Everything the ranking reads.
#[derive(Debug, Clone, Default)]
pub struct Inputs<'a> {
    /// What was typed (without an offered completion).
    pub typed: &'a str,
    /// What the field holds (with it); empty means the same as `typed`.
    pub full: &'a str,
    /// The search engine's name ("Google").
    pub engine: &'a str,
    /// Where Return would go with the typed text (address or search).
    pub destination: Option<String>,
    /// The typed text is an address rather than words.
    pub is_address: bool,
    /// Open-tab rows (`Browser::open_tab_suggestions`) — this space first.
    pub tabs: Vec<Suggestion>,
    pub visited: Vec<Suggestion>,
    pub bookmarks: Vec<BookmarkHit>,
    pub commands: Vec<CommandRow>,
    /// Empty field in the palette: every open tab.
    pub open: Vec<OpenRow>,
}

/// How many rows a card draws (Mac: `CommandBar.limit`).
pub const LIMIT: usize = 8;

/// The rows for `inputs`, best first.
pub fn rows(purpose: Purpose, inputs: &Inputs<'_>) -> Vec<Row> {
    let typed = inputs.typed.trim();
    let mut out: Vec<Row> = Vec::new();
    if typed.is_empty() {
        if purpose == Purpose::Palette {
            for t in inputs.open.iter().take(LIMIT) {
                out.push(Row {
                    kind: Kind::Tab,
                    title: if t.title.trim().is_empty() {
                        address::pretty_str(&t.url)
                    } else {
                        t.title.clone()
                    },
                    detail: t
                        .elsewhere
                        .clone()
                        .unwrap_or_else(|| address::pretty_str(&t.url)),
                    hint: "Switch to Tab".into(),
                    action: Action::Switch(t.id.clone()),
                    url: Some(t.url.clone()),
                    icon: None,
                });
            }
        }
        return out;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut add = |out: &mut Vec<Row>, row: Row| {
        let key = row
            .url
            .as_deref()
            .map(address::strip_typed)
            .unwrap_or_else(|| row.title.to_lowercase());
        if row.kind != Kind::Command && seen.contains(&key) {
            return;
        }
        seen.push(key);
        out.push(row);
    };

    // What Return does with the text itself comes first, except in the
    // palette, where an open tab wins (Ctrl+K, Return = back to that page).
    let typed_row = inputs.destination.as_ref().map(|dest| {
        // The text is a place already known (completed from history or an
        // open tab): go to that place exactly — its scheme and port, which
        // the shown form leaves out — under its title.
        if inputs.is_address
            && let Some(known) = known_place(
                if inputs.full.trim().is_empty() {
                    typed
                } else {
                    inputs.full
                },
                &inputs.tabs,
                &inputs.visited,
            )
        {
            return Row {
                kind: Kind::Address,
                title: if known.title.trim().is_empty() {
                    address::pretty_str(&known.url)
                } else {
                    known.title.clone()
                },
                detail: address::pretty_str(&known.url),
                hint: if purpose == Purpose::Edit {
                    "Go"
                } else {
                    "Open"
                }
                .into(),
                action: Action::Go(known.url.clone()),
                url: Some(known.url.clone()),
                icon: None,
            };
        }
        if inputs.is_address {
            Row {
                kind: Kind::Address,
                title: address::pretty_str(dest),
                detail: String::new(),
                hint: if purpose == Purpose::Edit {
                    "Go"
                } else {
                    "Open"
                }
                .into(),
                action: Action::Go(dest.clone()),
                url: Some(dest.clone()),
                icon: None,
            }
        } else {
            Row {
                kind: Kind::Search,
                title: typed.to_string(),
                detail: format!("Search {}", inputs.engine),
                hint: "Search".into(),
                action: Action::Go(dest.clone()),
                url: None,
                icon: None,
            }
        }
    });

    if purpose != Purpose::Palette
        && let Some(r) = typed_row.clone()
    {
        add(&mut out, r);
    }
    for s in inputs.tabs.iter().take(3) {
        let Some(id) = s.tab.clone() else { continue };
        add(
            &mut out,
            Row {
                kind: Kind::Tab,
                title: if s.title.trim().is_empty() {
                    address::pretty_str(&s.url)
                } else {
                    s.title.clone()
                },
                detail: address::pretty_str(&s.url),
                hint: "Switch to Tab".into(),
                action: Action::Switch(id),
                url: Some(s.url.clone()),
                icon: None,
            },
        );
    }
    if purpose == Purpose::Palette {
        let needle = typed.trim_start_matches('/').to_lowercase();
        for c in inputs
            .commands
            .iter()
            .filter(|c| command_matches(&c.name, &needle))
            .take(4)
        {
            add(
                &mut out,
                Row {
                    kind: Kind::Command,
                    title: c.name.clone(),
                    detail: c.shortcut.clone().unwrap_or_default(),
                    hint: "Run".into(),
                    action: Action::Command(c.id.clone()),
                    url: None,
                    icon: Some(c.icon.clone()),
                },
            );
        }
    }
    for b in inputs.bookmarks.iter().take(3) {
        let Some(url) = b.bookmark.url.clone() else {
            continue;
        };
        add(
            &mut out,
            Row {
                kind: Kind::Bookmark,
                title: if b.bookmark.title.is_empty() {
                    address::pretty_str(&url)
                } else {
                    b.bookmark.title.clone()
                },
                detail: address::pretty_str(&url),
                hint: "Open".into(),
                action: Action::Go(url.clone()),
                url: Some(url),
                icon: None,
            },
        );
    }
    for v in inputs.visited.iter().take(6) {
        add(
            &mut out,
            Row {
                kind: Kind::Visited,
                title: if v.title.trim().is_empty() {
                    v.key.clone()
                } else {
                    v.title.clone()
                },
                detail: address::pretty_str(&v.url),
                hint: "Open".into(),
                action: Action::Go(v.url.clone()),
                url: Some(v.url.clone()),
                icon: None,
            },
        );
    }
    if purpose == Purpose::Palette
        && let Some(r) = typed_row
    {
        add(&mut out, r);
    }
    out.truncate(LIMIT);
    out
}

/// The open tab or visit whose shown address is exactly `typed`.
pub fn known_place<'a>(
    typed: &str,
    tabs: &'a [Suggestion],
    visited: &'a [Suggestion],
) -> Option<&'a Suggestion> {
    let want = address::strip_typed(typed);
    let want = want.trim_end_matches('/');
    if want.is_empty() {
        return None;
    }
    visited.iter().chain(tabs).find(|s| {
        let shown = address::strip_typed(&address::pretty_str(&s.url));
        shown.trim_end_matches('/') == want
    })
}

/// Every word of what was typed starts a word of the command's name
/// ("new sp" finds "New Space").
pub fn command_matches(name: &str, needle: &str) -> bool {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    let words: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    needle
        .split_whitespace()
        .all(|n| words.iter().any(|w| w.starts_with(n)))
}

/// The inline completion for `typed`: the rest of the best address that
/// carries on from it (two characters at least), from open tabs and
/// history. Never offered for words with a space.
pub fn inline(typed: &str, tabs: &[Suggestion], visited: &[Suggestion]) -> Option<String> {
    if typed.contains(' ') || typed.ends_with('/') && typed.len() < 3 {
        return None;
    }
    let mut options: Vec<Suggestion> = visited.to_vec();
    options.extend(tabs.iter().cloned());
    history::completion(typed, &options)
}

/// After an edit: the text was only extended (a completion may be offered),
/// not shortened or changed in the middle.
pub fn grew(before: &str, after: &str) -> bool {
    after.len() > before.len() && after.starts_with(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::history::SuggestionKind;

    fn visited(key: &str, url: &str, title: &str) -> Suggestion {
        Suggestion {
            key: key.into(),
            title: title.into(),
            url: url.into(),
            kind: SuggestionKind::Visited,
            tab: None,
            score: 1.0,
        }
    }

    #[test]
    fn three_letters_complete_a_visited_site() {
        let v = vec![visited("github.com", "https://github.com/", "GitHub")];
        assert_eq!(inline("git", &[], &v).as_deref(), Some("hub.com"));
        assert_eq!(inline("g", &[], &v), None, "two characters at least");
        assert_eq!(inline("git hub", &[], &v), None, "words never complete");
        assert_eq!(inline("gitlab", &[], &v), None);
    }

    #[test]
    fn edits_that_shrink_do_not_complete() {
        assert!(grew("gi", "git"));
        assert!(!grew("git", "gi"));
        assert!(!grew("git", "gxt"));
    }

    #[test]
    fn empty_palette_lists_open_tabs() {
        let a = TabId::new();
        let inputs = Inputs {
            open: vec![OpenRow {
                id: a.clone(),
                title: "Docs".into(),
                url: "https://docs.rs/".into(),
                elsewhere: None,
            }],
            ..Default::default()
        };
        let rows = rows(Purpose::Palette, &inputs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].action, Action::Switch(a));
        assert!(rows_empty_for_launch());
    }

    fn rows_empty_for_launch() -> bool {
        rows(Purpose::Launch, &Inputs::default()).is_empty()
    }

    #[test]
    fn typed_words_search_first_in_the_field_and_last_in_the_palette() {
        let tab = TabId::new();
        let inputs = Inputs {
            typed: "rust",
            engine: "Google",
            destination: Some("https://www.google.com/search?q=rust".into()),
            is_address: false,
            tabs: vec![Suggestion {
                key: "Rust".into(),
                title: "Rust".into(),
                url: "https://www.rust-lang.org/".into(),
                kind: SuggestionKind::Open,
                tab: Some(tab.clone()),
                score: 12.0,
            }],
            visited: vec![visited(
                "rust-lang.org",
                "https://www.rust-lang.org/",
                "Rust",
            )],
            commands: vec![CommandRow {
                id: "x".into(),
                name: "Rust Thing".into(),
                icon: "x".into(),
                shortcut: None,
            }],
            ..Default::default()
        };
        let field = rows(Purpose::Launch, &inputs);
        assert_eq!(field[0].kind, Kind::Search);
        assert_eq!(field[1].action, Action::Switch(tab.clone()));
        assert!(
            field.iter().all(|r| r.kind != Kind::Command),
            "no commands in the field"
        );
        assert_eq!(
            field
                .iter()
                .filter(|r| r.url.as_deref() == Some("https://www.rust-lang.org/"))
                .count(),
            1,
            "the open tab and the visit fold into one row"
        );
        let palette = rows(Purpose::Palette, &inputs);
        assert_eq!(palette[0].action, Action::Switch(tab));
        assert_eq!(palette[1].kind, Kind::Command);
        assert_eq!(palette.last().unwrap().kind, Kind::Search);
    }

    #[test]
    fn commands_match_word_starts() {
        assert!(command_matches("New Space", "new sp"));
        assert!(command_matches("Reopen Closed Tab", "clo"));
        assert!(!command_matches("New Space", "pace"));
        assert!(!command_matches("New Space", ""));
    }

    #[test]
    fn a_completed_address_goes_to_the_known_place_with_its_port() {
        let v = vec![visited(
            "127.0.0.1/essay",
            "http://127.0.0.1:47731/essay",
            "On Restraint",
        )];
        let inputs = Inputs {
            typed: "127.0.0.1/essay",
            destination: Some("http://127.0.0.1/essay".into()),
            is_address: true,
            visited: v,
            ..Default::default()
        };
        let r = rows(Purpose::Edit, &inputs);
        assert_eq!(
            r[0].action,
            Action::Go("http://127.0.0.1:47731/essay".into())
        );
        assert_eq!(r[0].title, "On Restraint");
        assert_eq!(r.len(), 1, "the visit folds into the top row");
    }

    #[test]
    fn an_address_says_open() {
        let inputs = Inputs {
            typed: "example.com",
            destination: Some("https://example.com/".into()),
            is_address: true,
            ..Default::default()
        };
        let r = rows(Purpose::Edit, &inputs);
        assert_eq!(r[0].kind, Kind::Address);
        assert_eq!(r[0].hint, "Go");
        assert_eq!(r[0].title, "example.com");
    }
}
