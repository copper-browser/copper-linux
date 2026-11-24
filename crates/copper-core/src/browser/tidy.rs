//! Tidy, the model half (Mac: `Fork/Tidy.swift`, `Tidy.apply` / `Tidy.undo`
//! and `TidyPlan.restored`): file loose rows of the current space into
//! folders, and put them back exactly where they were.
//!
//! Which rows and which folders is the shell's call (`copper-shell-core`'s
//! `tidy`); this moves the rows the way `Groups.assign` does — into the
//! folder, next to its other rows, or at the foot of Saved for a folder's
//! first — and remembers each row's old place for the Undo.

use super::{Browser, TabId};

/// The row as it was: `now` without the `moved` ids, each put back at its
/// old index, lowest first. Exactly the old row when nothing else moved; a
/// tab closed since is skipped, and one opened since keeps its place among
/// the others (`TidyPlan.restored`).
pub fn restored<I: PartialEq + Clone>(now: &[I], moved: &[(I, usize)]) -> Vec<I> {
    let mut row: Vec<I> = now
        .iter()
        .filter(|id| !moved.iter().any(|(m, _)| m == *id))
        .cloned()
        .collect();
    let mut back: Vec<&(I, usize)> = moved.iter().filter(|(id, _)| now.contains(id)).collect();
    back.sort_by_key(|(_, at)| *at);
    for (id, at) in back {
        row.insert((*at).min(row.len()), id.clone());
    }
    row
}

impl Browser {
    /// File rows of the current space into groups: `plan` is each group's
    /// id and the tabs going into it. A tab joins its group's other rows in
    /// this space (after the last of them, in their block); a group's first
    /// goes to the foot of Saved, where a new folder stands. Returns each
    /// filed tab with its index in the row before, for [`Browser::unfile_tabs`].
    pub fn file_tabs(&mut self, plan: &[(String, Vec<TabId>)]) -> Vec<(TabId, usize)> {
        let space = self.current.clone();
        let Some(order) = self.row_of(&space).map(|r| r.tabs.clone()) else {
            return Vec::new();
        };
        let mut moved = Vec::new();
        for (group, ids) in plan {
            for id in ids {
                let Some(at) = order.iter().position(|t| t == id) else {
                    continue;
                };
                moved.push((id.clone(), at));
                // Each row: id, Saved, already in this group.
                let row: Vec<(TabId, bool, bool)> = self
                    .row(&space)
                    .iter()
                    .map(|t| {
                        let member = t
                            .group
                            .as_deref()
                            .is_some_and(|g| g.eq_ignore_ascii_case(group));
                        (t.id.clone(), t.saved, member)
                    })
                    .collect();
                self.set_group(id, Some(group));
                match row.iter().rposition(|(t, _, member)| t != id && *member) {
                    Some(last) => {
                        let saved = row[last].1;
                        self.update_tab(id, |t| t.saved = saved && !t.private);
                        let here = row.iter().position(|(t, ..)| t == id).unwrap_or(last);
                        self.move_tab(id, if last < here { last + 1 } else { last });
                    }
                    None => {
                        self.place_in_section(id, true, None);
                    }
                }
            }
        }
        moved
    }

    /// Undo [`Browser::file_tabs`]: every tab still here out of its folder
    /// and back in Today, and the current space's row put back as it was
    /// ([`restored`]). Returns how many came back.
    pub fn unfile_tabs(&mut self, moved: &[(TabId, usize)]) -> usize {
        let mut back = 0;
        for (id, _) in moved {
            if self.tabs.get(id).is_some_and(|t| !t.is_pinned()) {
                self.update_tab(id, |t| {
                    t.group = None;
                    t.saved = false;
                });
                back += 1;
            }
        }
        let space = self.current.clone();
        let now: Vec<TabId> = self
            .row_of(&space)
            .map(|r| r.tabs.clone())
            .unwrap_or_default();
        for (i, id) in restored(&now, moved).iter().enumerate() {
            if self.row_of(&space).and_then(|r| r.tabs.get(i)) != Some(id) {
                self.move_tab(id, i);
            }
        }
        back
    }
}

#[cfg(test)]
mod tests {
    use super::restored;
    use crate::browser::{Browser, OpenOptions, TabId};

