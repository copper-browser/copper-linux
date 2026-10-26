//! How two copies of one document become one again (Mac: `CloudMerge.swift`).
//! Pure: no browser, no network.
//!
//! Every whole-document domain merges three ways: the copy last known to be
//! on the server and applied here (the base), this device's copy now, and the
//! server's copy now. Something changed on one side only takes that side;
//! something changed on both takes the server's ("server wins"). Something
//! missing from one side that the base had was deleted there, and stays
//! deleted; something the base never had is new, and is kept. With no base —
//! the first sync of a domain — it is a plain union, the server winning where
//! both have the same thing. Nothing is lost on a first sync.

use copper_core::bookmarks::Bookmark;
use copper_core::cloud::{BookmarksDoc, CloudFolder, KeptTab, SettingsDoc, SpacesDoc};
use copper_core::session::Space;
use std::collections::{BTreeMap, HashMap, HashSet};

/// UUIDs compare as UUIDs (any case); strings that aren't one compare as
/// they are.
fn uuid_key(id: &str) -> String {
    id.to_ascii_uppercase()
}

/// The key for each kept tab: space, section, host, and which of that host's
/// tabs in the section it is (Mac: `CloudDocs.keys(for:)`).
pub fn kept_keys(tabs: &[(String, bool, String)]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    tabs.iter()
        .map(|(space, pinned, url)| {
            let host = url::Url::parse(url)
                .ok()
                .and_then(|u| {
                    u.host_str().map(|h| {
                        h.trim_start_matches('[')
                            .trim_end_matches(']')
                            .to_lowercase()
                    })
                })
                .unwrap_or_else(|| url.to_lowercase());
            let stem = format!(
                "{}|{}|{host}",
                space.to_lowercase(),
                if *pinned { "pin" } else { "saved" }
            );
            let n = seen.entry(stem.clone()).or_insert(0);
            let key = format!("{stem}|{n}");
            *n += 1;
            key
        })
        .collect()
}

/// Three-way merge of keyed lists. Order: the server's, unless only this
/// device reordered since the base; things new here go after.
pub fn list<T: PartialEq + Clone>(
    base: Option<&[T]>,
    local: &[T],
    server: &[T],
    id: impl Fn(&T) -> String,
) -> Vec<T> {
    fn index<'a, T>(items: &'a [T], id: &impl Fn(&T) -> String) -> HashMap<String, &'a T> {
        let mut out = HashMap::new();
        for item in items {
            out.entry(id(item)).or_insert(item);
        }
        out
    }
    let base_by = base.map(|b| index(b, &id));
    let local_by = index(local, &id);
    let server_by = index(server, &id);

    let mut chosen: HashMap<String, T> = HashMap::new();
    let keys: HashSet<&String> = local_by.keys().chain(server_by.keys()).collect();
    for key in keys {
        let mine = local_by.get(key).copied();
        let theirs = server_by.get(key).copied();
        let was = base_by.as_ref().and_then(|b| b.get(key).copied());
        match (mine, theirs) {
            (Some(mine), Some(theirs)) => {
                // Both have it: mine only if the server's is still the base's.
                let pick = match was {
                    Some(was) if theirs == was && mine != was => mine,
                    _ => theirs,
                };
                chosen.insert(key.clone(), pick.clone());
            }
            (Some(mine), None) => {
                // Only here: new here, or deleted there — kept if edited here.
                if was.is_none_or(|was| mine != was) {
                    chosen.insert(key.clone(), mine.clone());
                }
            }
            (None, Some(theirs)) => {
                if was.is_none_or(|was| theirs != was) {
                    chosen.insert(key.clone(), theirs.clone());
                }
            }
            (None, None) => {}
        }
    }

    let base_order: Vec<String> = base
        .map(|b| b.iter().map(&id).collect())
        .unwrap_or_default();
    let local_order: Vec<String> = local.iter().map(&id).collect();
    let server_order: Vec<String> = server.iter().map(&id).collect();
    let in_base = |k: &String| base_by.as_ref().is_some_and(|b| b.contains_key(k));
    let local_moved = base.is_some()
        && local_order
            .iter()
            .filter(|k| in_base(k))
            .collect::<Vec<_>>()
            != base_order
                .iter()
                .filter(|k| local_by.contains_key(*k))
                .collect::<Vec<_>>();
    let server_moved = base.is_none()
        || server_order
            .iter()
            .filter(|k| in_base(k))
            .collect::<Vec<_>>()
            != base_order
                .iter()
                .filter(|k| server_by.contains_key(*k))
                .collect::<Vec<_>>();
    let local_first = local_moved && !server_moved;
    let (first, second) = if local_first {
        (&local_order, &server_order)
    } else {
        (&server_order, &local_order)
    };
    let mut out = Vec::new();
    let mut placed = HashSet::new();
    for key in first.iter().chain(second.iter()) {
        if placed.contains(key) {
            continue;
        }
        if let Some(item) = chosen.get(key) {
            placed.insert(key.clone());
            out.push(item.clone());
        }
    }
    out
}

