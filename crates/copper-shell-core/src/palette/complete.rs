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
    /// Bring back the closed tab at this index of
    /// [`copper_core::Browser::closed_tabs`].
    Reopen(usize),
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

/// The groups the rows come in, always in this order: the tabs you have
/// open, the places you have been, your bookmarks, the browser's commands
/// (`Ctrl+K` only), and last what Return does with the text itself (the
/// search, or the address typed). Tab and Shift-Tab move between them.
/// `Closed` is the empty `Ctrl+T`'s only group (Mac `Launcher.resting`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Tabs,
    History,
    Bookmarks,
    Commands,
    Search,
    Closed,
}

impl Section {
    /// The quiet label over the group (`first`: its first row's kind).
    pub fn title(self, first: Kind) -> &'static str {
        match self {
            Section::Tabs => "Open Tabs",
            Section::History => "History",
            Section::Bookmarks => "Bookmarks",
            Section::Commands => "Commands",
            Section::Search if first == Kind::Address => "Go To",
            Section::Search => "Search",
            Section::Closed => "Recently Closed",
        }
    }
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
    /// The group it is drawn in.
    pub section: Option<Section>,
}

/// The rows, and the one highlighted to begin with (what Return does).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Offers {
    pub rows: Vec<Row>,
    pub default: Option<usize>,
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
    /// Where Return would go with the text: for an address, what the field
    /// holds (with the offered completion); for words, the search for what
    /// was typed.
    pub destination: Option<String>,
    /// What was typed (not the offered completion) is an address rather
    /// than words: a domain, `localhost:3000`, an http(s) URL.
    pub is_address: bool,
    /// Open-tab rows (`Browser::open_tab_suggestions`) — this space first.
    pub tabs: Vec<Suggestion>,
    pub visited: Vec<Suggestion>,
    pub bookmarks: Vec<BookmarkHit>,
    pub commands: Vec<CommandRow>,
    /// Empty field in the palette: every open tab.
    pub open: Vec<OpenRow>,
    /// Tabs closed this session as `(title, url)`, oldest first
    /// ([`copper_core::Browser::closed_tabs`] order).
    pub closed: Vec<(String, String)>,
}

/// How many open tabs the palette lists for an empty field (Mac:
/// `CommandBar.limit`).
pub const LIMIT: usize = 8;

/// How many closed tabs the empty `Ctrl+T` offers (Mac: `Launcher.Cap.closed`).
pub const CLOSED: usize = 2;

/// How many rows each group may bring, so the card stays something you
/// glance at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    pub tabs: usize,
    pub history: usize,
    pub bookmarks: usize,
    pub commands: usize,
}

impl Caps {
    pub fn of(purpose: Purpose) -> Caps {
        match purpose {
            Purpose::Palette => Caps {
                tabs: 3,
                history: 3,
                bookmarks: 2,
                commands: 4,
            },
            Purpose::Launch | Purpose::Edit => Caps {
                tabs: 3,
                history: 5,
                bookmarks: 2,
                commands: 0,
            },
        }
    }

    fn of_section(self, section: Section) -> usize {
        match section {
            Section::Tabs => self.tabs,
            Section::History => self.history,
            Section::Bookmarks => self.bookmarks,
            Section::Commands => self.commands,
            Section::Search => 1,
            Section::Closed => CLOSED,
        }
    }
}

/// The rows for `inputs`, in order.
pub fn rows(purpose: Purpose, inputs: &Inputs<'_>) -> Vec<Row> {
    offers(purpose, inputs).rows
}