    // Mac `TidyPlanTests.undoPutsTheRowBackExactly`.
    #[test]
    fn undo_puts_the_row_back_exactly() {
        // pins p, then Saved s1, then Today t1…t5; t2 and t4 were filed into
        // a folder at the foot of Saved, t5 into another.
        let before = ["p", "s1", "t1", "t2", "t3", "t4", "t5"];
        let after = ["p", "s1", "t2", "t4", "t5", "t1", "t3"];
        let moved = [("t2", 3), ("t4", 5), ("t5", 6)];
        assert_eq!(restored(&after, &moved), before);
    }

    // Mac `TidyPlanTests.undoCopesWithTabsClosedOrOpenedSince`.
    #[test]
    fn undo_copes_with_tabs_closed_or_opened_since() {
        let moved = [("t2", 2), ("t4", 4)];
        // t4 closed since: only t2 goes back.
        assert_eq!(
            restored(&["s1", "t2", "t1", "t3"], &moved),
            ["s1", "t1", "t2", "t3"]
        );
        // a new tab n opened at the top of Today: everything is still there once.
        let out = restored(&["s1", "t2", "t4", "n", "t1", "t3"], &moved);
        let mut sorted = out.clone();
        sorted.sort();
        assert_eq!(sorted, ["n", "s1", "t1", "t2", "t3", "t4"]);
        assert_eq!(out.iter().position(|t| *t == "t2"), Some(2));
        assert_eq!(out.iter().position(|t| *t == "t4"), Some(4));
    }

    fn open(b: &mut Browser, url: &str) -> TabId {
        b.open_tab(
            url,
            OpenOptions {
                at_end: true,
                ..Default::default()
            },
        )
    }

    fn row(b: &Browser) -> Vec<(String, bool, Option<String>)> {
        b.row(b.current_space_id())
            .iter()
            .map(|t| (t.url.clone(), t.saved, t.group.clone()))
            .collect()
    }

    #[test]
    fn filed_into_saved_then_put_back() {
        let mut b = Browser::new();
        let s1 = open(&mut b, "s1");
        let t: Vec<TabId> = (1..=5).map(|n| open(&mut b, &format!("t{n}"))).collect();
        b.update_tab(&s1, |t| {
            t.saved = true;
            t.group = Some("HOME".into());
        });
        let before = row(&b);
        // t2 and t4 into a new folder; t5 into the one s1 is in (any case).
        let moved = b.file_tabs(&[
            ("NEW".into(), vec![t[1].clone(), t[3].clone()]),
            ("home".into(), vec![t[4].clone()]),
        ]);
        assert_eq!(
            moved,
            [(t[1].clone(), 2), (t[3].clone(), 4), (t[4].clone(), 5)]
        );
        let urls: Vec<String> = row(&b).into_iter().map(|r| r.0).collect();
        assert_eq!(urls, ["s1", "t5", "t2", "t4", "t1", "t3"]);
        for id in [&t[1], &t[3], &t[4]] {
            assert!(b.tab(id).unwrap().saved, "a filed tab is in Saved");
        }
        assert_eq!(b.tab(&t[1]).unwrap().group.as_deref(), Some("NEW"));
        assert_eq!(b.tab(&t[4]).unwrap().group.as_deref(), Some("home"));

        assert_eq!(b.unfile_tabs(&moved), 3);
        assert_eq!(row(&b), before);
    }

    #[test]
    fn a_folder_in_today_takes_its_tab_in_today() {
        let mut b = Browser::new();
        let a = open(&mut b, "a");
        let k = open(&mut b, "k");
        let c = open(&mut b, "c");
        b.update_tab(&k, |t| t.group = Some("G".into()));
        b.file_tabs(&[("G".into(), vec![a.clone(), c.clone()])]);
        let urls: Vec<String> = row(&b).into_iter().map(|r| r.0).collect();
        assert_eq!(urls, ["k", "a", "c"]);
        assert!(!b.tab(&a).unwrap().saved && !b.tab(&c).unwrap().saved);
    }

    #[test]
    fn undo_skips_a_tab_closed_since() {
        let mut b = Browser::new();
        let a = open(&mut b, "a");
        open(&mut b, "c");
        let d = open(&mut b, "d");
        let moved = b.file_tabs(&[("G".into(), vec![a.clone(), d.clone()])]);
        b.close_tab(&a, true);
        assert_eq!(b.unfile_tabs(&moved), 1);
        let urls: Vec<String> = row(&b).into_iter().map(|r| r.0).collect();
        assert_eq!(urls, ["c", "d"]);
        assert!(b.tab(&d).unwrap().group.is_none() && !b.tab(&d).unwrap().saved);
    }
}
