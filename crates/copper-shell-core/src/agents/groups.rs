//! `browser_groups`: tab groups as `groups.json` keeps them (Mac:
//! `Fork/Groups.swift`), membership on each tab (`Tab::group`, saved in the
//! session). The file and the membership round-trip with the Mac's; the
//! column draws them as folders ([`crate::folders`], [`crate::sidedrop`]).

use crate::Platform;
use crate::folders::{MARK, below};
use crate::shell::Shell;
use copper_core::TabId;
use copper_core::groups::{GroupsFile, TabGroup};
use copper_mcp::GroupInfo;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::SystemTime;

fn load<P: Platform>(shell: &Shell<P>) -> GroupsFile {
    shell
        .store()
        .load::<GroupsFile>()
        .map(|l| l.value)
        .unwrap_or_default()
}

/// Every group in `groups.json`, read again only when the file changed.
pub fn all<P: Platform>(shell: &Shell<P>) -> Vec<TabGroup> {
    thread_local! {
        static ALL: RefCell<Option<(Option<SystemTime>, Vec<TabGroup>)>> = const { RefCell::new(None) };
    }
    let path = shell.store().path(copper_core::DataFile::Groups);
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let Some(hit) = ALL.with(|a| {
        a.borrow()
            .as_ref()
            .filter(|(at, _)| at.is_some() && *at == stamp)
            .map(|(_, g)| g.clone())
    }) {
        return hit;
    }
    let groups = load(shell).groups;
    ALL.with(|a| *a.borrow_mut() = Some((stamp, groups.clone())));
    groups
}

/// Open a folder or shut it (each device's own; never synced).
pub fn toggle<P: Platform>(shell: &Shell<P>, id: &str) {
    let mut file = load(shell);
    let Some(g) = file
        .groups
        .iter_mut()
        .find(|g| g.id.eq_ignore_ascii_case(id))
    else {
        return;
    };
    g.collapsed = !g.collapsed;
    if let Err(e) = shell.store().save(&file) {
        tracing::warn!(error = %e, "could not write groups.json");
        return;
    }
    crate::view::ShellView::refresh(
        shell.view(),
        shell,
        &crate::view::Refresh {
            tabs: true,
            ..Default::default()
        },
    );
}

/// `groups.json`'s names, and the file's modification time they are from.
type Names = (Option<SystemTime>, HashMap<String, String>);

thread_local! {
    /// Cached: the tab list asks several times per tool call.
    static NAMES: RefCell<Option<Names>> = const { RefCell::new(None) };
}

/// Group id → name.
pub fn names<P: Platform>(shell: &Shell<P>) -> HashMap<String, String> {
    let path = shell.store().path(copper_core::DataFile::Groups);
    let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    if let Some(names) = NAMES.with(|n| {
        n.borrow()
            .as_ref()
            .filter(|(at, _)| *at == stamp)
            .map(|(_, names)| names.clone())
    }) {
        return names;
    }
    let names: HashMap<String, String> = load(shell)
        .groups
        .into_iter()
        .map(|g| (g.id, g.name))
        .collect();
    NAMES.with(|n| *n.borrow_mut() = Some((stamp, names.clone())));
    names
}

/// Every group with the titles of its tabs, in the order the window has them.
pub fn list<P: Platform>(shell: &Shell<P>) -> Vec<GroupInfo> {
    let file = load(shell);
    let browser = shell.browser();
    let mut order: Vec<&copper_core::Tab> = browser.visible();
    let shown: Vec<TabId> = order.iter().map(|t| t.id.clone()).collect();
    order.extend(browser.all_tabs().filter(|t| !shown.contains(&t.id)));
    file.groups
        .iter()
        .map(|g| GroupInfo {
            name: g.name.clone(),
            members: order
                .iter()
                .filter(|t| t.group.as_deref() == Some(g.id.as_str()))
                .map(|t| t.label())
                .collect(),
        })
        .collect()
}

/// Put the tab in the group called `name` (any case), made if new with the
/// next of eight hues, as the Mac does.
pub fn assign<P: Platform>(shell: &Shell<P>, id: &TabId, name: &str) {
    let clean = name.trim();
    if clean.is_empty() {
        return;
    }
    let mut file = load(shell);
    let wanted = clean.to_lowercase();
    let gid = match file
        .groups
        .iter_mut()
        .find(|g| g.name.to_lowercase() == wanted)
    {
        Some(g) => {
            g.collapsed = false;
            g.id.clone()
        }
        None => {
            let hue = (file.groups.len() % 8) as f64 / 8.0;
            let g = TabGroup::new(clean, Some(hue));
            let gid = g.id.clone();
            file.groups.push(g);
            gid
        }
    };
    if let Err(e) = shell.store().save(&file) {
        tracing::warn!(error = %e, "could not write groups.json");
        return;
    }
    NAMES.with(|n| n.borrow_mut().take());
    shell.with_browser(|b| b.set_group(id, Some(&gid)));
}

/// Take the tab out of its group.
pub fn remove<P: Platform>(shell: &Shell<P>, id: &TabId) {
    shell.with_browser(|b| b.set_group(id, None));
}