/// The rows for `inputs`, grouped, and the one Return takes before any
/// arrow key.
///
/// Groups come in [`Section`] order — Open Tabs, History, Bookmarks,
/// Commands (`Ctrl+K`), then the search or the address — each ranked as
/// before. The first row of the first group that has one is highlighted, so
/// typing then Return goes back to the open tab or the place you meant;
/// when nothing matches, Return searches. An address typed in full
/// (`example.com`, `localhost:3000`, an http(s) URL) still goes to that
/// address: its row, at the foot, is the one highlighted.
pub fn offers(purpose: Purpose, inputs: &Inputs<'_>) -> Offers {
    let typed = inputs.typed.trim();
    if typed.is_empty() {
        if purpose == Purpose::Launch {
            return closed_offers(&inputs.closed);
        }
        if purpose != Purpose::Palette {
            return Offers::default();
        }
        // `Ctrl+K` on an empty field: what is open, most recent first.
        let rows: Vec<Row> = inputs
            .open
            .iter()
            .take(LIMIT)
            .map(|t| Row {
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
                section: Some(Section::Tabs),
            })
            .collect();
        let default = (!rows.is_empty()).then_some(0);
        return Offers { rows, default };
    }

    let caps = Caps::of(purpose);
    let typed_row = typed_row(purpose, inputs);
    // A typed address that is a known place says the same as that place's
    // history row, under the same title: the history row folds into it. An
    // open tab there stays (switching to it is another answer).
    let place = typed_row
        .as_ref()
        .filter(|r| r.kind == Kind::Address)
        .map(fold_key);
    let mut out: Vec<Row> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    // Adds `row` to `section` unless that group is full (`false`: stop
    // looking) or it says what an earlier row already says.
    let mut add = |out: &mut Vec<Row>, mut row: Row, section: Section| -> bool {
        if out.iter().filter(|r| r.section == Some(section)).count() >= caps.of_section(section) {
            return false;
        }
        let key = fold_key(&row);
        let folds = row.kind != Kind::Command
            && (seen.contains(&key) || (section != Section::Tabs && place.as_ref() == Some(&key)));
        if !folds {
            seen.push(key);
            row.section = Some(section);
            out.push(row);
        }
        true
    };
    for s in &inputs.tabs {
        let Some(id) = s.tab.clone() else { continue };
        if !add(&mut out, tab_row(s, id), Section::Tabs) {
            break;
        }
    }
    for v in &inputs.visited {
        if !add(&mut out, visited_row(v), Section::History) {
            break;
        }
    }
    for row in inputs.bookmarks.iter().filter_map(bookmark_row) {
        if !add(&mut out, row, Section::Bookmarks) {
            break;
        }
    }
    if purpose == Purpose::Palette {
        let needle = typed.trim_start_matches('/').to_lowercase();
        for c in inputs
            .commands
            .iter()
            .filter(|c| command_matches(&c.name, &needle))
        {
            let row = Row {
                kind: Kind::Command,
                title: c.name.clone(),
                detail: c.shortcut.clone().unwrap_or_default(),
                hint: "Run".into(),
                action: Action::Command(c.id.clone()),
                url: None,
                icon: Some(c.icon.clone()),
                section: None,
            };
            if !add(&mut out, row, Section::Commands) {
                break;
            }
        }
    }
    let mut default = (!out.is_empty()).then_some(0);
    if let Some(mut row) = typed_row {
        row.section = Some(Section::Search);
        if inputs.is_address || default.is_none() {
            default = Some(out.len());
        }
        out.push(row);
    }
    Offers { rows: out, default }
}

/// The empty `Ctrl+T`: the tabs you closed, newest first, a Return away.
fn closed_offers(closed: &[(String, String)]) -> Offers {
    let rows: Vec<Row> = closed
        .iter()
        .enumerate()
        .rev()
        .take(CLOSED)
        .map(|(i, (title, url))| {
            let place = address::pretty_str(url);
            let label = if title.trim().is_empty() {
                place.clone()
            } else {
                title.clone()
            };
            Row {
                kind: Kind::Command,
                title: format!("Reopen \u{201C}{label}\u{201D}"),
                // The address, unless the title already says it.
                detail: if label == place { String::new() } else { place },
                hint: "Reopen".into(),
                action: Action::Reopen(i),
                url: None,
                icon: Some("edit-undo-symbolic".into()),
                section: Some(Section::Closed),
            }
        })
        .collect();
    let default = (!rows.is_empty()).then_some(0);
    Offers { rows, default }
}

