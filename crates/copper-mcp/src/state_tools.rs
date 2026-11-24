//! The browser's own state, for agents (Fork/MCP/StateTools.swift): the
//! user's history, bookmarks and spaces. Copper's own tools in Playwright
//! MCP's style. Read through [`Host`]; the words are the Mac's.
//!
//! `browser_spaces move` takes only the caller's own tab, never one on the
//! user's screen.

use copper_core::bookmarks::Bookmark;
use copper_core::history::Trace;
use serde_json::{Map, Value};

use crate::agent_tabs::{one_line, short};
use crate::host::{Content, Host, SpaceInfo};
use crate::tools::{arg_f64, arg_str};
use crate::{CallCtx, Failure, Mcp};

/// These tool names are answered here.
pub const NAMES: [&str; 3] = ["browser_history", "browser_bookmarks", "browser_spaces"];

/// No tab to stage first: these read or write the browser, not a page
/// (`StateTools.tabless`).
pub fn tabless(name: &str, args: &Map<String, Value>) -> bool {
    match name {
        "browser_history" => true,
        "browser_bookmarks" => arg_str(args, "action") != Some("add") || args.contains_key("url"),
        "browser_spaces" => arg_str(args, "action") != Some("move"),
        _ => false,
    }
}

/// `browser_history`'s answer for these rows.
pub fn history_text(query: &str, found: &[Trace]) -> String {
    if found.is_empty() {
        return if query.is_empty() {
            "History is empty.".into()
        } else {
            format!("Nothing in history matches “{query}”.")
        };
    }
    let lines: Vec<String> = found
        .iter()
        .map(|t| {
            let title = if t.title.is_empty() {
                t.key.clone()
            } else {
                one_line(&t.title, 80).replace(']', ")")
            };
            let last = copper_core::date::IsoDate(t.last.unix().floor() as i64);
            let visits = if t.count == 1 { "visit" } else { "visits" };
            format!(
                "- [{title}]({}) · last {last} · {} {visits}",
                t.url, t.count
            )
        })
        .collect();
    let what = if query.is_empty() {
        " (most recent)".to_string()
    } else {
        format!(" matching “{query}”")
    };
    format!("### History{what}\n{}", lines.join("\n"))
}

/// `browser_bookmarks list|search`: every site under `roots` matching
/// `query` (lowercased; empty matches all), with the folders it is in.
pub fn bookmarks_text(roots: &[Bookmark], query: &str) -> String {
    fn walk(nodes: &[Bookmark], trail: &str, query: &str, lines: &mut Vec<String>) {
        for node in nodes {
            if node.is_folder() {
                let inner = if trail.is_empty() {
                    node.title.clone()
                } else {
                    format!("{trail} › {}", node.title)
                };
                walk(node.kids(), &inner, query, lines);
                continue;
            }
            let Some(url) = &node.url else { continue };
            if !query.is_empty()
                && !node.title.to_lowercase().contains(query)
                && !url.to_lowercase().contains(query)
            {
                continue;
            }
            let place = if trail.is_empty() {
                String::new()
            } else {
                format!(" in {trail}")
            };
            lines.push(format!("- [{}]({url}){place}", node.title));
        }
    }
    let mut lines = Vec::new();
    walk(roots, "", query, &mut lines);
    if lines.is_empty() {
        return if query.is_empty() {
            "No bookmarks.".into()
        } else {
            format!("No bookmark matches “{query}”.")
        };
    }
    let more = if lines.len() > 200 {
        format!("\n… {} more — search to narrow", lines.len() - 200)
    } else {
        String::new()
    };
    lines.truncate(200);
    format!("### Bookmarks\n{}{more}", lines.join("\n"))
}

/// `browser_spaces list`. `yours`: the space holding the caller's own tab.
pub fn spaces_text(spaces: &[SpaceInfo], yours: Option<&str>) -> String {
    let lines: Vec<String> = spaces
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut marks = Vec::new();
            if s.on_screen {
                marks.push("on screen in this window");
            }
            if yours == Some(s.id.as_str()) {
                marks.push("holds your tab");
            }
            let name = if s.name.is_empty() {
                "Untitled"
            } else {
                &s.name
            };
            let profile = s
                .profile
                .as_ref()
                .map(|p| format!(" (profile {p})"))
                .unwrap_or_default();
            let marked = if marks.is_empty() {
                String::new()
            } else {
                format!(" ({})", marks.join(", "))
            };
            format!(
                "- {i}: {name} {{space {}}}{profile} — {} tabs{marked}",
                short_id(&s.id),
                s.tabs
            )
        })
        .collect();
    format!("### Spaces\n{}", lines.join("\n"))
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect::<String>().to_lowercase()
}

