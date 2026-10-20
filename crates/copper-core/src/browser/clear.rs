//! Clear, the model half (Mac: `Fork/ClearTabs.swift`, `Browser.closeForClear`
//! / `restoreClearedTabs`): close a set of loose rows of the current space
//! at once, and put them back exactly where they were.
//!
//! Which rows may go is the shell's call (`copper-shell-core`'s `clear`:
//! not pins, Saved rows, groups, canvases or pages an agent is working);
//! this closes the ones it names the ordinary way — each joins the closed
//! stack like a ⌘W — and remembers enough to undo it: the tabs themselves
//! (same ids, titles, icons), their places in the row, the selection, the
//! space and the closed-stack records the closes made.

use super::{Browser, BrowserEvent, ClosedTab, Closing, Tab, TabId};

/// What one Clear took, for [`Browser::undo_clear`].
#[derive(Debug, Clone, PartialEq)]
pub struct Cleared {
    /// The tabs as they were, each with its index in the row, row order.
    pub tabs: Vec<(Tab, usize)>,
    /// The tab selected before (cleared or not).
    pub active: Option<TabId>,
    /// The space it happened in.
    pub space: String,
    /// The closed-stack records the closes added (taken off again on undo,
    /// so ⌘⇧T never brings back a tab Undo already did).
    ghosts: Vec<ClosedTab>,
}

impl Cleared {
    /// How many tabs went.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }
}

impl Browser {
    /// Close `ids` — loose rows of the current space; any other id is
    /// skipped — the way ⌘W closes a Today row. When the selected tab was
    /// one of them, nothing is selected afterwards: the space lands on its
    /// empty state (the launch card), as Arc's Clear does, not on a kept
    /// neighbour. `None` when nothing went.
    pub fn clear_tabs(&mut self, ids: &[TabId]) -> Option<Cleared> {
        let space = self.current.clone();
        let row: Vec<TabId> = self.row_of(&space)?.tabs.clone();
        let mut picked: Vec<(Tab, usize)> = row
            .iter()
            .enumerate()
            .filter(|(_, id)| ids.contains(id))
            .filter_map(|(i, id)| {
                let t = self.tabs.get(id)?;
                (!t.is_pinned()).then(|| (t.clone(), i))
            })
            .collect();
        if picked.is_empty() {
            return None;
        }
        picked.sort_by_key(|(_, i)| *i);
        let active = self.active_id();
        // Unselected first, so no close lands on (and wakes) a neighbour.
        if active
            .as_ref()
            .is_some_and(|a| picked.iter().any(|(t, _)| &t.id == a))
        {
            self.set_active(None);
        }
        let mut ghosts = Vec::new();
        for (tab, _) in &picked {
            let before = self.closed.len();
            let last = self.closed.last().cloned();
            self.close_with(&tab.id, Closing::REMOVE);
            // A restorable close pushes one record (dropping the oldest at
            // the cap, so the length alone can't tell).
            if self.closed.len() > before || self.closed.last() != last.as_ref() {
                ghosts.extend(self.closed.last().cloned());
            }
        }
        Some(Cleared {
            tabs: picked,
            active,
            space,
            ghosts,
        })
    }

    /// Put a Clear back: its space shown again, each tab in its old place
    /// (asleep — its page went — except the one that was selected, which is
    /// selected and wakes), its closed-stack records gone. Returns how many
    /// came back. A tab whose id is somehow open again is left alone; a
    /// space removed meanwhile gives its tabs to the current one.
    pub fn undo_clear(&mut self, cleared: Cleared) -> usize {
        let space = if self.row_of(&cleared.space).is_some() {
            if self.current != cleared.space {
                self.switch_space(&cleared.space);
            }
            cleared.space.clone()
        } else {
            self.current.clone()
        };
        let mut back = 0;
        for (mut tab, index) in cleared.tabs {
            if self.tabs.contains_key(&tab.id) {
                continue;
            }
            tab.space = space.clone();
            tab.sleeping = cleared.active.as_ref() != Some(&tab.id);
            let id = tab.id.clone();
            let Some(row) = self.row_mut(&space) else {
                break;
            };
            row.tabs.insert(index.min(row.tabs.len()), id.clone());
            self.tabs.insert(id.clone(), tab);
            self.changed(BrowserEvent::TabOpened(id));
            back += 1;
        }
        if back > 0 {
            self.changed(BrowserEvent::RowChanged {
                space: Some(space.clone()),
            });
        }
        for ghost in cleared.ghosts.iter().rev() {
            if let Some(i) = self.closed.iter().rposition(|g| g == ghost) {
                self.closed.remove(i);
            }
        }
        if let Some(active) = cleared.active.filter(|a| self.tabs.contains_key(a)) {
            self.select(&active);
        }
        back
    }
}

