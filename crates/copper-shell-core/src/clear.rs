//! Clear (Mac: `Fork/ClearTabs.swift`): Arc's way of tidying a space — the
//! loose, unorganised pages of the current space go at once, and for six
//! seconds "Cleared N tabs · Undo" puts them all back.
//!
//! Pins, Saved rows, grouped tabs, canvases, blank tabs, pages an agent is
//! working on and tabs another window shows are left alone ([`clearable`];
//! Copper has one window, so the last never applies here). The column's
//! "↓ Clear", the palette's Clear Tabs and Ctrl+Shift+K all use the same
//! predicate, so a visible Clear always does something.
//!
//! The model half (closing, the places to go back to) is
//! [`copper_core::browser::Cleared`]; this keeps the one pending Undo, its
//! deadline and the tabs' icons. A second Clear while an Undo is pending
//! first puts the earlier one back (Mac: a new action never silently loses
//! the first undo). Undone tabs come back asleep, except the one that was
//! selected — their pages went with the close.

use crate::{Platform, Shell};
use copper_core::TabId;
use copper_core::browser::Cleared;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long Undo is offered (`ClearTabs.clear`'s six seconds).
pub const UNDO_FOR: Duration = Duration::from_secs(6);

/// What decides whether a row may be cleared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    pub pinned: bool,
    /// No address yet (a new tab).
    pub blank: bool,
    /// A Saved row.
    pub saved: bool,
    /// In a tab group.
    pub grouped: bool,
    /// Showing a canvas (`copper://canvas…`).
    pub canvas: bool,
    /// An agent is working on it.
    pub agent: bool,
    /// Another window shows it.
    pub elsewhere: bool,
}

/// Mac `ClearTabs.clearable`: only loose, unorganised pages nobody else is
/// using.
pub fn clearable(f: &Facts) -> bool {
    !(f.pinned || f.blank || f.saved || f.grouped || f.canvas || f.agent || f.elsewhere)
}

/// The Undo pill's words: "Cleared 3 tabs" (`ClearTabsToast`).
pub fn cleared_words(count: usize) -> String {
    format!("Cleared {count} tab{}", if count == 1 { "" } else { "s" })
}

/// A blank tab: nothing loaded yet (Mac `Tab.isBlank`).
pub fn is_blank(url: &str) -> bool {
    let u = url.trim();
    u.is_empty() || u.eq_ignore_ascii_case("about:blank")
}

/// The one Undo on offer.
struct Pending<I> {
    cleared: Cleared,
    icons: HashMap<TabId, I>,
    until: Instant,
}

thread_local! {
    /// The `Pending<P::Icon>` (one platform per process).
    static PENDING: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

fn take_pending<I: 'static>() -> Option<Pending<I>> {
    PENDING
        .with(|p| p.borrow_mut().take())
        .and_then(|b| b.downcast::<Pending<I>>().ok())
        .map(|b| *b)
}

fn with_pending<I: 'static, R>(f: impl FnOnce(Option<&Pending<I>>) -> R) -> R {
    PENDING.with(|p| {
        let slot = p.borrow();
        f(slot.as_ref().and_then(|b| b.downcast_ref::<Pending<I>>()))
    })
}

impl<P: Platform> Shell<P> {
    /// The current space's rows Clear would close, in row order.
    pub fn clearable_tabs(&self) -> Vec<TabId> {
        let agents = crate::agents::get::<P>();
        let b = self.browser();
        b.row(b.current_space_id())
            .into_iter()
            .filter(|t| {
                clearable(&Facts {
                    pinned: t.is_pinned(),
                    blank: is_blank(&t.url),
                    saved: t.saved,
                    grouped: t.group.is_some(),
                    canvas: crate::canvas::id_from_url(&t.url).is_some(),
                    agent: agents.as_ref().is_some_and(|a| a.works_on(&t.id)),
                    elsewhere: false,
                })
            })
            .map(|t| t.id.clone())
            .collect()
    }

    /// Clear the current space (an Undo still on offer is taken first).
    /// Returns how many tabs went; the caller shows "Cleared N tabs · Undo"
    /// for [`UNDO_FOR`] when it is more than none.
    pub fn clear_tabs(&self) -> usize {
        if self.clear_undo_count().is_some() {
            self.undo_clear();
        }
        let ids = self.clearable_tabs();
        if ids.is_empty() {
            return 0;
        }
        let icons: HashMap<TabId, P::Icon> = {
            let f = self.favicons().borrow();
            ids.iter()
                .filter_map(|id| Some((id.clone(), f.get(id)?.clone())))
                .collect()
        };
        let Some(cleared) = self.with_browser(|b| b.clear_tabs(&ids)) else {
            return 0;
        };
        let count = cleared.len();
        tracing::info!(count, "cleared tabs");
        PENDING.with(|p| {
            *p.borrow_mut() = Some(Box::new(Pending {
                cleared,
                icons,
                until: Instant::now() + UNDO_FOR,
            }))
        });
        count
    }

    /// How many tabs Undo would bring back, while it is on offer.
    pub fn clear_undo_count(&self) -> Option<usize> {
        let live =
            with_pending::<P::Icon, _>(|p| p.map(|p| (p.until > Instant::now(), p.cleared.len())));
        match live {
            Some((true, n)) => Some(n),
            Some((false, _)) => {
                // Expired: let it go.
                take_pending::<P::Icon>();
                None
            }
            None => None,
        }
    }

    /// Undo the last Clear, while on offer: its tabs back in their places,
    /// the selected one selected again. Returns how many came back.
    pub fn undo_clear(&self) -> usize {
        let Some(p) = take_pending::<P::Icon>() else {
            return 0;
        };
        if p.until <= Instant::now() {
            return 0;
        }
        self.favicons().borrow_mut().extend(p.icons);
        let back = self.with_browser(|b| b.undo_clear(p.cleared));
        tracing::info!(back, "clear undone");
        back
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loose_unorganised_pages_clear() {
        assert!(clearable(&Facts::default()));
        let each = [
            Facts {
                pinned: true,
                ..Facts::default()
            },
            Facts {
                blank: true,
                ..Facts::default()
            },
            Facts {
                saved: true,
                ..Facts::default()
            },
            Facts {
                grouped: true,
                ..Facts::default()
            },
            Facts {
                canvas: true,
                ..Facts::default()
            },
            Facts {
                agent: true,
                ..Facts::default()
            },
            Facts {
                elsewhere: true,
                ..Facts::default()
            },
        ];
        for f in each {
            assert!(!clearable(&f), "{f:?} must stay");
        }
    }

    #[test]
    fn words_and_blanks() {
        assert_eq!(cleared_words(1), "Cleared 1 tab");
        assert_eq!(cleared_words(4), "Cleared 4 tabs");
        assert!(is_blank(""));
        assert!(is_blank("about:blank"));
        assert!(!is_blank("https://a.test/"));
    }

    #[test]
    fn canvases_are_recognised() {
        assert!(crate::canvas::id_from_url("copper://canvas/abc").is_some());
        assert!(crate::canvas::id_from_url("copper://canvas").is_some());
        assert!(crate::canvas::id_from_url("https://canvas.test/").is_none());
    }
}
