//! Chromium's `AccountBookmarks` and `Bookmarks` (`FlowBookmarks.bookmarks`).
//!
//! Both files have the same tree shape. Per profile the account file comes
//! first and wins each duplicate; local-only sites and folders follow, then
//! the next profile. The bar's contents become the top level; "Other" and
//! "Mobile" become folders at its foot. Only http(s) addresses with a host
//! are kept. A file that is there but can't be read is reported, so a caller
//! never takes a partial read as "everything was removed".

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use copper_core::bookmarks::Bookmark;
use serde_json::{Map, Value};

/// What a read found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Read {
    pub nodes: Vec<Bookmark>,
    pub failed_files: Vec<PathBuf>,
}

impl Read {
    pub fn complete(&self) -> bool {
        self.failed_files.is_empty()
    }

    /// Sites, folders opened.
    pub fn count(&self) -> usize {
        count(&self.nodes)
    }
}

/// Sites in a tree, folders opened (`FlowBookmarks.count`).
pub fn count(nodes: &[Bookmark]) -> usize {
    nodes
        .iter()
        .map(|n| if n.is_folder() { count(n.kids()) } else { 1 })
        .sum()
}

/// Read every profile folder's bookmark files.
pub fn read(profile_dirs: &[PathBuf]) -> Read {
    let mut bar = Vec::new();
    let mut other = Vec::new();
    let mut mobile = Vec::new();
    let mut failed = Vec::new();
    for profile in profile_dirs {
        for name in ["AccountBookmarks", "Bookmarks"] {
            let file = profile.join(name);
            if !file.exists() {
                continue;
            }
            match parse_file(&file) {
                Some((b, o, m)) => {
                    merge(b, &mut bar);
                    merge(o, &mut other);
                    merge(m, &mut mobile);
                }
                None => failed.push(file),
            }
        }
    }
    if !other.is_empty() {
        bar.push(Bookmark::folder("Other", other));
    }
    if !mobile.is_empty() {
        bar.push(Bookmark::folder("Mobile", mobile));
    }
    Read {
        nodes: bar,
        failed_files: failed,
    }
}

type Roots = (Vec<Bookmark>, Vec<Bookmark>, Vec<Bookmark>);

fn parse_file(file: &Path) -> Option<Roots> {
    let data = crate::copy_read(file).ok()?;
    let top: Value = serde_json::from_slice(&data).ok()?;
    let roots = top.get("roots")?.as_object()?;
    if !["bookmark_bar", "other", "synced"]
        .iter()
        .any(|k| roots.contains_key(*k))
    {
        return None;
    }
    let bar = nodes(children(roots, "bookmark_bar")?)?;
    let other = nodes(children(roots, "other")?)?;
    let mobile = nodes(children(roots, "synced")?)?;
    Some((bar, other, mobile))
}

fn children<'a>(roots: &'a Map<String, Value>, key: &str) -> Option<&'a [Value]> {
    match roots.get(key) {
        None => Some(&[]),
        Some(root) => root.get("children")?.as_array().map(Vec::as_slice),
    }
}

fn nodes(raw: &[Value]) -> Option<Vec<Bookmark>> {
    let mut found = Vec::new();
    for entry in raw {
        let name = entry.get("name")?.as_str()?;
        match entry.get("type").and_then(Value::as_str) {
            Some("folder") => {
                let kids = nodes(entry.get("children")?.as_array()?)?;
                found.push(Bookmark::folder(name, kids));
            }
            Some("url") => {
                let text = entry.get("url")?.as_str()?;
                let ok = url::Url::parse(text).is_ok_and(|u| {
                    matches!(u.scheme(), "http" | "https")
                        && u.host_str().is_some_and(|h| !h.is_empty())
                });
                if ok {
                    found.push(Bookmark::site(name, text));
                }
            }
            _ => return None,
        }
    }
    Some(found)
}

/// Merge matching folders (by title) in place and identical URL + title at
/// the same level; a different title for the same URL stays apart.
pub fn merge(incoming: Vec<Bookmark>, result: &mut Vec<Bookmark>) {
    if incoming.is_empty() {
        return;
    }
    let mut folders: HashMap<String, usize> = HashMap::new();
    let mut sites: HashSet<(Option<String>, String)> = HashSet::new();
    for (i, node) in result.iter().enumerate() {
        if node.is_folder() {
            folders.entry(node.title.clone()).or_insert(i);
        } else {
            sites.insert((node.url.clone(), node.title.clone()));
        }
    }
    let mut pending: HashMap<usize, Vec<Bookmark>> = HashMap::new();
    let mut order = Vec::new();
    for node in incoming {
        if node.is_folder() {
            if let Some(&i) = folders.get(&node.title) {
                if !pending.contains_key(&i) {
                    order.push(i);
                }
                pending
                    .entry(i)
                    .or_default()
                    .extend(node.children.unwrap_or_default());
            } else {
                folders.insert(node.title.clone(), result.len());
                result.push(node);
            }
        } else if sites.insert((node.url.clone(), node.title.clone())) {
            result.push(node);
        }
    }
    for i in order {
        let mut kids = result[i].children.take().unwrap_or_default();
        merge(pending.remove(&i).unwrap_or_default(), &mut kids);
        result[i].children = Some(kids);
    }
}