/// A row's identity for folding duplicates: its address, else its title.
fn fold_key(row: &Row) -> String {
    row.url
        .as_deref()
        .map(address::strip_typed)
        .unwrap_or_else(|| row.title.to_lowercase())
}

fn tab_row(s: &Suggestion, id: TabId) -> Row {
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
        section: None,
    }
}

fn visited_row(v: &Suggestion) -> Row {
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
        section: None,
    }
}

fn bookmark_row(b: &BookmarkHit) -> Option<Row> {
    let url = b.bookmark.url.clone()?;
    Some(Row {
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
        section: None,
    })
}

/// What Return does with the text itself: the address (a place already
/// known goes there exactly, under its title), or the search.
fn typed_row(purpose: Purpose, inputs: &Inputs<'_>) -> Option<Row> {
    let typed = inputs.typed.trim();
    let dest = inputs.destination.as_ref()?;
    let go = if purpose == Purpose::Edit {
        "Go"
    } else {
        "Open"
    };
    if !inputs.is_address {
        return Some(Row {
            kind: Kind::Search,
            title: typed.to_string(),
            detail: format!("Search {}", inputs.engine),
            hint: "Search".into(),
            action: Action::Go(dest.clone()),
            url: None,
            icon: None,
            section: None,
        });
    }
    // The text is a place already known (completed from history or an open
    // tab): go to that place exactly — its scheme and port, which the shown
    // form leaves out — under its title.
    let full = if inputs.full.trim().is_empty() {
        typed
    } else {
        inputs.full
    };
    Some(match known_place(full, &inputs.tabs, &inputs.visited) {
        Some(known) => Row {
            kind: Kind::Address,
            title: if known.title.trim().is_empty() {
                address::pretty_str(&known.url)
            } else {
                known.title.clone()
            },
            detail: address::pretty_str(&known.url),
            hint: go.into(),
            action: Action::Go(known.url.clone()),
            url: Some(known.url.clone()),
            icon: None,
            section: None,
        },
        None => Row {
            kind: Kind::Address,
            title: address::pretty_str(dest),
            detail: String::new(),
            hint: go.into(),
            action: Action::Go(dest.clone()),
            url: Some(dest.clone()),
            icon: None,
            section: None,
        },
    })
}

/// The first row of each group, in order.
pub fn section_starts(rows: &[Row]) -> Vec<usize> {
    (0..rows.len())
        .filter(|&i| {
            rows[i].section.is_some() && (i == 0 || rows[i].section != rows[i - 1].section)
        })
        .collect()
}

/// Where Tab (`forward`) or Shift-Tab moves the highlight from `at`: the
/// first row of the next (or previous) group, wrapping. `None` when there
/// are no groups.
pub fn jump(rows: &[Row], at: Option<usize>, forward: bool) -> Option<usize> {
    let starts = section_starts(rows);
    let n = starts.len();
    if n == 0 {
        return None;
    }
    let here = at
        .filter(|&a| a < rows.len())
        .map(|a| starts.iter().rposition(|&s| s <= a).unwrap_or(0));
    Some(match here {
        None if forward => starts[0],
        None => starts[n - 1],
        Some(k) if forward => starts[(k + 1) % n],
        Some(k) => starts[(k + n - 1) % n],
    })
}

