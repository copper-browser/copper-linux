//! ⌃Tab in most-recent order (Mac: `Fork/Recent.swift`; Settings › Tabs ›
//! "Ctrl+Tab switches to" › "The tab you were on before", `tabs.switching
//! = recent`).
//!
//! [`Recent`] keeps every tab looked at, newest first. A walk freezes that
//! order while Ctrl is held, so each further Tab goes one deeper instead of
//! bouncing between the last two; letting go of Ctrl ([`Recent::end`])
//! puts the tab the walk landed on at the front. The Mac's hold strip
//! (`RecentUI.swift`) is the platform's to draw, if it wants one.

use copper_core::TabId;

/// The most-recent order and the walk in progress, if any.
#[derive(Debug, Default)]
pub struct Recent {
    /// Every tab looked at, newest first, in every space.
    order: Vec<TabId>,
    walk: Option<Walk>,
}

#[derive(Debug)]
struct Walk {
    /// The row in most-recent order when the walk began.
    snapshot: Vec<TabId>,
    /// Where the walk is now.
    landing: TabId,
}

impl Recent {
    /// A tab came to the front. Ignored during a walk: the order stays
    /// still until [`Recent::end`].
    pub fn touched(&mut self, id: &TabId) {
        if self.walk.is_some() {
            return;
        }
        self.order.retain(|t| t != id);
        self.order.insert(0, id.clone());
    }

    /// A tab closed: out of the order, and out of a walk's snapshot so the
    /// next press skips it.
    pub fn forget(&mut self, id: &TabId) {
        self.order.retain(|t| t != id);
        if let Some(w) = &mut self.walk {
            w.snapshot.retain(|t| t != id);
        }
    }

    /// A walk is in progress (Ctrl still held).
    pub fn walking(&self) -> bool {
        self.walk.is_some()
    }

    /// One ⌃Tab (`forward`) or ⌃⇧Tab: the tab to select. `row` is the tabs
    /// the walk may land on (the space's, blank ones left out), `active`
    /// the one showing. The first press begins a walk from `active`; a
    /// fresh session walks in row order behind it.
    pub fn step(&mut self, row: &[TabId], active: Option<&TabId>, forward: bool) -> Option<TabId> {
        if self.walk.is_none() {
            let active = active.filter(|a| row.contains(a))?;
            if row.len() < 2 {
                return None;
            }
            let mut snapshot = vec![active.clone()];
            snapshot.extend(
                self.order
                    .iter()
                    .filter(|t| *t != active && row.contains(t))
                    .cloned(),
            );
            let unseen: Vec<TabId> = row
                .iter()
                .filter(|t| !snapshot.contains(t))
                .cloned()
                .collect();
            snapshot.extend(unseen);
            self.walk = Some(Walk {
                snapshot,
                landing: active.clone(),
            });
        }
        let walk = self.walk.as_mut()?;
        walk.snapshot.retain(|t| row.contains(t));
        let n = walk.snapshot.len();
        if n < 2 {
            return None;
        }
        let here = walk
            .snapshot
            .iter()
            .position(|t| *t == walk.landing)
            .or_else(|| active.and_then(|a| walk.snapshot.iter().position(|t| t == a)))?;
        let next = if forward {
            (here + 1) % n
        } else {
            (here + n - 1) % n
        };
        walk.landing = walk.snapshot[next].clone();
        Some(walk.landing.clone())
    }

    /// Ctrl let go: the walk ends, and the tab showing (`active`) goes to the
    /// front, the frozen order behind it.
    pub fn end(&mut self, active: Option<&TabId>) {
        if self.walk.take().is_none() {
            return;
        }
        if let Some(a) = active {
            self.touched(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(n: usize) -> Vec<TabId> {
        (0..n).map(|_| TabId::new()).collect()
    }

    /// Look at `ids` in order (the last is showing).
    fn looked(r: &mut Recent, ids: &[&TabId]) {
        for id in ids {
            r.touched(id);
        }
    }

    #[test]
    fn a_held_walk_goes_deeper_without_reordering() {
        let t = tabs(4);
        let (a, b, c, d) = (&t[0], &t[1], &t[2], &t[3]);
        let mut r = Recent::default();
        looked(&mut r, &[a, b, c, d]);
        // Held: d → c → b → a → d, each press one deeper.
        let mut active = d.clone();
        for want in [c, b, a, d] {
            let got = r.step(&t, Some(&active), true).unwrap();
            assert_eq!(&got, want);
            r.touched(&got); // the selection the shell makes; frozen
            active = got;
        }
        assert!(r.walking());
        // Back the other way.
        assert_eq!(r.step(&t, Some(&active), false).as_ref(), Some(a));
        r.end(Some(a));
        assert!(!r.walking());
        // a went to the front, the rest stayed: a, d, c, b.
        assert_eq!(r.step(&t, Some(a), true).as_ref(), Some(d));
        assert_eq!(r.step(&t, Some(d), true).as_ref(), Some(c));
        assert_eq!(r.step(&t, Some(c), true).as_ref(), Some(b));
    }

    #[test]
    fn tap_and_release_flips_between_the_last_two() {
        let t = tabs(3);
        let (a, b, c) = (&t[0], &t[1], &t[2]);
        let mut r = Recent::default();
        looked(&mut r, &[a, b, c]);
        assert_eq!(r.step(&t, Some(c), true).as_ref(), Some(b));
        r.end(Some(b));
        assert_eq!(r.step(&t, Some(b), true).as_ref(), Some(c));
        r.end(Some(c));
        assert_eq!(r.step(&t, Some(c), true).as_ref(), Some(b));
    }

    #[test]
    fn unseen_tabs_follow_in_row_order_and_closed_ones_are_skipped() {
        let t = tabs(4);
        let (a, b, c, d) = (&t[0], &t[1], &t[2], &t[3]);
        let mut r = Recent::default();
        looked(&mut r, &[c]);
        assert_eq!(r.step(&t, Some(c), true).as_ref(), Some(a));
        r.forget(b);
        let row = [a.clone(), c.clone(), d.clone()];
        assert_eq!(r.step(&row, Some(a), true).as_ref(), Some(d));
        r.end(Some(d));
        // Tabs of another space are not walked into.
        assert_eq!(r.step(std::slice::from_ref(d), Some(d), true), None);
        assert!(!r.walking());
    }

    #[test]
    fn end_without_a_walk_changes_nothing() {
        let t = tabs(2);
        let mut r = Recent::default();
        looked(&mut r, &[&t[0], &t[1]]);
        r.end(Some(&t[0]));
        assert_eq!(r.step(&t, Some(&t[1]), true).as_ref(), Some(&t[0]));
    }
}