#[cfg(test)]
mod tests {
    use crate::browser::{Browser, OpenOptions, TabId};

    fn open(b: &mut Browser, url: &str) -> TabId {
        b.open_tab(
            url,
            OpenOptions {
                at_end: true,
                foreground: true,
                ..Default::default()
            },
        )
    }

    fn row(b: &Browser) -> Vec<String> {
        b.row(b.current_space_id())
            .iter()
            .map(|t| t.url.clone())
            .collect()
    }

    #[test]
    fn clear_then_undo_puts_everything_back() {
        let mut b = Browser::new();
        let a = open(&mut b, "https://a.test/");
        let k = open(&mut b, "https://kept.test/");
        let c = open(&mut b, "https://c.test/");
        let d = open(&mut b, "https://d.test/");
        b.update_tab(&k, |t| t.saved = true);
        b.select(&c);
        let before = row(&b);
        let closed_before = b.closed_tabs().len();
        b.take_events();

        let cleared = b.clear_tabs(&[a.clone(), c.clone(), d.clone()]).unwrap();
        assert_eq!(cleared.len(), 3);
        assert_eq!(row(&b), vec!["https://kept.test/"]);
        assert_eq!(b.active_id(), None, "the space lands on its empty state");
        assert_eq!(b.closed_tabs().len(), closed_before + 3);

        assert_eq!(b.undo_clear(cleared), 3);
        assert_eq!(row(&b), before);
        assert_eq!(b.active_id(), Some(c.clone()));
        assert_eq!(b.closed_tabs().len(), closed_before, "no ghosts left");
        assert!(!b.tab(&c).unwrap().sleeping, "the selected one wakes");
        assert!(b.tab(&a).unwrap().sleeping, "the rest come back asleep");
        assert!(b.tab(&d).unwrap().sleeping);
    }

    #[test]
    fn clearing_a_background_tab_keeps_the_selection() {
        let mut b = Browser::new();
        let a = open(&mut b, "https://a.test/");
        let k = open(&mut b, "https://kept.test/");
        b.select(&k);
        let cleared = b.clear_tabs(std::slice::from_ref(&a)).unwrap();
        assert_eq!(b.active_id(), Some(k.clone()));
        b.undo_clear(cleared);
        assert_eq!(b.active_id(), Some(k));
        assert_eq!(row(&b), vec!["https://a.test/", "https://kept.test/"]);
    }

    #[test]
    fn pins_and_other_spaces_are_never_taken() {
        let mut b = Browser::new();
        let p = open(&mut b, "https://pin.test/");
        b.pin(&p, None);
        let home = b.current_space_id().to_string();
        let work = b.add_space("Work");
        b.switch_space(&work);
        let w = open(&mut b, "https://work.test/");
        b.switch_space(&home);
        assert!(b.clear_tabs(&[p.clone(), w.clone()]).is_none());
        assert!(b.tab(&p).is_some() && b.tab(&w).is_some());
    }

    #[test]
    fn undo_goes_back_to_the_space_it_happened_in() {
        let mut b = Browser::new();
        let home = b.current_space_id().to_string();
        let a = open(&mut b, "https://a.test/");
        let cleared = b.clear_tabs(std::slice::from_ref(&a)).unwrap();
        let work = b.add_space("Work");
        b.switch_space(&work);
        b.undo_clear(cleared);
        assert_eq!(b.current_space_id(), home);
        assert_eq!(b.active_id(), Some(a));
    }

    #[test]
    fn reopen_closed_never_doubles_an_undone_tab() {
        let mut b = Browser::new();
        let old = open(&mut b, "https://old.test/");
        b.close_tab(&old, false);
        let a = open(&mut b, "https://a.test/");
        let cleared = b.clear_tabs(std::slice::from_ref(&a)).unwrap();
        b.undo_clear(cleared);
        let back = b.reopen_closed().unwrap();
        assert_eq!(b.tab(&back).unwrap().url, "https://old.test/");
    }
}