/// Spaces and their kept tabs.
pub fn spaces(base: Option<&SpacesDoc>, local: &SpacesDoc, server: &SpacesDoc) -> SpacesDoc {
    // The first sync of a second device: both have a "Home" (each made its own
    // on first launch). Same name, never synced here — the same space.
    let rekeyed_local;
    let local = if base.is_none() {
        rekeyed_local = rekeyed(local, server);
        &rekeyed_local
    } else {
        local
    };
    let spaces = list(
        base.map(|b| b.spaces.as_slice()),
        &local.spaces,
        &server.spaces,
        |s: &Space| uuid_key(&s.id),
    );
    let alive: HashSet<String> = spaces.iter().map(|s| uuid_key(&s.id)).collect();
    let tabs = list(
        base.map(|b| b.tabs.as_slice()),
        &local.tabs,
        &server.tabs,
        |t: &KeptTab| t.key.clone(),
    )
    .into_iter()
    .filter(|t| alive.contains(&uuid_key(&t.space)))
    .collect();
    // A server document without folders is an older build's: it knows
    // nothing of filing, so its blank `group`s are not unfilings.
    let tabs = refiled(tabs, local, server);
    SpacesDoc {
        v: 1,
        spaces,
        tabs,
        groups: groups(
            base.and_then(|b| b.groups.as_deref()),
            local.groups.as_deref(),
            server.groups.as_deref(),
        ),
        extra: Default::default(),
    }
}

/// Where the server said nothing about folders, each kept tab keeps the
/// folder this device had it in.
fn refiled(mut tabs: Vec<KeptTab>, local: &SpacesDoc, server: &SpacesDoc) -> Vec<KeptTab> {
    if server.groups.is_some() || local.groups.is_none() {
        return tabs;
    }
    let mine: HashMap<&str, &Option<String>> = local
        .tabs
        .iter()
        .map(|t| (t.key.as_str(), &t.group))
        .collect();
    for t in &mut tabs {
        if t.group.is_none()
            && let Some(g) = mine.get(t.key.as_str())
        {
            t.group = (*g).clone();
        }
    }
    tabs
}

/// The folders, merged like the spaces. A side that sent none (a build from
/// before folders travelled) has said nothing about them: the other side's
/// stand, rather than every folder reading as deleted (`CloudMerge.groups`).
pub fn groups(
    base: Option<&[CloudFolder]>,
    local: Option<&[CloudFolder]>,
    server: Option<&[CloudFolder]>,
) -> Option<Vec<CloudFolder>> {
    let Some(server) = server else {
        return local.map(<[CloudFolder]>::to_vec);
    };
    let Some(local) = local else {
        return Some(server.to_vec());
    };
    Some(list(base, local, server, |f: &CloudFolder| uuid_key(&f.id)))
}

/// Local spaces the server doesn't know, matched by name (ignoring case) to
/// server spaces this device doesn't have: old id → server id.
pub fn same_named(local: &[Space], server: &[Space]) -> BTreeMap<String, String> {
    let local_ids: HashSet<String> = local.iter().map(|s| uuid_key(&s.id)).collect();
    let server_ids: HashSet<String> = server.iter().map(|s| uuid_key(&s.id)).collect();
    let mut out = BTreeMap::new();
    let mut taken = HashSet::new();
    let norm = |name: &str| name.trim_matches(' ').to_lowercase();
    for space in local {
        if server_ids.contains(&uuid_key(&space.id)) {
            continue;
        }
        let name = norm(&space.name);
        if let Some(found) = server.iter().find(|s| {
            !local_ids.contains(&uuid_key(&s.id))
                && !taken.contains(&uuid_key(&s.id))
                && norm(&s.name) == name
        }) {
            out.insert(uuid_key(&space.id), found.id.clone());
            taken.insert(uuid_key(&found.id));
        }
    }
    out
}

