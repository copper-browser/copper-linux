//! Where a row dragged down the column lands, folders and all (Mac:
//! `Fork/SavedDrop.swift`). No widgets, so the geometry is unit-tested.
//!
//! The column is, top to bottom: the favourites, the Saved lines (folder
//! heads and rows, as [`crate::folders::plan`] lays them out), New Tab, the
//! Today lines. The place is read off the lines as they are on screen: a
//! shut folder's rows are not drawn, so they are never counted. Counting
//! rows put a tab in whichever folder came next, and a tab could not be
//! dragged out of a folder. A row's room opens between two lines, inside a
//! folder (stepped in like its rows), onto a folder's line (in it, first),
//! or under a block's last line at the top level, which is how a tab leaves
//! a folder that ends the block. Over its own place nothing opens
//! ([`stays`]). Above the rows are the favourites, and a row let go there
//! becomes a pin.

use copper_core::TabId;

/// What a drawn line is.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Tab(TabId),
    Folder { id: String, collapsed: bool },
}

/// One drawn line of a block, in the window's coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub kind: Kind,
    pub top: f32,
    pub bottom: f32,
    /// How many folders in it sits (0: the top level).
    pub depth: usize,
    /// A row's folder; a folder's own parent folder.
    pub group: Option<String>,
}

impl Line {
    fn middle(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

/// The column as a drag sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// Above this are the favourites.
    pub pins: f32,
    /// Where Saved starts (under the space's name).
    pub saved_top: f32,
    pub saved: Vec<Line>,
    /// The New Tab row (top, bottom): above its middle is Saved.
    pub new_tab: (f32, f32),
    pub today: Vec<Line>,
    /// The room between two lines.
    pub gap: f32,
}

/// Where a dropped row goes, relative to what is drawn.
#[derive(Debug, Clone, PartialEq)]
pub enum Anchor {
    Before(TabId),
    After(TabId),
    /// Above a folder, and so above every tab in it.
    BeforeFolder(String),
    /// Onto a folder's own line: in it, first.
    Into(String),
    /// The block's end, at the top level.
    End,
}

/// A place in a block.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    /// In Saved (else Today).
    pub saved: bool,
    pub anchor: Anchor,
    /// The folder it lands in; `None` at the top level.
    pub group: Option<String>,
    /// How far in the room opens, in folder steps.
    pub depth: usize,
    /// The middle of the room it opens.
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Up over the favourites: it becomes a pin, at their end.
    Pins,
    Spot(Spot),
}

/// Where a row let go with the pointer at `y` goes.
pub fn target(layout: &Layout, y: f32) -> Target {
    if y < layout.pins {
        return Target::Pins;
    }
    let (top, bottom) = layout.new_tab;
    let half = layout.gap / 2.0;
    Target::Spot(if y < (top + bottom) / 2.0 {
        spot(&layout.saved, y, true, layout.saved_top + half, half)
    } else {
        spot(&layout.today, y, false, bottom + half, half)
    })
}

/// The place in one block nearest the pointer; `empty` is where the room
/// opens in a block with no lines.
fn spot(lines: &[Line], y: f32, saved: bool, empty: f32, half: f32) -> Spot {
    let end = |y| Spot {
        saved,
        anchor: Anchor::End,
        group: None,
        depth: 0,
        y,
    };
    let (Some(first), Some(last)) = (lines.first(), lines.last()) else {
        return end(empty);
    };
    if y < first.top {
        return before(first, saved, half);
    }
    // Below the last line: the block's end at the top level, whatever
    // folder the last line is in.
    if y >= last.bottom + half {
        return end(last.bottom + half);
    }
    // The line under the pointer, with the gap below it counted as its own.
    let i = lines
        .iter()
        .position(|l| y < l.bottom + half)
        .unwrap_or(lines.len() - 1);
    let line = &lines[i];
    let next = lines.get(i + 1);
    match &line.kind {
        Kind::Folder { id, collapsed } => {
            let rel = (y - line.top) / (line.bottom - line.top).max(1.0);
            // The top quarter is the gap above the folder. The bottom
            // quarter of an open one is the gap above its first row, the
            // same place as onto the folder, shown where it will be.
            if rel < 0.25 {
                return before(line, saved, half);
            }
            if let Some(n) = next
                && rel > 0.75
                && !collapsed
                && n.depth > line.depth
            {
                return before(n, saved, half);
            }
            Spot {
                saved,
                anchor: Anchor::Into(id.clone()),
                group: Some(id.clone()),
                depth: line.depth + 1,
                y: line.bottom + half,
            }
        }
        Kind::Tab(id) => {
            if y < line.middle() {
                return before(line, saved, half);
            }
            // The lower half: after this row, in its folder. Above a
            // sibling that is the same place as before it.
            if let Some(n) = next
                && matches!(n.kind, Kind::Tab(_))
                && n.group == line.group
                && n.depth == line.depth
            {
                return before(n, saved, half);
            }
            Spot {
                saved,
                anchor: Anchor::After(id.clone()),
                group: line.group.clone(),
                depth: line.depth,
                y: line.bottom + half,
            }
        }
    }
}

