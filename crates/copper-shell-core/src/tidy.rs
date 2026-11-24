//! Tidy (Mac: `Fork/Tidy.swift`, copper-browser/Copper#200): the loose tabs
//! of the current space filed into a few named folders in one go, with Undo
//! right after.
//!
//! No storage of its own. The tabs it takes are the ones Clear would take
//! ([`Shell::clearable_tabs`]; private tabs too stay out, as they never sit
//! in Saved), and a tidied tab is a Saved row in an ordinary folder
//! (`groups.json` plus `Tab::group`), so Copper Cloud syncs it like any
//! other. Which folder: the configured model, shown each tab's title and
//! host (never the page), names a few and may reuse a folder this space
//! already has; without a model, or when it fails or takes over
//! [`PATIENCE`], tabs are filed by site, two or more to a folder. A tab that
//! fits nowhere stays loose.
//!
//! The pure half ([`question`], [`parse`], [`by_site`], …) is `TidyPlan`;
//! the rows' moves and their Undo are copper-core's
//! (`Browser::file_tabs`, `Browser::unfile_tabs`).

use crate::agents::groups;
use crate::folders::{MARK, below};
use crate::view::{Refresh, ShellView};
use crate::{Platform, Shell};
use copper_core::TabId;
use copper_core::groups::TabGroup;
use copper_mcp::intelligence::{self, Effective, Lane};
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// The column offers Tidy from this many loose tabs (`Tidy.least`).
pub const LEAST: usize = 3;
/// The model's whole budget, after which tabs are filed by site.
pub const PATIENCE: Duration = Duration::from_secs(8);
/// How long Undo is offered.
pub const UNDO_FOR: Duration = Duration::from_secs(8);

/// What the model is told (`TidyPlan.instructions`).
pub const INSTRUCTIONS: &str = "You file a person's open browser tabs into a few sidebar folders. You get JSON: \"existing\" (folder names this space already has) and \"tabs\" (each with \"i\", \"title\", \"host\"). Answer with JSON only, no prose: {\"folders\":[{\"name\":\"Folder name\",\"tabs\":[0,2]}]}. Rules: 2 to 5 folders; a name is 1 to 3 plain words in Title Case, no emoji; use an existing folder's exact name when tabs fit it; a new folder needs at least 2 tabs; a tab goes in at most one folder; leave out tabs that fit no folder well. Group by what the tabs are for (a project, a topic, a task), not only by site.";

/// One folder of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    /// Indexes into the tabs the plan was made for.
    pub tabs: Vec<usize>,
}

/// A tab's host as the model and the fallback see it: lowercased, no `www.`.
pub fn host(url: &str) -> String {
    let Some(host) = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
    else {
        return String::new();
    };
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// A folder name from a host (`Fork.brand`): `docs.github.com` → `Github`,
/// `bbc.co.uk` → `Bbc`.
pub fn brand(host: &str) -> String {
    let host = host.to_lowercase();
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.first() == Some(&"www") {
        parts.remove(0);
    }
    // The registrable label, roughly: second from the end, or third when the
    // end looks like `co.uk`.
    let two = ["co", "com", "org", "net", "ac", "gov", "edu"];
    let n = parts.len();
    let core = if n >= 3 && two.contains(&parts[n - 2]) && parts[n - 1].chars().count() == 2 {
        parts[n - 3]
    } else if n >= 2 {
        parts[n - 2]
    } else {
        parts.first().copied().unwrap_or("Group")
    };
    let mut chars = core.chars();
    chars.next().map_or_else(
        || "Group".into(),
        |c| c.to_uppercase().chain(chars).collect(),
    )
}

/// The question: titles (cut to 100 characters) and hosts, nothing else.
/// Keys sorted, as the Mac sends them.
pub fn question(titles: &[String], hosts: &[String], existing: &[String]) -> String {
    let tabs: Vec<Value> = titles
        .iter()
        .zip(hosts)
        .enumerate()
        .map(|(i, (title, host))| {
            json!({ "host": host, "i": i, "title": title.chars().take(100).collect::<String>() })
        })
        .collect();
    json!({ "existing": existing, "tabs": tabs }).to_string()
}

/// An existing folder the name means: the whole path or its last part, any
/// case.
pub fn reuse(name: &str, existing: &[String]) -> Option<String> {
    let key = name.trim().to_lowercase();
    existing
        .iter()
        .find(|e| e.to_lowercase() == key)
        .or_else(|| {
            existing
                .iter()
                .find(|e| e.rsplit(MARK).next().unwrap_or(e).to_lowercase() == key)
        })
        .cloned()
}

/// A new folder's name: one line, at most 32 characters, and no ` › `,
/// which would make it a folder inside another.
pub fn clean(name: &str) -> String {
    let flat = name.replace('›', "-").lines().collect::<Vec<_>>().join(" ");
    let flat = flat.trim_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');
    flat.chars().take(32).collect::<String>().trim().to_string()
}

/// The model's answer, read strictly: indexes in range and each used once,
/// a new folder only with two tabs or more (an existing one takes one), at
/// most six folders. Anything else is left out.
pub fn parse(json: &Map<String, Value>, count: usize, existing: &[String]) -> Vec<Folder> {
    let Some(raw) = json.get("folders").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut used: HashSet<usize> = HashSet::new();
    let mut out: Vec<Folder> = Vec::new();
    for entry in raw {
        let Some(said) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let old = reuse(said, existing);
        let name = old.clone().unwrap_or_else(|| clean(said));
        if name.is_empty() {
            continue;
        }
        let mut tabs: Vec<usize> = Vec::new();
        for value in entry
            .get("tabs")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            // A number, whole or not, as `NSNumber.intValue` reads it.
            let Some(i) = value.as_f64().map(|f| f.trunc()) else {
                continue;
            };
            if i < 0.0 || i >= count as f64 {
                continue;
            }
            let i = i as usize;
            if !used.contains(&i) && !tabs.contains(&i) {
                tabs.push(i);
            }
        }
        if let Some(same) = out
            .iter_mut()
            .find(|f| f.name.to_lowercase() == name.to_lowercase())
        {
            used.extend(&tabs);
            same.tabs.extend(tabs);
            continue;
        }
        let least = if old.is_none() { 2 } else { 1 };
        if tabs.len() < least || out.len() >= 6 {
            continue;
        }
        used.extend(&tabs);
        out.push(Folder { name, tabs });
    }
    out
}

