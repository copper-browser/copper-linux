//! The Calendar pin's card (Mac: `Fork/CalendarPeek.swift`): the calendars
//! kept fresh for it, its words, and how it says when a meeting is. The
//! reading itself is [`copper_calendar`]'s.
//!
//! Each app draws the card beside a pin that [`peeks`] once the pointer
//! has rested on it half a second, from what [`open`] says; its Settings
//! add and remove the addresses ([`add`], [`remove`]).

use crate::Platform;
use crate::format::Calendar;
use crate::shell::Shell;
pub use copper_calendar::feeds::owner;
use copper_calendar::{Cache, feeds};
pub use copper_calendar::{Card, Feed, Meeting, peeks};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

/// The card's words.
pub const HEADING: &str = "Coming up";
pub const LOADING: &str = "Loading…";
pub const NOTHING: &str = "Nothing in the next 12 hours.";
pub const NO_FEEDS: &str = "Add your Google Calendar in Settings to see your next meetings here.";
pub const OPEN_SETTINGS: &str = "Open Calendar Settings";
pub const JOIN: &str = "Join";

/// How long the pointer rests on a Calendar pin before the card opens
/// (passing over the pins opens nothing), and how long it may be off both
/// the pin and the card before the card goes (so it can cross the gap).
pub const REST: Duration = Duration::from_millis(500);
pub const LEAVE: Duration = Duration::from_millis(250);

/// What the card is to do, from where the pointer is (Mac: `PinSquare`
/// `settlePeek`): on Calendar pin `pin` or not, on the card or not, with
/// the card up beside `showing` or away. `Some(true)`: open beside `pin`
/// once the pointer has rested [`REST`]; `Some(false)`: go after
/// [`LEAVE`]; `None`: stay as it is.
pub fn settle<T: PartialEq>(pin: Option<&T>, on_card: bool, showing: Option<&T>) -> Option<bool> {
    match (pin, showing) {
        (Some(on), Some(beside)) if on == beside => None,
        (Some(_), _) => Some(true),
        (None, Some(_)) if !on_card => Some(false),
        (None, _) => None,
    }
}

/// Where Google Calendar gives the address.
pub const WHERE: &str =
    "In Google Calendar: Settings › the calendar › “Secret address in iCal format”.";

thread_local! {
    /// The calendars and what each last gave (the UI thread's).
    static CACHE: RefCell<Cache> = RefCell::default();
}

fn data<P: Platform>(shell: &Shell<P>) -> PathBuf {
    shell.store().paths().data.clone()
}

fn now() -> i64 {
    copper_core::date::now_unix() as i64
}

/// What the card shows now. `calendar.json` is read again first (an
/// address added by hand counts at once), and the calendars not read in
/// the last five minutes are read on a thread of their own; once they
/// land, `changed` gets the card as it is then.
pub fn open<P: Platform>(
    shell: &Shell<P>,
    changed: impl FnOnce(&Shell<P>, Card) + 'static,
) -> Card {
    match feeds::load(&data(shell)) {
        Ok(list) => CACHE.with_borrow_mut(|c| c.set_feeds(list)),
        // The calendars as they were: a half-edited file loses nothing.
        Err(e) => tracing::warn!(target: "copper::calendar", "{e}"),
    }
    let at = now();
    let due = CACHE.with_borrow_mut(|c| c.due(at));
    if !due.is_empty() {
        let weak = shell.downgrade();
        crate::main_loop::spawn::<P::Loop>(async move {
            let urls: Vec<String> = due.iter().map(|f| f.url.clone()).collect();
            let got = crate::main_loop::spawn_blocking(move || copper_calendar::fetch(&due, at))
                .await
                .unwrap_or_else(|| {
                    urls.into_iter()
                        .map(|u| (u, Err("reading it failed".to_string())))
                        .collect()
                });
            CACHE.with_borrow_mut(|c| {
                for (url, read) in got {
                    c.landed(&url, read);
                }
            });
            if let Some(shell) = weak.upgrade() {
                changed(&shell, CACHE.with_borrow(|c| c.card(now())));
            }
        });
    }
    CACHE.with_borrow(|c| c.card(at))
}

/// The calendars, each with what Settings calls it.
pub fn list<P: Platform>(shell: &Shell<P>) -> Vec<(Feed, String)> {
    let list = feeds::load(&data(shell)).unwrap_or_else(|e| {
        tracing::warn!(target: "copper::calendar", "{e}");
        Vec::new()
    });
    CACHE.with_borrow(|c| {
        list.into_iter()
            .map(|f| {
                let name = c.name(&f);
                (f, name)
            })
            .collect()
    })
}

