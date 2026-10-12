//! `bookmarks.json`: a tree of sites and folders (Mac: `Bookmarks.swift`).
//!
//! A node with a `url` is a site; without one it is a folder, whose
//! `children` may be absent (an empty folder written by an older build).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One node (`Bookmark`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Bookmark {
    /// UUID, uppercase.
    pub id: String,
    pub title: String,
    /// `None` for a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Bookmark>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Bookmark {
    /// A site. An empty title becomes the pretty address.
    pub fn site(title: &str, url: &str) -> Self {
        let title = if title.is_empty() {
            crate::address::pretty_str(url)
        } else {
            title.to_string()
        };
        Bookmark {
            id: crate::ids::new_uuid(),
            title,
            url: Some(url.to_string()),
            ..Default::default()
        }
    }

    /// A folder.
    pub fn folder(title: &str, children: Vec<Bookmark>) -> Self {
        Bookmark {
            id: crate::ids::new_uuid(),
            title: title.to_string(),
            url: None,
            children: Some(children),
            ..Default::default()
        }
    }

    /// No `url`.
    pub fn is_folder(&self) -> bool {
        self.url.is_none()
    }

    /// The children, or none.
    pub fn kids(&self) -> &[Bookmark] {
        self.children.as_deref().unwrap_or(&[])
    }

    /// `id` is this node or anywhere inside it.
    pub fn holds(&self, id: &str) -> bool {
        self.id == id || self.kids().iter().any(|k| k.holds(id))
    }
}

/// A search hit: the node and the folder titles above it.
#[derive(Debug, Clone, PartialEq)]
pub struct BookmarkHit {
    pub bookmark: Bookmark,
    pub path: Vec<String>,
}

/// The tree with the Mac's edit operations. Mutations bump
/// [`Bookmarks::generation`]; persist [`Bookmarks::roots`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bookmarks {
    roots: Vec<Bookmark>,
    generation: u64,
}

impl Bookmarks {
    /// From the file.
    pub fn new(roots: Vec<Bookmark>) -> Self {
        Bookmarks {
            roots,
            generation: 0,
        }
    }

    /// The top level, as written to the file.
    pub fn roots(&self) -> &[Bookmark] {
        &self.roots
    }

    /// Bumped on every change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Replace the whole tree (cloud sync's merged copy), ids kept.
    pub fn replace(&mut self, roots: Vec<Bookmark>) {
        self.roots = roots;
        self.generation += 1;
    }

    /// How many sites (folders are not counted).
    pub fn count(&self) -> usize {
        fn walk(nodes: &[Bookmark]) -> usize {
            nodes
                .iter()
                .map(|n| if n.is_folder() { walk(n.kids()) } else { 1 })
                .sum()
        }
        walk(&self.roots)
    }