/// No model: one folder per site with two tabs or more, named by the site
/// ([`brand`]) or by the existing folder of that name.
pub fn by_site(hosts: &[String], existing: &[String]) -> Vec<Folder> {
    let mut buckets: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, host) in hosts.iter().enumerate().filter(|(_, h)| !h.is_empty()) {
        let name = brand(host);
        match buckets.iter_mut().find(|(n, _)| *n == name) {
            Some((_, tabs)) => tabs.push(i),
            None => buckets.push((name, vec![i])),
        }
    }
    buckets
        .into_iter()
        .filter(|(_, tabs)| tabs.len() >= 2)
        .map(|(name, tabs)| Folder {
            name: reuse(&name, existing).unwrap_or(name),
            tabs,
        })
        .collect()
}

/// The pill's words: "Tidied 5 tabs into 2 folders" (`TidyToast`).
pub fn tidied_words(tabs: usize, folders: usize) -> String {
    let s = |n: usize| if n == 1 { "" } else { "s" };
    format!(
        "Tidied {tabs} tab{} into {folders} folder{}",
        s(tabs),
        s(folders)
    )
}

/// What one Tidy did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tidied {
    /// How many tabs were filed.
    pub tabs: usize,
    /// The folders they went into, by name.
    pub folders: Vec<String>,
    /// The model named them (else: by site).
    pub by_model: bool,
    /// Why nothing happened, when something stopped it.
    pub error: Option<&'static str>,
}

/// The one Undo on offer.
struct Undo {
    moved: Vec<(TabId, usize)>,
    /// Folders this tidy made, and existing ones it opened.
    made: Vec<String>,
    opened: Vec<String>,
    space: String,
    folders: usize,
    until: Instant,
}

