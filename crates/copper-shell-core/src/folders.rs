//! Folders out of the groups (Mac: `Fork/Folders.swift`): a group whose name
//! is another's plus ` › ` and more is that one's child, so the tree is read
//! back out of the names. Nothing here owns anything: it takes one block's
//! rows (Saved or Today) and the groups, and says what the column draws, in
//! order, with a depth on every line — headers for the folders, rows for the
//! tabs, nothing under a folded folder.
//!
//! Left out from the Mac's: empty folders (they have no tab to hang off and
//! stand where their `slot` says).

use copper_core::groups::TabGroup;

/// What `arc-import` puts between a folder and the one inside it.
pub const MARK: &str = " › ";

/// One line of a block.
#[derive(Debug, Clone, PartialEq)]
pub enum FolderRow {
    /// A folder's line: `label` is its name below its parent's, `count` the
    /// tabs it holds (below it, all the way down).
    Head {
        id: String,
        label: String,
        depth: usize,
        count: usize,
        collapsed: bool,
        hue: Option<f64>,
    },
    /// The block's `index`th row, `depth` in (0: in no folder).
    Tab { index: usize, depth: usize },
}

/// Is `name` somewhere below `other`?
pub fn below(name: &str, other: &str) -> bool {
    name.len() > other.len() && name.starts_with(other) && name[other.len()..].starts_with(MARK)
}

/// The block's lines. `rows` are each row's group id (`Tab::group`), in row
/// order; `groups` is `groups.json`. A folder's rows are gathered after its
/// first one, nesting and all, so a folder split by other rows still draws
/// once (`Folders.gathered`).
pub fn plan(rows: &[Option<&str>], groups: &[TabGroup]) -> Vec<FolderRow> {
    let by_id = |id: &str| groups.iter().find(|g| g.id.eq_ignore_ascii_case(id));
    // Each row's folder and that folder's ancestors that exist, outermost
    // first.
    let chain = |g: &TabGroup| -> Vec<TabGroup> {
        let mut up: Vec<TabGroup> = groups
            .iter()
            .filter(|o| {
                below(&g.name, &o.name)
                    && (o.space.is_none() || g.space.is_none() || o.space == g.space)
            })
            .cloned()
            .collect();
        up.sort_by_key(|o| o.name.len());
        // One per depth: two folders of one name (two spaces') would nest twice.
        up.dedup_by(|a, b| a.name == b.name);
        up.push(g.clone());
        up
    };
    let chains: Vec<Vec<TabGroup>> = rows
        .iter()
        .map(|r| r.and_then(by_id).map(chain).unwrap_or_default())
        .collect();
    let order = gathered(&chains);
    let weight = |id: &str| {
        chains
            .iter()
            .filter(|c| c.iter().any(|g| g.id == id))
            .count()
    };
    let mut out = Vec::new();
    let mut open: Vec<TabGroup> = Vec::new();
    for index in order {
        let c = &chains[index];
        let keep = open
            .iter()
            .zip(c.iter())
            .take_while(|(a, b)| a.id == b.id)
            .count();
        open.truncate(keep);
        for g in &c[keep..] {
            let depth = open.len();
            let hidden = open.iter().any(|o| o.collapsed);
            if !hidden {
                let label = match open.last() {
                    Some(p) if below(&g.name, &p.name) => {
                        g.name[p.name.len() + MARK.len()..].to_string()
                    }
                    _ => g.name.clone(),
                };
                out.push(FolderRow::Head {
                    id: g.id.clone(),
                    label,
                    depth,
                    count: weight(&g.id),
                    collapsed: g.collapsed,
                    hue: g.hue,
                });
            }
            open.push(g.clone());
        }
        if !c.iter().any(|g| g.collapsed) {
            out.push(FolderRow::Tab {
                index,
                depth: c.len(),
            });
        }
    }
    out
}

/// Row indices with every folder's rows side by side after its first one.
fn gathered(chains: &[Vec<TabGroup>]) -> Vec<usize> {
    fn gather(items: Vec<usize>, chains: &[Vec<TabGroup>], depth: usize) -> Vec<usize> {
        let mut out: Vec<Result<usize, String>> = Vec::new();
        let mut buckets: Vec<(String, Vec<usize>)> = Vec::new();
        for i in items {
            match chains[i].get(depth) {
                None => out.push(Ok(i)),
                Some(g) => match buckets.iter_mut().find(|(id, _)| *id == g.id) {
                    Some((_, b)) => b.push(i),
                    None => {
                        buckets.push((g.id.clone(), vec![i]));
                        out.push(Err(g.id.clone()));
                    }
                },
            }
        }
        out.into_iter()
            .flat_map(|piece| match piece {
                Ok(i) => vec![i],
                Err(id) => {
                    let items = buckets
                        .iter()
                        .find(|(b, _)| *b == id)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    gather(items, chains, depth + 1)
                }
            })
            .collect()
    }
    gather((0..chains.len()).collect(), chains, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(id: &str, name: &str, collapsed: bool) -> TabGroup {
        TabGroup {
            id: id.into(),
            name: name.into(),
            collapsed,
            ..Default::default()
        }
    }

    fn heads(p: &[FolderRow]) -> Vec<String> {
        p.iter()
            .map(|r| match r {
                FolderRow::Head {
                    label,
                    depth,
                    count,
                    ..
                } => format!("{depth}:{label}({count})"),
                FolderRow::Tab { index, depth } => format!("{depth}:#{index}"),
            })
            .collect()
    }

    #[test]
    fn nests_by_name_and_gathers() {
        let groups = [
            g("A", "Exowatt", false),
            g("B", "Exowatt › Apps", false),
            g("C", "Docs", false),
        ];
        // Loose, Apps, Docs, Exowatt itself, Apps again.
        let rows = [None, Some("B"), Some("C"), Some("A"), Some("b")];
        assert_eq!(
            heads(&plan(&rows, &groups)),
            [
                "0:#0",
                "0:Exowatt(3)",
                "1:Apps(2)",
                "2:#1",
                "2:#4",
                "1:#3",
                "0:Docs(1)",
                "1:#2"
            ]
        );
    }

    #[test]
    fn a_shut_folder_hides_everything_inside() {
        let groups = [g("A", "Exowatt", true), g("B", "Exowatt › Apps", false)];
        let rows = [Some("B"), Some("A"), None];
        assert_eq!(heads(&plan(&rows, &groups)), ["0:Exowatt(2)", "0:#2"]);
    }

    #[test]
    fn a_missing_parent_keeps_the_path() {
        let groups = [g("B", "Archive › Grunts", false)];
        assert_eq!(
            heads(&plan(&[Some("B")], &groups)),
            ["0:Archive › Grunts(1)", "1:#0"]
        );
    }
}