    /// Every site's URL, folders opened, in order.
    pub fn urls(&self) -> Vec<String> {
        fn walk(nodes: &[Bookmark], out: &mut Vec<String>) {
            for n in nodes {
                match &n.url {
                    Some(u) => out.push(u.clone()),
                    None => walk(n.kids(), out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, &mut out);
        out
    }

    /// Every folder with its depth, for "move to folder" lists.
    pub fn folders(&self) -> Vec<(&Bookmark, usize)> {
        fn walk<'a>(nodes: &'a [Bookmark], depth: usize, out: &mut Vec<(&'a Bookmark, usize)>) {
            for n in nodes.iter().filter(|n| n.is_folder()) {
                out.push((n, depth));
                walk(n.kids(), depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.roots, 0, &mut out);
        out
    }

    /// The node with this id.
    pub fn find(&self, id: &str) -> Option<&Bookmark> {
        fn walk<'a>(nodes: &'a [Bookmark], id: &str) -> Option<&'a Bookmark> {
            nodes.iter().find_map(|n| {
                if n.id == id {
                    Some(n)
                } else {
                    walk(n.kids(), id)
                }
            })
        }
        walk(&self.roots, id)
    }

    /// Whether any site has exactly this URL.
    pub fn contains_url(&self, url: &str) -> bool {
        self.urls().iter().any(|u| u == url)
    }

    /// The page at the end of the top level, unless it is already kept.
    /// Returns the new node's id.
    pub fn add(&mut self, url: &str, title: &str) -> Option<String> {
        if self.contains_url(url) {
            return None;
        }
        let node = Bookmark::site(title, url);
        let id = node.id.clone();
        self.roots.push(node);
        self.generation += 1;
        Some(id)
    }

    /// File `node` under folder `parent` (or at the top level for `None` or a
    /// folder that isn't there). Returns the node's id.
    pub fn insert(&mut self, node: Bookmark, parent: Option<&str>) -> String {
        let id = node.id.clone();
        let node = match parent {
            Some(p) => match insert_into(&mut self.roots, p, node) {
                Ok(()) => {
                    self.generation += 1;
                    return id;
                }
                Err(node) => node,
            },
            None => node,
        };
        self.roots.push(node);
        self.generation += 1;
        id
    }

    /// Remove a node (and everything inside it).
    pub fn remove(&mut self, id: &str) -> bool {
        let gone = detach(&mut self.roots, id).is_some();
        if gone {
            self.generation += 1;
        }
        gone
    }

    /// Move a node to the end of folder `into` (or the top level). Moving a
    /// folder into itself or its own children is refused.
    pub fn move_to(&mut self, id: &str, into: Option<&str>) -> bool {
        if Some(id) == into {
            return false;
        }
        let mut working = self.roots.clone();
        let Some(node) = detach(&mut working, id) else {
            return false;
        };
        match into {
            Some(folder) => {
                if node.holds(folder) || insert_into(&mut working, folder, node).is_err() {
                    return false;
                }
            }
            None => working.push(node),
        }
        self.roots = working;
        self.generation += 1;
        true
    }

    /// New title and/or address (a folder's address is never set).
    pub fn update(&mut self, id: &str, title: Option<&str>, url: Option<&str>) -> bool {
        fn walk(nodes: &mut [Bookmark], id: &str, title: Option<&str>, url: Option<&str>) -> bool {
            for n in nodes.iter_mut() {
                if n.id == id {
                    if let Some(t) = title {
                        n.title = t.to_string();
                    }
                    if let (Some(u), false) = (url, n.is_folder()) {
                        n.url = Some(u.to_string());
                    }
                    return true;
                }
                if let Some(kids) = n.children.as_mut()
                    && walk(kids, id, title, url)
                {
                    return true;
                }
            }
            false
        }
        let done = walk(&mut self.roots, id, title, url);
        if done {
            self.generation += 1;
        }
        done
    }

    /// Sites whose title or URL contains `query` (case-insensitive), in tree
    /// order, each with its folder path.
    pub fn search(&self, query: &str) -> Vec<BookmarkHit> {
        let needle = query.trim().to_lowercase();
        let mut out = Vec::new();
        fn walk(
            nodes: &[Bookmark],
            needle: &str,
            path: &mut Vec<String>,
            out: &mut Vec<BookmarkHit>,
        ) {
            for n in nodes {
                match &n.url {
                    Some(u) => {
                        if needle.is_empty()
                            || n.title.to_lowercase().contains(needle)
                            || u.to_lowercase().contains(needle)
                        {
                            out.push(BookmarkHit {
                                bookmark: n.clone(),
                                path: path.clone(),
                            });
                        }
                    }
                    None => {
                        path.push(n.title.clone());
                        walk(n.kids(), needle, path, out);
                        path.pop();
                    }
                }
            }
        }
        walk(&self.roots, &needle, &mut Vec::new(), &mut out);
        out
    }
}

fn detach(nodes: &mut Vec<Bookmark>, id: &str) -> Option<Bookmark> {
    if let Some(i) = nodes.iter().position(|n| n.id == id) {
        return Some(nodes.remove(i));
    }
    nodes
        .iter_mut()
        .find_map(|n| n.children.as_mut().and_then(|kids| detach(kids, id)))
}

fn insert_into(nodes: &mut [Bookmark], folder: &str, node: Bookmark) -> Result<(), Bookmark> {
    let mut node = node;
    for n in nodes.iter_mut() {
        if n.id == folder && n.is_folder() {
            n.children.get_or_insert_with(Vec::new).push(node);
            return Ok(());
        }
        if let Some(kids) = n.children.as_mut() {
            match insert_into(kids, folder, node) {
                Ok(()) => return Ok(()),
                Err(back) => node = back,
            }
        }
    }
    Err(node)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_edits() {
        let mut b = Bookmarks::new(vec![]);
        let a = b.add("https://a.test/", "A").unwrap();
        assert!(b.add("https://a.test/", "A again").is_none());
        let f = b.insert(Bookmark::folder("Work", vec![]), None);
        let g = b.insert(Bookmark::folder("Deep", vec![]), Some(&f));
        assert!(b.move_to(&a, Some(&g)));
        assert_eq!(
            b.search("a.test")[0].path,
            vec!["Work".to_string(), "Deep".to_string()]
        );
        assert!(!b.move_to(&f, Some(&g)), "a folder cannot go inside itself");
        assert_eq!(b.count(), 1);
        assert!(b.update(&a, Some("Renamed"), None));
        assert_eq!(b.find(&a).unwrap().title, "Renamed");
        assert!(b.remove(&f));
        assert_eq!(b.count(), 0);
    }
}