/// `doc` with its same-named spaces taking the server's ids.
pub fn rekeyed(doc: &SpacesDoc, server: &SpacesDoc) -> SpacesDoc {
    let map = same_named(&doc.spaces, &server.spaces);
    let mut doc = doc.clone();
    if map.is_empty() {
        return doc;
    }
    for space in &mut doc.spaces {
        if let Some(to) = map.get(&uuid_key(&space.id)) {
            space.id = to.clone();
        }
    }
    for tab in &mut doc.tabs {
        let Some(to) = map.get(&uuid_key(&tab.space)) else {
            continue;
        };
        let old = tab.space.to_lowercase();
        tab.space = to.clone();
        if let Some(rest) = tab.key.strip_prefix(&old) {
            tab.key = format!("{}{rest}", to.to_lowercase());
        }
    }
    doc
}

/// The safe settings: server wins, except where only this device changed it
/// since the base.
pub fn settings(
    base: Option<&SettingsDoc>,
    local: &SettingsDoc,
    server: &SettingsDoc,
) -> SettingsDoc {
    let mut out = local.values.clone();
    for (key, value) in &server.values {
        if let Some(base) = base
            && base.values.get(key) == Some(value)
            && let Some(mine) = local.values.get(key)
            && mine != value
        {
            continue;
        }
        out.insert(key.clone(), value.clone());
    }
    SettingsDoc {
        v: 1,
        values: out,
        extra: Default::default(),
    }
}

/// A bookmark without its children, and where it hangs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    pub title: String,
    pub url: Option<String>,
}

/// The tree as a list, parents first.
pub fn flatten(roots: &[Bookmark]) -> Vec<Node> {
    fn walk(nodes: &[Bookmark], parent: Option<&str>, out: &mut Vec<Node>) {
        for node in nodes {
            out.push(Node {
                id: uuid_key(&node.id),
                parent: parent.map(uuid_key),
                title: node.title.clone(),
                url: node.url.clone(),
            });
            if let Some(kids) = &node.children {
                walk(kids, Some(&node.id), out);
            }
        }
    }
    let mut out = vec![];
    walk(roots, None, &mut out);
    out
}