/// A space by index, exact name (any case), or the start of its id
/// (`StateTools.pick`).
pub fn pick<'a>(raw: &str, all: &'a [SpaceInfo]) -> Option<&'a SpaceInfo> {
    if let Ok(i) = raw.parse::<usize>()
        && let Some(s) = all.get(i)
    {
        return Some(s);
    }
    let named: Vec<&SpaceInfo> = all
        .iter()
        .filter(|s| s.name.to_lowercase() == raw.to_lowercase())
        .collect();
    if let [one] = named[..] {
        return Some(one);
    }
    let lower = raw.to_lowercase();
    let ids: Vec<&SpaceInfo> = all
        .iter()
        .filter(|s| s.id.to_lowercase().starts_with(&lower))
        .collect();
    match ids[..] {
        [one] => Some(one),
        _ => None,
    }
}

impl<H: Host> Mcp<H> {
    pub(crate) async fn state_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        ctx: &CallCtx,
    ) -> Result<Vec<Content>, Failure> {
        let text = match name {
            "browser_history" => self.history_tool(args, ctx)?,
            "browser_bookmarks" => self.bookmarks_tool(args, ctx)?,
            _ => self.spaces_tool(args, ctx)?,
        };
        Ok(vec![Content::Text(text)])
    }

    fn history_tool(&self, args: &Map<String, Value>, ctx: &CallCtx) -> Result<String, Failure> {
        if !ctx.loopback {
            return Err(Failure::new(
                "browser_history is only for agents the user runs on this computer, not linked apps",
            ));
        }
        let query = arg_str(args, "query").unwrap_or("");
        let limit = arg_f64(args, "limit")
            .map_or(20, |n| n as i64)
            .clamp(1, 200) as usize;
        Ok(history_text(query, &self.host.history(query, limit)))
    }

    fn bookmarks_tool(&self, args: &Map<String, Value>, ctx: &CallCtx) -> Result<String, Failure> {
        match arg_str(args, "action").unwrap_or("list") {
            "list" | "search" => {
                let query = arg_str(args, "query").unwrap_or("").to_lowercase();
                Ok(bookmarks_text(&self.host.bookmarks(), &query))
            }
            "add" => {
                let mut title = arg_str(args, "title").map(str::to_string);
                let url = match arg_str(args, "url") {
                    Some(raw) => copper_core::url_from_typed(raw)
                        .ok_or_else(|| Failure(format!("not an address: {raw}")))?,
                    None => {
                        let tab = self.current(ctx)?;
                        let page = tab
                            .url
                            .as_deref()
                            .filter(|u| !u.is_empty() && *u != "about:blank")
                            .and_then(|u| url::Url::parse(u).ok())
                            .ok_or("your tab has no page to bookmark")?;
                        title = title.or(Some(tab.title));
                        page
                    }
                };
                let title = title
                    .filter(|t| !t.is_empty())
                    .or_else(|| url.host_str().map(str::to_string))
                    .unwrap_or_else(|| url.to_string());
                if self.host.add_bookmark(url.as_str(), &title) {
                    Ok(format!("Bookmarked {url}."))
                } else {
                    Ok(format!("Already bookmarked: {url}"))
                }
            }
            _ => Err(Failure::new("action must be list, search or add")),
        }
    }

    fn spaces_tool(&self, args: &Map<String, Value>, ctx: &CallCtx) -> Result<String, Failure> {
        let spaces = self.host.spaces();
        match arg_str(args, "action").unwrap_or("list") {
            "list" => {
                let yours = self
                    .own(&ctx.who.key)
                    .and_then(|t| self.host.space_of(&t.id));
                Ok(spaces_text(&spaces, yours.as_deref()))
            }
            "move" => {
                let raw = arg_str(args, "space").unwrap_or("").trim();
                if raw.is_empty() {
                    return Err(Failure::new(
                        "space required (name, index or id from browser_spaces list)",
                    ));
                }
                let space = pick(raw, &spaces).ok_or_else(|| {
                    Failure(format!(
                        "no single space {raw} — browser_spaces list names them"
                    ))
                })?;
                let tab = self.current(ctx)?;
                // shortcut: the Mac also takes any tab an agent opened (AgentTabs.opened); this takes the caller's own only.
                if self.own(&ctx.who.key).map(|t| t.id) != Some(tab.id.clone()) {
                    return Err(Failure::new(
                        "only a tab of your own moves — open one with browser_tabs new",
                    ));
                }
                if tab.pinned {
                    return Err(Failure::new("a pinned tab is in every space already"));
                }
                if tab.selected {
                    return Err(Failure::new(
                        "your tab is on the user's screen; moving it would change what they see",
                    ));
                }
                if self.host.space_of(&tab.id).as_deref() == Some(space.id.as_str()) {
                    return Ok(format!("Your tab is already in {}.", space.name));
                }
                if !self.host.move_to_space(&tab.id, &space.id)
                    || self.host.space_of(&tab.id).as_deref() != Some(space.id.as_str())
                {
                    return Err(Failure::new("Copper did not move the tab"));
                }
                Ok(format!(
                    "Moved your tab to {}; your calls still land on it. {{tab {}}}",
                    space.name,
                    short(&tab.id)
                ))
            }
            _ => Err(Failure::new("action must be list or move")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::date::SwiftDate;

    fn trace(title: &str, url: &str, unix: f64, count: i64) -> Trace {
        Trace {
            key: copper_core::address::strip_typed(url),
            title: title.into(),
            url: url.into(),
            last: SwiftDate::from_unix(unix),
            count,
        }
    }

    #[test]
    fn history_lines_like_the_mac() {
        let rows = [
            trace(
                "Example ]Domain[",
                "https://example.com/",
                1_700_000_000.0,
                3,
            ),
            trace("", "https://nameless.test/a", 1_699_999_000.5, 1),
        ];
        assert_eq!(
            history_text("", &rows),
            "### History (most recent)\n\
             - [Example )Domain[](https://example.com/) · last 2023-11-14T22:13:20Z · 3 visits\n\
             - [nameless.test/a](https://nameless.test/a) · last 2023-11-14T21:56:40Z · 1 visit"
        );
        assert!(
            history_text("exa", &rows[..1]).starts_with("### History matching “exa”\n- [Example")
        );
        assert_eq!(history_text("", &[]), "History is empty.");
        assert_eq!(history_text("zz", &[]), "Nothing in history matches “zz”.");
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(one_line("a\n\tb   c", 80), "a b c");
        let long = "x".repeat(90);
        assert_eq!(one_line(&long, 80), format!("{}…", "x".repeat(80)));
    }

    #[test]
    fn bookmarks_walk_folders_and_filter() {
        let roots = vec![
            Bookmark::site("Top", "https://top.test/"),
            Bookmark::folder(
                "Work",
                vec![Bookmark::folder(
                    "Docs",
                    vec![Bookmark::site("Spec", "https://docs.test/spec")],
                )],
            ),
        ];
        assert_eq!(
            bookmarks_text(&roots, ""),
            "### Bookmarks\n- [Top](https://top.test/)\n- [Spec](https://docs.test/spec) in Work › Docs"
        );
        assert_eq!(
            bookmarks_text(&roots, "docs"),
            "### Bookmarks\n- [Spec](https://docs.test/spec) in Work › Docs"
        );
        assert_eq!(
            bookmarks_text(&roots, "nope"),
            "No bookmark matches “nope”."
        );
        assert_eq!(bookmarks_text(&[], ""), "No bookmarks.");
        let many: Vec<Bookmark> = (0..205)
            .map(|i| Bookmark::site(&format!("s{i}"), &format!("https://s{i}.test/")))
            .collect();
        let text = bookmarks_text(&many, "");
        assert_eq!(text.lines().count(), 1 + 200 + 1);
        assert!(text.ends_with("\n… 5 more — search to narrow"));
    }

    fn space(id: &str, name: &str, tabs: usize, on_screen: bool) -> SpaceInfo {
        SpaceInfo {
            id: id.into(),
            name: name.into(),
            profile: None,
            tabs,
            on_screen,
        }
    }

    #[test]
    fn spaces_list_and_pick() {
        let mut all = vec![
            space("AAAA1111-0000-4000-8000-000000000001", "Home", 3, true),
            space("BBBB2222-0000-4000-8000-000000000002", "", 0, false),
            space("BBBB3333-0000-4000-8000-000000000003", "Work", 1, false),
        ];
        all[2].profile = Some("Work".into());
        assert_eq!(
            spaces_text(&all, Some("BBBB3333-0000-4000-8000-000000000003")),
            "### Spaces\n\
             - 0: Home {space aaaa1111} — 3 tabs (on screen in this window)\n\
             - 1: Untitled {space bbbb2222} — 0 tabs\n\
             - 2: Work {space bbbb3333} (profile Work) — 1 tabs (holds your tab)"
        );
        assert_eq!(pick("2", &all).map(|s| s.name.as_str()), Some("Work"));
        assert_eq!(pick("work", &all).map(|s| s.name.as_str()), Some("Work"));
        assert_eq!(pick("aaaa", &all).map(|s| s.name.as_str()), Some("Home"));
        assert!(pick("bbbb", &all).is_none(), "two ids begin bbbb");
        assert!(pick("9", &all).is_none());
    }

    #[test]
    fn which_calls_need_no_tab() {
        let args = |v: Value| v.as_object().cloned().unwrap();
        assert!(tabless("browser_history", &Map::new()));
        assert!(tabless("browser_bookmarks", &Map::new()));
        assert!(!tabless(
            "browser_bookmarks",
            &args(serde_json::json!({"action": "add"}))
        ));
        assert!(tabless(
            "browser_bookmarks",
            &args(serde_json::json!({"action": "add", "url": "https://x.test"}))
        ));
        assert!(tabless("browser_spaces", &Map::new()));
        assert!(!tabless(
            "browser_spaces",
            &args(serde_json::json!({"action": "move"}))
        ));
        assert!(!tabless("browser_click", &Map::new()));
    }
}