/// The inline completion the field offers after what was typed.
///
/// For an address being typed, the rest of the best known address that
/// carries on from it ([`inline`]). For words, the rest of the address the
/// highlighted row goes to, when it carries on from what was typed — so
/// what the field shows is what Return does — and nothing when that row is
/// the search. Never in the palette.
pub fn completion(purpose: Purpose, inputs: &Inputs<'_>) -> Option<String> {
    let typed = inputs.typed;
    if purpose == Purpose::Palette {
        return None;
    }
    if inputs.is_address {
        return inline(typed, &inputs.tabs, &inputs.visited);
    }
    if typed.contains(' ') {
        return None;
    }
    let want = address::strip_typed(typed);
    if want.chars().count() < 2 {
        return None;
    }
    let offers = offers(purpose, inputs);
    let row = offers.default.and_then(|i| offers.rows.get(i))?;
    if row.section == Some(Section::Search) {
        return None;
    }
    let shown = address::strip_typed(&address::pretty_str(row.url.as_deref()?));
    shown
        .strip_prefix(&want)
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
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

/// Whether a completion may be added to the field now: it holds exactly
/// what was typed (nothing offered yet), the caret (in characters) is at its
/// end and nothing is selected. Anything else means the user moved on — a
/// later key, a click inside the text — and the offer is dropped.
pub fn may_offer(text: &str, typed: &str, caret: i32, selection: Option<(i32, i32)>) -> bool {
    !text.is_empty()
        && text == typed
        && usize::try_from(caret).is_ok_and(|c| c == text.chars().count())
        && selection.is_none_or(|(a, b)| a == b)
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
    fn a_completion_lands_only_on_the_text_it_was_made_for() {
        assert!(may_offer("news.", "news.", 5, None));
        assert!(
            may_offer("café", "café", 4, None),
            "the caret counts characters"
        );
        assert!(may_offer("news.", "news.", 5, Some((5, 5))));
        // GTK has not put the caret after the new character yet.
        assert!(!may_offer("news.", "news.", 4, None));
        // A completion is already showing.
        assert!(!may_offer(
            "news.ycombinator.com",
            "news.",
            20,
            Some((5, 20))
        ));
        // Another key arrived first.
        assert!(!may_offer("news.y", "news.", 6, None));
        assert!(!may_offer("news.", "news.", 5, Some((0, 5))));
        assert!(!may_offer("", "", 0, None));
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
    fn empty_launch_offers_the_two_newest_closed_tabs() {
        let inputs = Inputs {
            closed: vec![
                ("Old".into(), "https://old.com/".into()),
                ("Newer".into(), "https://newer.com/".into()),
                (String::new(), "https://newest.com/".into()),
            ],
            ..Default::default()
        };
        let o = offers(Purpose::Launch, &inputs);
        let titles: Vec<&str> = o.rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Reopen \u{201C}newest.com\u{201D}",
                "Reopen \u{201C}Newer\u{201D}"
            ]
        );
        assert_eq!(o.rows[0].detail, "");
        assert_eq!(o.rows[1].detail, "newer.com");
        assert_eq!(o.rows[0].action, Action::Reopen(2));
        assert_eq!(o.rows[1].action, Action::Reopen(1));
        assert_eq!(
            o.rows[0].section.unwrap().title(Kind::Command),
            "Recently Closed"
        );
        assert_eq!(o.default, Some(0));
        // Typing leaves them for the usual groups; Edit never has them.
        assert!(rows(Purpose::Edit, &inputs).is_empty());
    }

    fn open_tab(title: &str, url: &str, id: &TabId) -> Suggestion {
        Suggestion {
            key: title.into(),
            title: title.into(),
            url: url.into(),
            kind: SuggestionKind::Open,
            tab: Some(id.clone()),
            score: 12.0,
        }
    }

    fn bookmark(title: &str, url: &str) -> BookmarkHit {
        BookmarkHit {
            bookmark: copper_core::bookmarks::Bookmark {
                title: title.into(),
                url: Some(url.into()),
                ..Default::default()
            },
            path: Vec::new(),
        }
    }

    /// "git" with something in every group.
    fn git(tab: &TabId) -> Inputs<'static> {
        Inputs {
            typed: "git",
            engine: "Google",
            destination: Some("https://www.google.com/search?q=git".into()),
            is_address: false,
            tabs: vec![open_tab(
                "Git Guide",
                "http://127.0.0.1:8737/git-guide.html",
                tab,
            )],
            visited: vec![
                visited("github.com", "https://github.com/", "GitHub"),
                visited("git-scm.com/book", "https://git-scm.com/book", "Git - Book"),
                // The open tab, again: it folds into the tab's row.
                visited(
                    "127.0.0.1/git-guide.html",
                    "http://127.0.0.1:8737/git-guide.html",
                    "Git Guide",
                ),
            ],
            bookmarks: vec![bookmark("Gitea", "https://gitea.io/")],
            commands: vec![CommandRow {
                id: "x".into(),
                name: "Git Thing".into(),
                icon: "x".into(),
                shortcut: None,
            }],
            ..Default::default()
        }
    }

    fn sections(rows: &[Row]) -> Vec<Section> {
        let mut v: Vec<Section> = rows.iter().filter_map(|r| r.section).collect();
        v.dedup();
        v
    }

    #[test]
    fn the_field_groups_open_tabs_then_history_then_search() {
        let tab = TabId::new();
        let inputs = git(&tab);
        let o = offers(Purpose::Launch, &inputs);
        assert_eq!(
            sections(&o.rows),
            [
                Section::Tabs,
                Section::History,
                Section::Bookmarks,
                Section::Search
            ]
        );
        let titles: Vec<&str> = o.rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            ["Git Guide", "GitHub", "Git - Book", "Gitea", "git"]
        );
        assert_eq!(o.rows[0].action, Action::Switch(tab.clone()));
        assert_eq!(o.rows.last().unwrap().kind, Kind::Search);
        assert_eq!(o.default, Some(0), "the open tab is what Return does");
        assert!(
            o.rows.iter().all(|r| r.kind != Kind::Command),
            "no commands in the field"
        );
        // Every row is in a group, groups are contiguous and in order.
        let order: Vec<Section> = o.rows.iter().map(|r| r.section.unwrap()).collect();
        assert!(order.windows(2).all(|w| w[0] <= w[1]));
        // Ctrl+L is grouped the same way.
        assert_eq!(rows(Purpose::Edit, &inputs), o.rows);
    }

    #[test]
    fn the_first_group_with_a_row_is_highlighted() {
        let tab = TabId::new();
        // No open tab matches: the history hit.
        let inputs = Inputs {
            tabs: vec![],
            ..git(&tab)
        };
        let o = offers(Purpose::Launch, &inputs);
        assert_eq!(o.rows[0].section, Some(Section::History));
        assert_eq!(o.default, Some(0));
        assert_eq!(o.rows[0].action, Action::Go("https://github.com/".into()));
        // Nothing but the search: Return searches.
        let only = Inputs {
            typed: "zzz qq",
            engine: "Google",
            destination: Some("https://www.google.com/search?q=zzz+qq".into()),
            ..Default::default()
        };
        let o = offers(Purpose::Launch, &only);
        assert_eq!(o.rows.len(), 1);
        assert_eq!((o.rows[0].kind, o.default), (Kind::Search, Some(0)));
        assert_eq!(o.rows[0].section.unwrap().title(o.rows[0].kind), "Search");
    }

    #[test]
    fn a_typed_address_still_goes_there_on_return() {
        let tab = TabId::new();
        for typed in [
            "example.com",
            "localhost:3000",
            "https://example.com/x",
            "127.0.0.1:8737/git",
        ] {
            let dest = format!("https://{}/", typed.trim_start_matches("https://"));
            let inputs = Inputs {
                typed,
                destination: Some(dest.clone()),
                is_address: true,
                tabs: vec![open_tab("Somewhere", "https://somewhere.test/", &tab)],
                visited: vec![visited("other.test", "https://other.test/", "Other")],
                ..Default::default()
            };
            let o = offers(Purpose::Launch, &inputs);
            let at = o.default.unwrap();
            assert_eq!(
                at,
                o.rows.len() - 1,
                "{typed}: the address row, at the foot"
            );
            assert_eq!(o.rows[at].kind, Kind::Address);
            assert_eq!(o.rows[at].action, Action::Go(dest));
            assert_eq!(o.rows[at].section.unwrap().title(Kind::Address), "Go To");
            assert_eq!(o.rows[0].section, Some(Section::Tabs), "groups still first");
        }
    }

    #[test]
    fn tab_jumps_between_groups_and_wraps() {
        let tab = TabId::new();
        let rows = rows(Purpose::Launch, &git(&tab));
        // 0 tab | 1 2 history | 3 bookmark | 4 search
        assert_eq!(section_starts(&rows), [0, 1, 3, 4]);
        assert_eq!(jump(&rows, Some(0), true), Some(1));
        assert_eq!(jump(&rows, Some(1), true), Some(3));
        assert_eq!(jump(&rows, Some(2), true), Some(3), "from inside a group");
        assert_eq!(jump(&rows, Some(3), true), Some(4));
        assert_eq!(jump(&rows, Some(4), true), Some(0), "wraps to the top");
        assert_eq!(jump(&rows, Some(0), false), Some(4), "Shift-Tab wraps back");
        assert_eq!(jump(&rows, Some(2), false), Some(0), "to the group before");
        assert_eq!(jump(&rows, Some(1), false), Some(0));
        assert_eq!(jump(&rows, None, true), Some(0));
        assert_eq!(jump(&rows, None, false), Some(4));
        assert_eq!(jump(&[], Some(0), true), None);
        // One group: Tab stays on its first row.
        let one = rows[4..].to_vec();
        assert_eq!(jump(&one, Some(0), true), Some(0));
    }

    #[test]
    fn the_completion_is_what_the_highlighted_row_opens() {
        let tab = TabId::new();
        // The open tab's address does not carry on from "git": nothing.
        assert_eq!(completion(Purpose::Launch, &git(&tab)), None);
        // GitHub highlighted: the rest of its address.
        let inputs = Inputs {
            tabs: vec![],
            ..git(&tab)
        };
        assert_eq!(
            completion(Purpose::Launch, &inputs).as_deref(),
            Some("hub.com")
        );
        // An address being typed: the best known address carrying on.
        let addr = Inputs {
            typed: "github.co",
            destination: Some("https://github.co/".into()),
            is_address: true,
            visited: vec![visited("github.com", "https://github.com/", "GitHub")],
            ..Default::default()
        };
        assert_eq!(completion(Purpose::Launch, &addr).as_deref(), Some("m"));
        assert_eq!(completion(Purpose::Palette, &inputs), None);
    }

    #[test]
    fn the_palette_groups_the_same_way_with_commands_before_the_search() {
        let tab = TabId::new();
        let inputs = git(&tab);
        let o = offers(Purpose::Palette, &inputs);
        assert_eq!(
            sections(&o.rows),
            [
                Section::Tabs,
                Section::History,
                Section::Bookmarks,
                Section::Commands,
                Section::Search
            ]
        );
        assert_eq!(o.rows[0].action, Action::Switch(tab));
        assert_eq!(o.default, Some(0));
        assert_eq!(
            o.rows
                .iter()
                .filter(|r| r.url.as_deref() == Some("http://127.0.0.1:8737/git-guide.html"))
                .count(),
            1,
            "the open tab and the visit fold into one row"
        );
        let commands = rows(
            Purpose::Palette,
            &Inputs {
                typed: "git th",
                ..git(&TabId::new())
            },
        );
        assert_eq!(
            commands
                .iter()
                .find(|r| r.kind == Kind::Command)
                .map(|r| r.section),
            Some(Some(Section::Commands))
        );
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