/// The tree again: each node under its parent, in list order. A node whose
/// parent is gone, or not a folder, or would make a loop, goes to the top
/// level rather than disappearing.
pub fn tree(nodes: &[Node]) -> Vec<Bookmark> {
    let mut by: HashMap<&str, &Node> = HashMap::new();
    for n in nodes {
        by.entry(n.id.as_str()).or_insert(n);
    }
    let homed = |node: &Node| -> Option<String> {
        let parent = node.parent.as_deref()?;
        let folder = by.get(parent)?;
        if folder.url.is_some() {
            return None;
        }
        let mut seen: HashSet<&str> = HashSet::from([node.id.as_str()]);
        let mut at = Some(parent);
        while let Some(here) = at {
            if !seen.insert(here) {
                return None;
            }
            at = by.get(here).and_then(|n| n.parent.as_deref());
        }
        Some(parent.to_string())
    };
    let mut kids: HashMap<Option<String>, Vec<&Node>> = HashMap::new();
    for node in nodes {
        kids.entry(homed(node)).or_default().push(node);
    }
    fn build(
        parent: Option<String>,
        depth: usize,
        kids: &HashMap<Option<String>, Vec<&Node>>,
    ) -> Vec<Bookmark> {
        if depth >= 64 {
            return vec![];
        }
        kids.get(&parent)
            .map(|list| {
                list.iter()
                    .map(|node| Bookmark {
                        id: node.id.clone(),
                        title: node.title.clone(),
                        url: node.url.clone(),
                        children: node
                            .url
                            .is_none()
                            .then(|| build(Some(node.id.clone()), depth + 1, kids)),
                        extra: Default::default(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    build(None, 0, &kids)
}

/// The bookmarks tree. A folder deleted on one side takes the things filed in
/// it along, unless they were changed on the other side — then they surface
/// at the top level.
pub fn bookmarks(
    base: Option<&BookmarksDoc>,
    local: &BookmarksDoc,
    server: &BookmarksDoc,
) -> BookmarksDoc {
    let base_nodes = base.map(|b| flatten(&b.roots));
    let merged = list(
        base_nodes.as_deref(),
        &flatten(&local.roots),
        &flatten(&server.roots),
        |n: &Node| n.id.clone(),
    );
    BookmarksDoc {
        v: 1,
        roots: tree(&merged),
        extra: Default::default(),
    }
}

/// Whether a visit from elsewhere is one this device already has: the same
/// place within a second.
pub fn same_visit(a: f64, b: f64) -> bool {
    (a - b).abs() < 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(id: &str, name: &str) -> Space {
        Space {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn kept(space: &str, url: &str, title: &str, n: usize) -> KeptTab {
        let host = url::Url::parse(url)
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        KeptTab {
            group: None,
            key: format!("{}|pin|{host}|{n}", space.to_lowercase()),
            space: space.into(),
            url: url.into(),
            title: title.into(),
            pinned: true,
            pin: None,
            extra: Default::default(),
        }
    }

    fn doc(spaces: Vec<Space>, tabs: Vec<KeptTab>) -> SpacesDoc {
        SpacesDoc {
            groups: None,
            v: 1,
            spaces,
            tabs,
            extra: Default::default(),
        }
    }

    #[test]
    fn keys_number_the_same_site() {
        let keys = kept_keys(&[
            ("AAAA".into(), true, "https://Mail.Example.com/a".into()),
            ("AAAA".into(), true, "https://mail.example.com/b".into()),
            ("AAAA".into(), false, "https://mail.example.com/c".into()),
            ("AAAA".into(), true, "not a url".into()),
        ]);
        assert_eq!(
            keys,
            vec![
                "aaaa|pin|mail.example.com|0",
                "aaaa|pin|mail.example.com|1",
                "aaaa|saved|mail.example.com|0",
                "aaaa|pin|not a url|0",
            ]
        );
    }

    #[test]
    fn first_sync_is_a_union_with_homes_matched() {
        let local = doc(
            vec![space("L-HOME", "Home"), space("L-WORK", "Work")],
            vec![
                kept("L-HOME", "https://a.test/", "A", 0),
                kept("L-WORK", "https://w.test/", "W", 0),
            ],
        );
        let server = doc(
            vec![space("S-HOME", "home "), space("S-PLAY", "Play")],
            vec![
                kept("S-HOME", "https://b.test/", "B", 0),
                kept("S-PLAY", "https://p.test/", "P", 0),
            ],
        );
        let merged = spaces(None, &local, &server);
        let ids: Vec<&str> = merged.spaces.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["S-HOME", "S-PLAY", "L-WORK"]);
        // Nothing lost: every tab from both sides, local Home's tab moved to the server's Home.
        let mut urls: Vec<(&str, &str)> = merged
            .tabs
            .iter()
            .map(|t| (t.space.as_str(), t.url.as_str()))
            .collect();
        urls.sort();
        assert_eq!(
            urls,
            vec![
                ("L-WORK", "https://w.test/"),
                ("S-HOME", "https://a.test/"),
                ("S-HOME", "https://b.test/"),
                ("S-PLAY", "https://p.test/"),
            ]
        );
        let a = merged
            .tabs
            .iter()
            .find(|t| t.url == "https://a.test/")
            .unwrap();
        assert!(a.key.starts_with("s-home|"));
    }

    #[test]
    fn deletions_stay_deleted_and_edits_win() {
        let base = doc(
            vec![space("A", "A"), space("B", "B"), space("C", "C")],
            vec![],
        );
        // Here: B deleted, C renamed. There: A renamed, D added.
        let local = doc(vec![space("A", "A"), space("C", "C2")], vec![]);
        let server = doc(
            vec![
                space("A", "A2"),
                space("B", "B"),
                space("C", "C"),
                space("D", "D"),
            ],
            vec![],
        );
        let merged = spaces(Some(&base), &local, &server);
        let names: Vec<&str> = merged.spaces.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["A2", "C2", "D"]);
        // Both changed the same thing: the server's.
        let local = doc(vec![space("A", "mine")], vec![]);
        let server = doc(vec![space("A", "theirs")], vec![]);
        let base = doc(vec![space("A", "was")], vec![]);
        assert_eq!(
            spaces(Some(&base), &local, &server).spaces[0].name,
            "theirs"
        );
        // Deleted there but edited here: the edit stays.
        let local = doc(vec![space("A", "edited")], vec![]);
        let server = doc(vec![], vec![]);
        assert_eq!(
            spaces(Some(&base), &local, &server).spaces[0].name,
            "edited"
        );
    }

    #[test]
    fn order_follows_whoever_moved() {
        let base = vec!["a", "b", "c"];
        let local = vec!["c", "a", "b"];
        let server = vec!["a", "b", "c", "d"];
        let id = |s: &&str| s.to_string();
        assert_eq!(
            list(Some(&base), &local, &server, id),
            vec!["c", "a", "b", "d"]
        );
        let server = vec!["b", "a", "c"];
        assert_eq!(list(Some(&base), &local, &server, id), vec!["b", "a", "c"]);
        assert_eq!(
            list(None, &["x", "a"], &["a", "y"], id),
            vec!["a", "y", "x"]
        );
    }

    #[test]
    fn tabs_of_deleted_spaces_go() {
        let base = doc(
            vec![space("A", "A"), space("B", "B")],
            vec![kept("B", "https://b.test/", "B", 0)],
        );
        let local = doc(
            vec![space("A", "A")],
            vec![kept("B", "https://b.test/", "B", 0)],
        );
        let server = base.clone();
        let merged = spaces(Some(&base), &local, &server);
        assert_eq!(merged.spaces.len(), 1);
        assert!(merged.tabs.is_empty());
    }

    #[test]
    fn settings_server_wins_unless_only_here_changed() {
        let m = |pairs: &[(&str, &str)]| SettingsDoc {
            v: 1,
            values: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            extra: Default::default(),
        };
        let base = m(&[("look", "light"), ("shield", "true")]);
        let local = m(&[("look", "dark"), ("shield", "true"), ("glyph", "icons")]);
        let server = m(&[("look", "light"), ("shield", "false")]);
        let out = settings(Some(&base), &local, &server);
        assert_eq!(out.values["look"], "dark");
        assert_eq!(out.values["shield"], "false");
        assert_eq!(out.values["glyph"], "icons");
        let out = settings(None, &local, &server);
        assert_eq!(out.values["look"], "light");
    }

    fn bm(id: &str, title: &str, url: Option<&str>, kids: Option<Vec<Bookmark>>) -> Bookmark {
        Bookmark {
            id: id.into(),
            title: title.into(),
            url: url.map(str::to_string),
            children: kids,
            extra: Default::default(),
        }
    }

    #[test]
    fn bookmarks_merge_and_rehome() {
        let base = BookmarksDoc {
            v: 1,
            roots: vec![bm(
                "F",
                "Folder",
                None,
                Some(vec![bm("X", "x", Some("https://x.test/"), None)]),
            )],
            extra: Default::default(),
        };
        // Here: the folder deleted. There: X edited — it surfaces at the top.
        let local = BookmarksDoc {
            v: 1,
            roots: vec![],
            extra: Default::default(),
        };
        let server = BookmarksDoc {
            v: 1,
            roots: vec![bm(
                "F",
                "Folder",
                None,
                Some(vec![bm("X", "x2", Some("https://x.test/"), None)]),
            )],
            extra: Default::default(),
        };
        let out = bookmarks(Some(&base), &local, &server);
        assert_eq!(out.roots.len(), 1);
        assert_eq!(out.roots[0].id, "X");
        assert_eq!(out.roots[0].title, "x2");
        // First sync: union, folders keep their children.
        let local = BookmarksDoc {
            v: 1,
            roots: vec![bm("L", "mine", Some("https://l.test/"), None)],
            extra: Default::default(),
        };
        let out = bookmarks(None, &local, &server);
        assert_eq!(out.roots.len(), 2);
        assert_eq!(out.roots[0].children.as_ref().unwrap()[0].id, "X");
        assert_eq!(out.roots[1].id, "L");
        // A loop goes to the top level.
        let nodes = vec![
            Node {
                id: "A".into(),
                parent: Some("B".into()),
                title: "a".into(),
                url: None,
            },
            Node {
                id: "B".into(),
                parent: Some("A".into()),
                title: "b".into(),
                url: None,
            },
        ];
        let t = tree(&nodes);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn mac_fixture_documents_merge_with_themselves() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/mac/");
        let read = |name: &str| std::fs::read(format!("{root}{name}")).ok();
        if let Some(bytes) = read("cloud-spaces.json") {
            let d: SpacesDoc = serde_json::from_slice(&bytes).unwrap();
            let merged = spaces(Some(&d), &d, &d);
            assert_eq!(merged.spaces, d.spaces);
            // A key seen twice is one tab (the first), as on the Mac.
            let mut seen = HashSet::new();
            let unique: Vec<KeptTab> = d
                .tabs
                .iter()
                .filter(|t| seen.insert(t.key.clone()))
                .cloned()
                .collect();
            assert_eq!(merged.tabs, unique);
        }
        if let Some(bytes) = read("cloud-bookmarks.json") {
            let d: BookmarksDoc = serde_json::from_slice(&bytes).unwrap();
            let merged = bookmarks(None, &d, &d);
            assert_eq!(flatten(&merged.roots), flatten(&d.roots));
        }
    }
}
