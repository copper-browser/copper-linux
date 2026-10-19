//! `browser_groups`: tab groups as `groups.json` keeps them (Mac:
//! `Fork/Groups.swift`), membership on each tab (`Tab::group`, saved in the
//! session). This window does not draw groups yet; the file and the
//! membership round-trip with the Mac's.

use crate::Platform;
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