/// Add the calendar at the pasted address `text`. The error is in words.
pub fn add<P: Platform>(shell: &Shell<P>, text: &str) -> Result<(), String> {
    let url = feeds::address(text)?;
    let dir = data(shell);
    let mut list = feeds::load(&dir)?;
    if list.iter().any(|f| f.url == url) {
        return Err("That calendar is here already".into());
    }
    list.push(Feed {
        url,
        label: String::new(),
        color: None,
    });
    feeds::save(&dir, &list)?;
    CACHE.with_borrow_mut(|c| c.set_feeds(list));
    Ok(())
}

/// Forget the calendar at `url`.
pub fn remove<P: Platform>(shell: &Shell<P>, url: &str) -> Result<(), String> {
    let dir = data(shell);
    let mut list = feeds::load(&dir)?;
    list.retain(|f| f.url != url);
    feeds::save(&dir, &list)?;
    CACHE.with_borrow_mut(|c| c.set_feeds(list));
    Ok(())
}

/// When a meeting is, the Mac's way: `Now · until 10:30 AM`, `In 12 min ·
/// 11:00 – 11:30 AM` (within the hour), `2:00 – 3:00 PM`.
pub fn when<C: Calendar>(m: &Meeting, now: i64) -> String {
    if m.start <= now {
        let (end, half) = clock::<C>(m.end);
        return format!("Now · until {end} {half}");
    }
    let span = span::<C>(m.start, m.end);
    // Rounded up: a meeting 30 seconds away is in 1 min, not 0.
    let minutes = (m.start - now + 59) / 60;
    if minutes <= 60 {
        format!("In {minutes} min · {span}")
    } else {
        span
    }
}

/// `11:00 – 11:30 AM`; `11:30 AM – 12:30 PM` across noon.
fn span<C: Calendar>(start: i64, end: i64) -> String {
    let ((a, a_half), (b, b_half)) = (clock::<C>(start), clock::<C>(end));
    if a_half == b_half {
        format!("{a} – {b} {b_half}")
    } else {
        format!("{a} {a_half} – {b} {b_half}")
    }
}

/// `10:30` and `AM`, on the local clock.
fn clock<C: Calendar>(unix: i64) -> (String, &'static str) {
    let Some(t) = C::civil(unix as f64) else {
        return (String::new(), "");
    };
    let hour = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    let half = if t.hour < 12 { "AM" } else { "PM" };
    (format!("{hour}:{:02}", t.minute), half)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::Civil;

    /// A clock on UTC: hours and minutes are all `when` reads.
    enum Utc {}

    impl Calendar for Utc {
        fn civil(unix: f64) -> Option<Civil> {
            let s = unix as i64;
            Some(Civil {
                year: 2026,
                month: 10,
                day: 28,
                day_of_year: 301,
                hour: (s.rem_euclid(86400) / 3600) as u32,
                minute: (s.rem_euclid(3600) / 60) as u32,
            })
        }
        fn month_name(_: u32) -> String {
            String::new()
        }
    }

    /// 2026-10-28 10:00 UTC.
    const NOW: i64 = 1_793_181_600;

    fn at(start: i64, end: i64) -> Meeting {
        Meeting {
            id: String::new(),
            title: String::new(),
            start: NOW + start * 60,
            end: NOW + end * 60,
            color: String::new(),
            join: None,
        }
    }

    #[test]
    fn the_card_opens_on_a_rest_and_goes_on_leaving_both() {
        let (a, b) = (&"a", &"b");
        assert_eq!(settle(Some(a), false, None), Some(true));
        assert_eq!(settle(Some(a), false, Some(a)), None);
        // Another Calendar pin: the card moves there after a rest.
        assert_eq!(settle(Some(b), false, Some(a)), Some(true));
        // Off the pin onto the card: it stays; off both: it goes.
        assert_eq!(settle(None, true, Some(a)), None);
        assert_eq!(settle(None, false, Some(a)), Some(false));
        assert_eq!(settle::<&str>(None, false, None), None);
    }

    #[test]
    fn when_a_meeting_is() {
        assert_eq!(when::<Utc>(&at(-5, 30), NOW), "Now · until 10:30 AM");
        assert_eq!(when::<Utc>(&at(0, 30), NOW), "Now · until 10:30 AM");
        assert_eq!(
            when::<Utc>(&at(60, 90), NOW),
            "In 60 min · 11:00 – 11:30 AM"
        );
        assert_eq!(when::<Utc>(&at(90, 150), NOW), "11:30 AM – 12:30 PM");
        assert_eq!(when::<Utc>(&at(240, 300), NOW), "2:00 – 3:00 PM");
        // Half a minute away is a minute away.
        let soon = Meeting {
            start: NOW + 30,
            ..at(0, 30)
        };
        assert_eq!(when::<Utc>(&soon, NOW), "In 1 min · 10:00 – 10:30 AM");
        assert_eq!(when::<Utc>(&at(840, 870), NOW), "12:00 – 12:30 AM");
    }
}