fn before(line: &Line, saved: bool, half: f32) -> Spot {
    let anchor = match &line.kind {
        Kind::Tab(id) => Anchor::Before(id.clone()),
        Kind::Folder { id, .. } => Anchor::BeforeFolder(id.clone()),
    };
    Spot {
        saved,
        anchor,
        group: line.group.clone(),
        depth: line.depth,
        y: line.top - half,
    }
}

/// Where in the space's row (`(id, saved)`, in order) a tab let go at
/// `spot` goes, as `Browser::move_tab` counts: its index once it is there.
/// `in_folder` says whether a tab is in a folder or one inside it.
pub fn index(
    spot: &Spot,
    moving: &TabId,
    row: &[(TabId, bool)],
    in_folder: impl Fn(&TabId, &str) -> bool,
) -> usize {
    let rest: Vec<&(TabId, bool)> = row.iter().filter(|(t, _)| t != moving).collect();
    let end = if spot.saved {
        rest.iter().rposition(|(_, s)| *s).map_or(0, |p| p + 1)
    } else {
        rest.len()
    };
    let at = |id: &TabId| rest.iter().position(|(t, _)| t == id);
    match &spot.anchor {
        Anchor::Before(id) => at(id).unwrap_or(end),
        Anchor::After(id) => at(id).map_or(end, |p| p + 1),
        Anchor::BeforeFolder(g) | Anchor::Into(g) => rest
            .iter()
            .position(|(t, s)| *s == spot.saved && in_folder(t, g))
            .unwrap_or(end),
        Anchor::End => end,
    }
}

