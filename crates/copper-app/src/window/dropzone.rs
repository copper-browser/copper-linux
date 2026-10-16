//! Where a tab dragged over the column lands — no widgets, so the math is
//! unit-tested (Mac: `docs/sections.md`, "A tab crosses the seam").
//!
//! The column is, top to bottom: the space header, the Saved rows, the
//! seam (a hairline, or the "drop here to save" slot while Saved is empty),
//! the "New Tab" row, the Today rows. A drop anywhere above the middle of
//! the "New Tab" row lands in Saved; anywhere below it, in Today. Inside a
//! block it lands before the first row whose middle is below the pointer.
//! There is no "clear step out of the block": the marker always says where
//! the tab will go, and that is where it goes.

use copper_core::TabId;

/// One drawn row's vertical extent, in the column body's coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub id: TabId,
    pub top: f64,
    pub bottom: f64,
}

impl Band {
    fn middle(&self) -> f64 {
        (self.top + self.bottom) / 2.0
    }
}

/// The column as a drop sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// The Saved rows, in order.
    pub saved: Vec<Band>,
    /// The Today rows, in order.
    pub today: Vec<Band>,
    /// Where the Saved block starts (under the space header).
    pub saved_top: f64,
    /// The "New Tab" row: (top, bottom).
    pub new_tab: (f64, f64),
}

/// Where a drop at `y` lands, and where to draw the marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Landing {
    /// Into Saved (else Today).
    pub saved: bool,
    /// Before this row of that block; `None` = at the block's end.
    pub before: Option<TabId>,
    /// The marker line's centre, in the body's coordinates.
    pub line: f64,
    /// The block has no row but the dragged one: the marker is the empty
    /// block's own slot (Saved's seam, or the space under "New Tab").
    pub empty: bool,
}

/// Where a drop at `y` lands for the `dragged` tab (its own row, if it is
/// in the column, is skipped: dropping a row on itself leaves it be).
pub fn land(layout: &Layout, y: f64, dragged: &TabId) -> Landing {
    let (new_top, new_bottom) = layout.new_tab;
    let saved = y < (new_top + new_bottom) / 2.0;
    let bands = if saved { &layout.saved } else { &layout.today };
    let others: Vec<&Band> = bands.iter().filter(|b| b.id != *dragged).collect();
    let before = others.iter().find(|b| y < b.middle()).map(|b| b.id.clone());
    // The marker sits in the gap the tab will fill, among the rows as they
    // are drawn now (the dragged one included).
    let at = bands.iter().find(|b| y < b.middle());
    let line = match (at, bands.last()) {
        (Some(b), _) => b.top - 1.0,
        (None, Some(last)) => last.bottom + 1.0,
        (None, None) if saved => (layout.saved_top + new_top) / 2.0,
        (None, None) => new_bottom + 1.0,
    };
    Landing {
        saved,
        before,
        line,
        empty: others.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> TabId {
        TabId(n.into())
    }

    fn band(n: &str, top: f64) -> Band {
        Band {
            id: id(n),
            top,
            bottom: top + 34.0,
        }
    }

    /// The column as drawn: header to 86, Saved s1 s2, the seam, New Tab
    /// at 170–204, Today t1 t2 t3.
    fn column() -> Layout {
        Layout {
            saved: vec![band("s1", 87.0), band("s2", 123.0)],
            today: vec![band("t1", 206.0), band("t2", 242.0), band("t3", 278.0)],
            saved_top: 86.0,
            new_tab: (170.0, 204.0),
        }
    }

    #[test]
    fn a_short_drag_up_out_of_today_saves_at_the_end_of_saved() {
        let c = column();
        // t2 lifted 70 px: over the seam above "New Tab".
        let l = land(&c, 160.0, &id("t2"));
        assert!(l.saved);
        assert_eq!(l.before, None, "at the end of Saved");
        assert_eq!(l.line, 158.0, "under the last Saved row");
        // Over the top half of "New Tab": still Saved.
        assert!(land(&c, 180.0, &id("t2")).saved);
        // The bottom half of "New Tab": the top of Today.
        let l = land(&c, 195.0, &id("t3"));
        assert!(!l.saved);
        assert_eq!(l.before, Some(id("t1")));
        assert_eq!(l.line, 205.0);
    }

    #[test]
    fn inside_saved_it_lands_by_the_rows_middles() {
        let c = column();
        let l = land(&c, 100.0, &id("t1"));
        assert_eq!((l.saved, l.before.clone()), (true, Some(id("s1"))));
        assert_eq!(l.line, 86.0);
        let l = land(&c, 110.0, &id("t1"));
        assert_eq!(l.before, Some(id("s2")));
        assert_eq!(l.line, 122.0);
        // Over the space header: the top of Saved.
        assert_eq!(land(&c, 60.0, &id("t1")).before, Some(id("s1")));
    }

    #[test]
    fn a_row_dropped_on_itself_stays_put() {
        let c = column();
        // s2 over its own lower half: before nothing (still the end).
        let l = land(&c, 150.0, &id("s2"));
        assert_eq!((l.saved, l.before), (true, None));
        // s1 over its own upper half: before s2 — where it already is.
        assert_eq!(land(&c, 95.0, &id("s1")).before, Some(id("s2")));
    }

    #[test]
    fn an_empty_saved_still_has_a_target_above_new_tab() {
        let c = Layout {
            saved: vec![],
            today: vec![band("t1", 140.0), band("t2", 176.0)],
            saved_top: 86.0,
            new_tab: (104.0, 138.0),
        };
        let l = land(&c, 112.0, &id("t1"));
        assert!(l.saved && l.empty);
        assert_eq!(l.before, None);
        assert_eq!(l.line, 95.0, "in the seam over New Tab");
        // The only Saved row being the dragged one is empty too.
        let c = Layout {
            saved: vec![band("s1", 87.0)],
            ..column()
        };
        assert!(land(&c, 100.0, &id("s1")).empty);
    }

    #[test]
    fn below_everything_is_the_end_of_today() {
        let c = column();
        let l = land(&c, 600.0, &id("s1"));
        assert_eq!((l.saved, l.before), (false, None));
        assert_eq!(l.line, 313.0);
        let empty = Layout {
            today: vec![],
            ..column()
        };
        let l = land(&empty, 400.0, &id("s1"));
        assert!(!l.saved && l.empty);
        assert_eq!(l.line, 205.0);
    }
}