thread_local! {
    // One window per process, on both platforms.
    static UNDO: RefCell<Option<Undo>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

impl<P: Platform> Shell<P> {
    /// The current space's rows Tidy would file, in row order.
    pub fn tidy_candidates(&self) -> Vec<TabId> {
        let ids = self.clearable_tabs();
        let b = self.browser();
        ids.into_iter()
            .filter(|id| b.tab(id).is_some_and(|t| !t.private))
            .collect()
    }

    /// A tidy is waiting on the model ("Tidying…").
    pub fn tidy_busy(&self) -> bool {
        BUSY.with(Cell::get)
    }

    /// The space's folders a tidy may reuse: those with a tab in its row
    /// (or a pin), and its empty top-level ones (`Groups.present` +
    /// `Groups.homeless`).
    fn space_folders(&self, space: &str) -> Vec<TabGroup> {
        let all = groups::all(self);
        let b = self.browser();
        let mut out: Vec<TabGroup> = Vec::new();
        for t in b.pins().into_iter().chain(b.row(space)) {
            let Some(g) = t
                .group
                .as_deref()
                .and_then(|id| all.iter().find(|g| g.id.eq_ignore_ascii_case(id)))
            else {
                continue;
            };
            if !out.iter().any(|o| o.id == g.id) {
                out.push(g.clone());
            }
        }
        let held: HashSet<String> = b
            .all_tabs()
            .filter_map(|t| t.group.as_deref().map(str::to_ascii_uppercase))
            .collect();
        out.extend(
            all.iter()
                .filter(|g| {
                    g.space
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(space))
                        && !held.contains(&g.id.to_ascii_uppercase())
                        && !all.iter().any(|o| below(&g.name, &o.name))
                })
                .cloned(),
        );
        out
    }

    fn set_busy(&self, busy: bool) {
        BUSY.with(|b| b.set(busy));
        self.view().refresh(
            self,
            &Refresh {
                tabs: true,
                ..Default::default()
            },
        );
    }

    /// File the current space's loose tabs (`Tidy.tidy`). `local` skips the
    /// model. The caller shows "Tidied N tabs into M folders · Undo" for
    /// [`UNDO_FOR`] when it filed any.
    pub async fn tidy_tabs(&self, local: bool) -> Tidied {
        if self.tidy_busy() {
            return Tidied {
                error: Some("already tidying"),
                ..Default::default()
            };
        }
        let ids = self.tidy_candidates();
        if ids.len() < 2 {
            return Tidied::default();
        }
        let space = self.browser().current_space_id().to_string();
        let existing: Vec<String> = self
            .space_folders(&space)
            .into_iter()
            .map(|g| g.name)
            .collect();
        let (titles, hosts): (Vec<String>, Vec<String>) = {
            let b = self.browser();
            ids.iter()
                .filter_map(|id| b.tab(id))
                .map(|t| (t.title.clone(), host(&t.url)))
                .unzip()
        };
        let mut plan = Vec::new();
        if !local && let Some(text) = self.ask(question(&titles, &hosts, &existing)).await {
            plan = parse(&intelligence::json_in(&text), ids.len(), &existing);
        }
        let by_model = !plan.is_empty();
        if plan.is_empty() {
            plan = by_site(&hosts, &existing);
        }
        // The row may have moved on while the model thought: only tabs that
        // are still loose here are filed.
        if self.browser().current_space_id() != space {
            return Tidied {
                error: Some("the space changed"),
                ..Default::default()
            };
        }
        let still: HashSet<TabId> = self.tidy_candidates().into_iter().collect();
        let filed: Vec<(String, Vec<TabId>)> = plan
            .into_iter()
            .map(|f| {
                let tabs = f
                    .tabs
                    .iter()
                    .map(|i| ids[*i].clone())
                    .filter(|id| still.contains(id))
                    .collect::<Vec<_>>();
                (f.name, tabs)
            })
            .filter(|(_, tabs)| !tabs.is_empty())
            .collect();
        let tabs = self.file(&filed, &space);
        tracing::info!(tabs, by_model, "tidied tabs");
        Tidied {
            tabs,
            folders: if tabs > 0 {
                filed.into_iter().map(|(name, _)| name).collect()
            } else {
                Vec::new()
            },
            by_model,
            error: None,
        }
    }

    /// One model call with a hard budget, on whichever lane is set up (the
    /// router or the Claude account, `Router.ask`): the reply's text, or
    /// `None` when there is no model, it failed or it took longer than
    /// [`PATIENCE`].
    async fn ask(&self, user: String) -> Option<String> {
        let claude = crate::claude::ready(self.store());
        let eff = Effective::new(&intelligence::read_files(self.store()), claude);
        if !eff.model_ready() {
            return None;
        }
        let model = intelligence::model(&eff.keys, Some(&eff.keys.text_model));
        self.set_busy(true);
        let call = if eff.keys.lane == Lane::Claude {
            let asked = crate::claude::account(self.store()).ask(
                INSTRUCTIONS,
                &user,
                &model,
                600,
                PATIENCE,
            );
            crate::cloud::spawn(async move {
                let asked = async { asked.await.map(|(text, _)| text).map_err(|f| f.text()) };
                tokio::time::timeout(PATIENCE, asked).await
            })
        } else {
            let net = intelligence::Net::new(
                crate::cloud::runtime().handle().clone(),
                copper_core::version::app(),
            );
            let keys = eff.keys;
            crate::cloud::spawn(async move {
                let asked = async {
                    intelligence::router_ask(
                        &net,
                        &keys,
                        &model,
                        INSTRUCTIONS,
                        &user,
                        PATIENCE,
                        600,
                    )
                    .await
                    .map(|reply| reply.text)
                };
                tokio::time::timeout(PATIENCE, asked).await
            })
        };
        let answer = call.await;
        self.set_busy(false);
        match answer {
            Ok(Ok(Ok(text))) => Some(text),
            Ok(Ok(Err(e))) => {
                tracing::info!(error = %e, "tidy: the model failed; by site");
                None
            }
            _ => {
                tracing::info!("tidy: the model took too long; by site");
                None
            }
        }
    }

    /// Make or open the folders and move the tabs in; keep the Undo.
    fn file(&self, plan: &[(String, Vec<TabId>)], space: &str) -> usize {
        if plan.is_empty() {
            return 0;
        }
        let mine = self.space_folders(space);
        let mut made = Vec::new();
        let mut opened = Vec::new();
        let mut by_id: Vec<(String, Vec<TabId>)> = Vec::new();
        let mut fresh: Vec<TabGroup> = Vec::new();
        for (name, tabs) in plan {
            let id = match mine.iter().find(|g| g.name == *name) {
                Some(old) => {
                    if old.collapsed {
                        opened.push(old.id.clone());
                    }
                    old.id.clone()
                }
                None => {
                    // Plain, so it wears the space's colour, as Arc's folders do.
                    let g = TabGroup::new(name.as_str(), None);
                    made.push(g.id.clone());
                    let id = g.id.clone();
                    fresh.push(g);
                    id
                }
            };
            by_id.push((id, tabs.clone()));
        }
        let wrote = groups::edit(self, |file| {
            for g in file.groups.iter_mut() {
                if opened.contains(&g.id) {
                    g.collapsed = false;
                }
            }
            file.groups.extend(fresh);
        });
        if !wrote {
            return 0;
        }
        let moved = self.with_browser(|b| b.file_tabs(&by_id));
        if moved.is_empty() {
            return 0;
        }
        let count = moved.len();
        UNDO.with(|u| {
            *u.borrow_mut() = Some(Undo {
                moved,
                made,
                opened,
                space: space.to_string(),
                folders: plan.len(),
                until: Instant::now() + UNDO_FOR,
            })
        });
        count
    }

    /// While Undo is on offer: how many tabs it puts back, and from how
    /// many folders (the pill's words).
    pub fn tidy_undo_line(&self) -> Option<(usize, usize)> {
        UNDO.with(|u| {
            let mut slot = u.borrow_mut();
            match slot.as_ref() {
                Some(undo) if undo.until > Instant::now() => Some((undo.moved.len(), undo.folders)),
                Some(_) => {
                    // Expired: let it go.
                    *slot = None;
                    None
                }
                None => None,
            }
        })
    }

    /// Every tidied tab out of its folder and back where it stood, the
    /// folders it made gone and the ones it opened shut again (`Tidy.undo`).
    /// Returns how many came back.
    pub fn undo_tidy(&self) -> usize {
        let Some(undo) = UNDO.with(|u| u.borrow_mut().take()) else {
            return 0;
        };
        if undo.until <= Instant::now() {
            return 0;
        }
        if self.browser().current_space_id() != undo.space {
            self.switch_space(&undo.space);
        }
        let back = self.with_browser(|b| b.unfile_tabs(&undo.moved));
        let held: HashSet<String> = self
            .browser()
            .all_tabs()
            .filter_map(|t| t.group.as_deref().map(str::to_ascii_uppercase))
            .collect();
        groups::edit(self, |file| {
            file.groups.retain(|g| {
                !(undo.made.contains(&g.id) && !held.contains(&g.id.to_ascii_uppercase()))
            });
            for g in file.groups.iter_mut() {
                if undo.opened.contains(&g.id) {
                    g.collapsed = true;
                }
            }
        });
        // Asked for in so many words: the cloud's brake lets the unfiling through.
        crate::cloud::confirmed();
        self.view().refresh(
            self,
            &Refresh {
                tabs: true,
                ..Default::default()
            },
        );
        tracing::info!(back, "tidy undone");
        back
    }
}