/// A row let go at `spot` would stay where it is (same block, same folder,
/// same place), so no room opens for it. `group` is its folder now.
pub fn stays(
    spot: &Spot,
    moving: &TabId,
    group: Option<&str>,
    row: &[(TabId, bool)],
    in_folder: impl Fn(&TabId, &str) -> bool,
) -> bool {
    if matches!(&spot.anchor, Anchor::Before(id) | Anchor::After(id) if id == moving) {
        return true;
    }
    let Some(here) = row.iter().position(|(t, _)| t == moving) else {
        return false;
    };
    row[here].1 == spot.saved
        && spot.group.as_deref() == group
        && index(spot, moving, row, in_folder) == here
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> TabId {
        TabId(n.into())
    }

    fn tab(n: &str, top: f32, depth: usize, group: Option<&str>) -> Line {
        Line {
            kind: Kind::Tab(id(n)),
            top,
            bottom: top + 30.0,
            depth,
            group: group.map(str::to_string),
        }
    }

    fn folder(n: &str, top: f32, collapsed: bool) -> Line {
        Line {
            kind: Kind::Folder {
                id: n.into(),
                collapsed,
            },
            top,
            bottom: top + 30.0,
            depth: 0,
            group: None,
        }
    }

    /// Pins above 80. Saved: a; folder W (open) with c, d, ending Saved.
    /// New Tab 210–240. Today: folder S (shut, holding x and y), then t.
    fn column() -> Layout {
        Layout {
            pins: 80.0,
            saved_top: 80.0,
            saved: vec![
                tab("a", 82.0, 0, None),
                folder("W", 114.0, false),
                tab("c", 146.0, 1, Some("W")),
                tab("d", 178.0, 1, Some("W")),
            ],
            new_tab: (210.0, 240.0),
            today: vec![folder("S", 242.0, true), tab("t", 274.0, 0, None)],
            gap: 2.0,
        }
    }

    /// The space's row: a, c, d saved; x and y (in shut S), t today.
    fn row() -> Vec<(TabId, bool)> {
        [
            ("a", true),
            ("c", true),
            ("d", true),
            ("x", false),
            ("y", false),
            ("t", false),
        ]
        .into_iter()
        .map(|(n, s)| (id(n), s))
        .collect()
    }

    fn in_folder(t: &TabId, g: &str) -> bool {
        match g {
            "W" => t.0 == "c" || t.0 == "d",
            "S" => t.0 == "x" || t.0 == "y",
            _ => false,
        }
    }

    fn spot_at(y: f32) -> Spot {
        spot_at_in(&column(), y)
    }

    fn spot_at_in(c: &Layout, y: f32) -> Spot {
        match target(c, y) {
            Target::Spot(s) => s,
            Target::Pins => panic!("over the pins at {y}"),
        }
    }

    #[test]
    fn a_shut_folders_hidden_rows_are_not_counted() {
        // Over t's upper half, under the shut S: before t, at the top level.
        let s = spot_at(280.0);
        assert_eq!(
            (s.saved, &s.anchor, &s.group),
            (false, &Anchor::Before(id("t")), &None)
        );
        // After x and y, which are not drawn: not into S.
        assert_eq!(index(&s, &id("a"), &row(), in_folder), 4);
        // Onto the shut folder's line: in it, first.
        let s = spot_at(260.0);
        assert_eq!(
            (&s.anchor, s.group.as_deref()),
            (&Anchor::Into("S".into()), Some("S"))
        );
        assert_eq!(index(&s, &id("a"), &row(), in_folder), 2);
        // Its top quarter: above it, at the top level.
        let s = spot_at(244.0);
        assert_eq!(
            (&s.anchor, &s.group),
            (&Anchor::BeforeFolder("S".into()), &None)
        );
    }

    #[test]
    fn under_a_folder_that_ends_saved_is_the_top_level() {
        // Below d, the last row of W and of Saved, down to New Tab's middle.
        for y in [210.0, 224.0] {
            let s = spot_at(y);
            assert_eq!(
                (s.saved, &s.anchor, &s.group, s.depth),
                (true, &Anchor::End, &None, 0)
            );
            assert!(!stays(&s, &id("d"), Some("W"), &row(), in_folder));
            assert_eq!(index(&s, &id("d"), &row(), in_folder), 2);
        }
        // Past it: the top of Today.
        let s = spot_at(226.0);
        assert_eq!(
            (s.saved, &s.anchor),
            (false, &Anchor::BeforeFolder("S".into()))
        );
    }

    #[test]
    fn dragged_out_of_a_folder_above_it() {
        // c over the top quarter of W's line: out, before the folder.
        let s = spot_at(116.0);
        assert_eq!(
            (&s.anchor, &s.group, s.depth),
            (&Anchor::BeforeFolder("W".into()), &None, 0)
        );
        assert!(!stays(&s, &id("c"), Some("W"), &row(), in_folder));
        assert_eq!(index(&s, &id("c"), &row(), in_folder), 1);
        // The lower half of a, the top level's last row before W: still out.
        let s = spot_at(108.0);
        assert_eq!((&s.anchor, &s.group), (&Anchor::After(id("a")), &None));
    }

    #[test]
    fn into_a_folder_stepped_in() {
        // The bottom quarter of the open W: above c, in W.
        let s = spot_at(140.0);
        assert_eq!(
            (&s.anchor, s.group.as_deref(), s.depth),
            (&Anchor::Before(id("c")), Some("W"), 1)
        );
        // a let go there lands in W, first.
        assert!(!stays(&s, &id("a"), None, &row(), in_folder));
        assert_eq!(index(&s, &id("a"), &row(), in_folder), 0);
    }

    #[test]
    fn over_its_own_place_nothing_opens() {
        // d over either half of its own line.
        for y in [182.0, 200.0] {
            assert!(
                stays(&spot_at(y), &id("d"), Some("W"), &row(), in_folder),
                "at {y}"
            );
        }
        // c's lower half is the gap above d: d's own place.
        assert!(stays(
            &spot_at(170.0),
            &id("d"),
            Some("W"),
            &row(),
            in_folder
        ));
        // And c's: it is right above d already.
        assert!(stays(
            &spot_at(170.0),
            &id("c"),
            Some("W"),
            &row(),
            in_folder
        ));
        // a over c's upper half moves into W.
        assert!(!stays(&spot_at(150.0), &id("a"), None, &row(), in_folder));
    }

    #[test]
    fn above_the_rows_are_the_pins() {
        assert_eq!(target(&column(), 79.0), Target::Pins);
        assert_eq!(target(&column(), 10.0), Target::Pins);
        assert!(matches!(
            target(&column(), 81.0),
            Target::Spot(Spot {
                anchor: Anchor::Before(_),
                ..
            })
        ));
    }

    #[test]
    fn an_empty_block_opens_at_its_start() {
        let c = Layout {
            saved: vec![],
            today: vec![],
            ..column()
        };
        let s = spot_at_in(&c, 100.0);
        assert_eq!((s.saved, &s.anchor, s.y), (true, &Anchor::End, 81.0));
        let s = spot_at_in(&c, 400.0);
        assert_eq!((s.saved, &s.anchor, s.y), (false, &Anchor::End, 241.0));
        assert_eq!(index(&s, &id("a"), &row(), in_folder), 5);
    }
}