/// Remove from Folder (`Groups.takeOut`): out of every folder, onto the
/// line just under the outermost one it was in, in its own block. Left
/// where it stood, it would split that folder in two around it.
pub fn take_out<P: Platform>(shell: &Shell<P>, id: &TabId) {
    let groups = all(shell);
    let name = |g: Option<&str>| {
        g.and_then(|g| groups.iter().find(|x| x.id.eq_ignore_ascii_case(g)))
            .map(|x| x.name.as_str())
    };
    let to = {
        let b = shell.browser();
        let Some(tab) = b.tab(id) else { return };
        let row = b.row(&tab.space);
        let filed: Vec<(bool, Option<&str>)> = row
            .iter()
            .map(|t| (t.saved, name(t.group.as_deref())))
            .collect();
        row.iter()
            .position(|t| t.id == *id)
            .and_then(|here| under_folder(&filed, here))
    };
    shell.with_browser(|b| {
        b.set_group(id, None);
        if let Some(to) = to {
            b.move_tab(id, to);
        }
    });
}

/// Where Remove from Folder moves row `here` of `row` (each tab's block
/// and folder name): onto the last place of the outermost folder it is in,
/// in its block, when that is further down.
fn under_folder(row: &[(bool, Option<&str>)], here: usize) -> Option<usize> {
    let (saved, Some(name)) = row[here] else {
        return None;
    };
    let root = name.split(MARK).next().unwrap_or(name);
    row.iter()
        .rposition(|(s, n)| *s == saved && n.is_some_and(|n| n == root || below(n, root)))
        .filter(|last| *last > here)
}

/// Is a tab filed in `group` in `folder`, or in a folder inside it?
pub fn in_folder(groups: &[TabGroup], group: Option<&str>, folder: &str) -> bool {
    let name = |id: &str| {
        groups
            .iter()
            .find(|g| g.id.eq_ignore_ascii_case(id))
            .map(|g| g.name.as_str())
    };
    let Some(mine) = group else { return false };
    mine.eq_ignore_ascii_case(folder)
        || matches!((name(mine), name(folder)), (Some(a), Some(b)) if below(a, b))
}

/// The folders as Copper Cloud carries them (`CloudDocs.Folder`): every
/// group but whether it is open.
pub fn folders<P: Platform>(shell: &Shell<P>) -> Vec<copper_core::cloud::CloudFolder> {
    load(shell)
        .groups
        .into_iter()
        .map(|g| copper_core::cloud::CloudFolder {
            id: g.id,
            name: g.name,
            hue: g.hue,
            space: g.space,
            slot: g.slot,
            extra: Default::default(),
        })
        .collect()
}

/// Another device's folders (`Groups.cloudAdopt`): each added or edited by
/// id, those `removed` taken away (their tabs stay, unfiled), folders only
/// this device has kept after. A folder new here arrives shut, as the Mac's
/// imported ones are; open or shut otherwise stays as it was here.
pub fn adopt<P: Platform>(
    shell: &Shell<P>,
    folders: &[copper_core::cloud::CloudFolder],
    removed: &[String],
) {
    let mut file = load(shell);
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let gone = |id: &str| removed.iter().any(|r| same(r, id));
    let mut next: Vec<TabGroup> = folders
        .iter()
        .map(|f| {
            let mut g = file
                .groups
                .iter()
                .find(|g| same(&g.id, &f.id))
                .cloned()
                .unwrap_or_else(|| TabGroup {
                    id: f.id.clone(),
                    collapsed: true,
                    ..Default::default()
                });
            g.name = f.name.clone();
            g.hue = f.hue;
            g.space = f.space.clone();
            g.slot = f.slot;
            g
        })
        .collect();
    next.extend(
        file.groups
            .iter()
            .filter(|g| !gone(&g.id) && !folders.iter().any(|f| same(&f.id, &g.id)))
            .cloned(),
    );
    if next != file.groups {
        file.groups = next;
        if let Err(e) = shell.store().save(&file) {
            tracing::warn!(error = %e, "could not write groups.json");
            return;
        }
        NAMES.with(|n| n.borrow_mut().take());
    }
    if !removed.is_empty() {
        let filed: Vec<TabId> = shell
            .browser()
            .all_tabs()
            .filter(|t| t.group.as_deref().is_some_and(gone))
            .map(|t| t.id.clone())
            .collect();
        shell.with_browser(|b| {
            for id in &filed {
                b.set_group(id, None);
            }
        });
    }
    crate::view::ShellView::refresh(shell.view(), shell, &crate::view::Refresh::spaces());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taken_out_it_goes_under_the_outermost_folder_in_its_block() {
        let w = Some("Work");
        let docs = Some("Work › Docs");
        // a, then Work: b, Work › Docs: c, Work: d; Today: e in Work.
        let row = [(true, None), (true, w), (true, docs), (true, w), (false, w)];
        // c, in the inner folder, goes under d: out of both.
        assert_eq!(under_folder(&row, 2), Some(3));
        // d is already the folder's last: it stays.
        assert_eq!(under_folder(&row, 3), None);
        // e's Work in Today is its own run.
        assert_eq!(under_folder(&row, 4), None);
        // A loose tab has no folder to leave.
        assert_eq!(under_folder(&row, 0), None);
    }

    #[test]
    fn in_a_folder_or_one_inside_it() {
        let groups = [
            TabGroup {
                id: "W".into(),
                name: "Work".into(),
                ..Default::default()
            },
            TabGroup {
                id: "D".into(),
                name: "Work › Docs".into(),
                ..Default::default()
            },
        ];
        assert!(in_folder(&groups, Some("d"), "W"));
        assert!(in_folder(&groups, Some("W"), "w"));
        assert!(!in_folder(&groups, Some("W"), "D"));
        assert!(!in_folder(&groups, None, "W"));
    }
}