#[cfg(test)]
mod tests {
    //! Mac `TidyPlanTests`, case for case (the row Undo puts back is
    //! copper-core's `browser::tidy`).
    use super::*;
    use copper_mcp::intelligence::json_in as json;

    impl Folder {
        fn new(name: &str, tabs: &[usize]) -> Folder {
            Folder {
                name: name.into(),
                tabs: tabs.to_vec(),
            }
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_the_models_folders() {
        let reply = json(
            r#"Sure: {"folders":[{"name":"Trip to Lisbon","tabs":[0,2]},{"name":"Reading","tabs":[1,3]}]}"#,
        );
        assert_eq!(
            parse(&reply, 5, &[]),
            [
                Folder::new("Trip to Lisbon", &[0, 2]),
                Folder::new("Reading", &[1, 3])
            ]
        );
    }

    #[test]
    fn drops_bad_indexes_repeats_and_lone_new_folders() {
        // 9 is out of range, 0 is taken by the first folder, "Solo" keeps one
        // tab and is new, so it goes and its tab stays loose.
        let reply = json(
            r#"{"folders":[{"name":"Work","tabs":[0,1,9]},{"name":"Other","tabs":[0,2,3]},{"name":"Solo","tabs":[4]}]}"#,
        );
        assert_eq!(
            parse(&reply, 5, &[]),
            [Folder::new("Work", &[0, 1]), Folder::new("Other", &[2, 3])]
        );
    }

    #[test]
    fn reuses_an_existing_folder_even_for_one_tab() {
        let reply =
            json(r#"{"folders":[{"name":"daily","tabs":[2]},{"name":"buildrfi","tabs":[0,1]}]}"#);
        assert_eq!(
            parse(&reply, 3, &s(&["Daily", "Misc › BuildrFi"])),
            [
                Folder::new("Daily", &[2]),
                Folder::new("Misc › BuildrFi", &[0, 1])
            ]
        );
    }

    #[test]
    fn new_names_cannot_nest_and_stay_short() {
        let long = "x".repeat(60);
        let reply = json(&format!(
            r#"{{"folders":[{{"name":"A › B","tabs":[0,1]}},{{"name":"{long}","tabs":[2,3]}}]}}"#
        ));
        let names: Vec<String> = parse(&reply, 4, &[]).into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["A - B".to_string(), "x".repeat(32)]);
    }

    #[test]
    fn nothing_usable_is_nothing() {
        assert!(parse(&json("no idea"), 4, &[]).is_empty());
        assert!(parse(&json(r#"{"folders":"nope"}"#), 4, &[]).is_empty());
        assert!(parse(&json(r#"{"folders":[{"name":"","tabs":[0,1]}]}"#), 4, &[]).is_empty());
    }

    #[test]
    fn at_most_six_folders() {
        let many: Vec<String> = (0..8)
            .map(|n| format!(r#"{{"name":"F{n}","tabs":[{},{}]}}"#, n * 2, n * 2 + 1))
            .collect();
        let reply = json(&format!(r#"{{"folders":[{}]}}"#, many.join(",")));
        assert_eq!(parse(&reply, 16, &[]).len(), 6);
    }

    #[test]
    fn by_site_needs_two_and_names_the_site() {
        let hosts = s(&[
            "github.com",
            "news.ycombinator.com",
            "gist.github.com",
            "apple.com",
            "",
            "bbc.co.uk",
            "bbc.co.uk",
        ]);
        assert_eq!(
            by_site(&hosts, &[]),
            [Folder::new("Github", &[0, 2]), Folder::new("Bbc", &[5, 6])]
        );
        // An existing folder of the site's name takes them.
        assert_eq!(
            by_site(&hosts, &s(&["GitHub"])).first(),
            Some(&Folder::new("GitHub", &[0, 2]))
        );
    }

    #[test]
    fn host_drops_www() {
        assert_eq!(host("https://www.Apple.com/mac"), "apple.com");
        assert_eq!(host(""), "");
    }

    #[test]
    fn question_holds_titles_and_hosts_only() {
        let text = question(
            &s(&["Inbox", &"t".repeat(300)]),
            &s(&["mail.google.com", "x.com"]),
            &s(&["Work"]),
        );
        let body = json(&text);
        let mut keys: Vec<&String> = body.keys().collect();
        keys.sort();
        assert_eq!(keys, ["existing", "tabs"]);
        let tabs = body["tabs"].as_array().unwrap();
        for tab in tabs {
            let mut keys: Vec<&String> = tab.as_object().unwrap().keys().collect();
            keys.sort();
            assert_eq!(keys, ["host", "i", "title"]);
        }
        assert_eq!(tabs[1]["title"].as_str().unwrap().chars().count(), 100);
    }

    #[test]
    fn the_pill_counts() {
        assert_eq!(tidied_words(1, 1), "Tidied 1 tab into 1 folder");
        assert_eq!(tidied_words(5, 2), "Tidied 5 tabs into 2 folders");
    }
}
