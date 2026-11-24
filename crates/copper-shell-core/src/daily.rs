//! The morning page (Mac: `Fork/DailyPage.swift`): a page for the start of
//! the day. The first window, launch, return to Copper or wake after the
//! day turns over (3 AM by default, `daily.hour`) opens it as the active
//! tab, once; every other window that day restores as it always did. Off
//! until Settings › General › Morning page has an address (`daily.url`);
//! `daily.shown` is when it last opened. Each app calls [`check`] at
//! launch, when its window comes to the front and when the computer wakes
//! with Copper in front.

use crate::format::Calendar;
use crate::{OpenTab, Platform, Shell};
use copper_core::TabId;
use copper_core::settings::keys;

/// Settings › General › Copper on this computer: the line's words, and
/// what search finds it by (`DailyPageLine`, `SettingsIndex`).
pub const TITLE: &str = "Morning page";
pub const DETAIL: &str = "Opens once a day, in the first window after 3 AM. Empty for off.";
pub const PLACEHOLDER: &str = "https://…";
pub const KEYWORDS: [&str; 8] = [
    "daily page",
    "newspaper",
    "morning",
    "start page",
    "home page",
    "homepage",
    "startup page",
    "first window",
];

/// The day `unix` belongs to, as a day count: its local date, or the date
/// before while it is still before `hour`. Wall-clock, so a DST night moves
/// nothing (`DailyPage.dayStart`). `None` where the calendar can't say.
pub fn day<C: Calendar>(unix: f64, hour: u32) -> Option<i64> {
    let t = C::civil(unix)?;
    Some(days_from_civil(t.year, t.month, t.day) - i64::from(t.hour < hour))
}

/// Whether the page is still to be shown today (`DailyPage.due`).
pub fn due<C: Calendar>(now: f64, shown: Option<f64>, hour: u32) -> bool {
    let Some(today) = day::<C>(now, hour) else {
        return false;
    };
    shown
        .and_then(|s| day::<C>(s, hour))
        .is_none_or(|then| then < today)
}

/// The first of `tabs` (id, address) already showing `url`: the same host
/// and path, whatever the query (`DailyPage.check`).
pub fn showing<'a>(
    tabs: impl IntoIterator<Item = (&'a TabId, &'a str)>,
    url: &url::Url,
) -> Option<TabId> {
    tabs.into_iter()
        .find(|(_, address)| {
            url::Url::parse(address)
                .is_ok_and(|a| a.host_str() == url.host_str() && a.path() == url.path())
        })
        .map(|(id, _)| id.clone())
}

/// Show the morning page if it is due: the tab already showing it (a pin
/// or the space's row) is selected and loads today's edition, else it
/// opens as the active tab. Scripted runs never do. True when it showed.
pub fn check<C: Calendar, P: Platform>(shell: &Shell<P>) -> bool {
    if shell.options().scripted() {
        return false;
    }
    let (url, hour, shown) = {
        let s = shell.settings();
        (s.daily_url(), s.daily_hour(), s.daily_shown())
    };
    let Some(url) = url else { return false };
    let now = copper_core::date::now_unix();
    if !due::<C>(now, shown, hour) {
        return false;
    }
    shell.update_settings(|s| s.set(keys::DAILY_SHOWN, now));
    let open = {
        let b = shell.browser();
        let tabs: Vec<_> = b
            .pins()
            .into_iter()
            .chain(b.row(b.current_space_id()))
            .map(|t| (&t.id, t.url.as_str()))
            .collect();
        showing(tabs, &url)
    };
    match open {
        Some(id) => {
            shell.select(&id);
            shell.navigate(&id, url.as_str()); // today's edition, not the one left open yesterday
        }
        None => {
            shell.open(
                url.as_str(),
                OpenTab {
                    foreground: true,
                    ..Default::default()
                },
            );
        }
    }
    tracing::info!(host = url.host_str().unwrap_or("-"), "daily page");
    true
}

/// Days since 1970-01-01 of a date on the proleptic Gregorian calendar
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    // The morning page's day (DailyPageTests.swift): it turns over at the
    // hour, in wall-clock time, so a launch past midnight still belongs to
    // yesterday and a DST night moves nothing.
    use super::*;
    use crate::format::Civil;

    /// America/New_York: EST, EDT from the second Sunday in March at 2 AM
    /// to the first Sunday in November at 2 AM.
    enum NewYork {}

    fn civil_from_days(days: i64) -> (i32, u32, u32) {
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        ((yoe + era * 400 + i64::from(month <= 2)) as i32, month, day)
    }

    /// The `n`th Sunday of `month` (1970-01-01 was a Thursday).
    fn sunday(year: i32, month: u32, n: i64) -> i64 {
        let first = days_from_civil(year, month, 1);
        first + (3 - first).rem_euclid(7) + 7 * (n - 1)
    }

    impl NewYork {
        fn offset(unix: i64) -> i64 {
            let (year, ..) = civil_from_days(unix.div_euclid(86_400));
            let starts = sunday(year, 3, 2) * 86_400 + 7 * 3600; // 2 AM EST
            let ends = sunday(year, 11, 1) * 86_400 + 6 * 3600; // 2 AM EDT
            if (starts..ends).contains(&unix) {
                -4 * 3600
            } else {
                -5 * 3600
            }
        }
    }

    impl Calendar for NewYork {
        fn civil(unix: f64) -> Option<Civil> {
            let local = unix as i64 + NewYork::offset(unix as i64);
            let (year, month, day) = civil_from_days(local.div_euclid(86_400));
            let secs = local.rem_euclid(86_400);
            Some(Civil {
                year,
                month,
                day,
                day_of_year: (days_from_civil(year, month, day) - days_from_civil(year, 1, 1) + 1)
                    as u32,
                hour: (secs / 3600) as u32,
                minute: (secs % 3600 / 60) as u32,
            })
        }
        fn month_name(_: u32) -> String {
            String::new()
        }
    }

    /// A New York wall-clock time (one that exists), as Unix seconds.
    fn at(y: i32, mo: u32, d: u32, h: i64, mi: i64) -> f64 {
        let wall = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60;
        [5, 4]
            .into_iter()
            .map(|off| wall + off * 3600)
            .find(|&u| u + NewYork::offset(u) == wall)
            .expect("a time that exists in New York") as f64
    }

    fn due_at(now: f64, shown: Option<f64>, hour: u32) -> bool {
        due::<NewYork>(now, shown, hour)
    }

    fn day_of(t: f64, hour: u32) -> i64 {
        day::<NewYork>(t, hour).unwrap()
    }

    #[test]
    fn the_test_calendar_is_new_york() {
        // 2026-11-21 10:00 EST is 15:00 UTC; 2027-03-14 03:01 EDT is 07:01 UTC.
        assert_eq!(at(2026, 11, 21, 10, 0), 1_795_273_200.0);
        assert_eq!(at(2027, 3, 14, 3, 1), 1_805_007_660.0);
        assert_eq!(at(2027, 3, 14, 1, 59), 1_805_007_540.0);
    }

    #[test]
    fn never_shown_is_due() {
        assert!(due_at(at(2026, 11, 21, 10, 0), None, 3));
    }

    #[test]
    fn shown_yesterday_morning_is_due_after_3_am_not_before() {
        let yesterday = Some(at(2026, 11, 20, 10, 0));
        assert!(!due_at(at(2026, 11, 21, 2, 59), yesterday, 3));
        assert!(due_at(at(2026, 11, 21, 3, 0), yesterday, 3));
        assert!(due_at(at(2026, 11, 21, 9, 45), yesterday, 3));
    }

    #[test]
    fn shown_today_a_second_window_or_launch_is_not_due() {
        let shown = Some(at(2026, 11, 21, 9, 50));
        assert!(!due_at(at(2026, 11, 21, 9, 51), shown, 3));
        assert!(!due_at(at(2026, 11, 21, 23, 59), shown, 3));
        // Past midnight still belongs to the same day...
        assert!(!due_at(at(2026, 11, 22, 1, 30), shown, 3));
        // ...until the next 3 AM.
        assert!(due_at(at(2026, 11, 22, 3, 0), shown, 3));
    }

    #[test]
    fn shown_at_1_am_after_a_late_night_the_3_am_after_it_is_a_new_day() {
        let late = at(2026, 11, 21, 1, 0); // belongs to the 20th's day
        assert_eq!(day_of(late, 3), day_of(at(2026, 11, 20, 3, 0), 3));
        assert!(!due_at(at(2026, 11, 21, 2, 0), Some(late), 3));
        assert!(due_at(at(2026, 11, 21, 3, 5), Some(late), 3));
    }

    #[test]
    fn dst_the_day_still_turns_at_3_am_wall_clock() {
        // Spring forward, 2027-03-14 (2 AM → 3 AM): a 23-hour day.
        assert_eq!(
            day_of(at(2027, 3, 14, 10, 0), 3),
            day_of(at(2027, 3, 14, 3, 0), 3)
        );
        assert!(due_at(
            at(2027, 3, 14, 3, 1),
            Some(at(2027, 3, 13, 9, 0)),
            3
        ));
        assert!(!due_at(
            at(2027, 3, 14, 1, 59),
            Some(at(2027, 3, 13, 9, 0)),
            3
        ));
        // Fall back, 2026-11-01 (2 AM → 1 AM): a 25-hour day.
        assert_eq!(
            day_of(at(2026, 11, 1, 10, 0), 3),
            day_of(at(2026, 11, 1, 3, 0), 3)
        );
        assert_eq!(
            day_of(at(2026, 11, 1, 2, 0), 3),
            day_of(at(2026, 10, 31, 3, 0), 3)
        );
        assert!(due_at(
            at(2026, 11, 1, 3, 0),
            Some(at(2026, 10, 31, 22, 0)),
            3
        ));
        // An hour that doesn't exist that night (2 AM, spring forward)
        // lands on the next real one.
        assert_eq!(
            day_of(at(2027, 3, 14, 10, 0), 2),
            day_of(at(2027, 3, 14, 3, 0), 2)
        );
        assert!(day_of(at(2027, 3, 14, 1, 59), 2) < day_of(at(2027, 3, 14, 3, 0), 2));
    }

    #[test]
    fn another_rollover_hour() {
        let shown = Some(at(2026, 11, 20, 12, 0));
        assert!(!due_at(at(2026, 11, 21, 4, 30), shown, 5));
        assert!(due_at(at(2026, 11, 21, 5, 0), shown, 5));
    }

    #[test]
    fn a_new_year_turns_over_like_any_night() {
        let shown = Some(at(2026, 12, 31, 10, 0));
        assert!(!due_at(at(2027, 1, 1, 2, 0), shown, 3));
        assert!(due_at(at(2027, 1, 1, 3, 0), shown, 3));
    }

    #[test]
    fn the_tab_already_showing_it_is_the_same_host_and_path() {
        let (a, b, c) = (TabId("a".into()), TabId("b".into()), TabId("c".into()));
        let url = url::Url::parse("https://paper.example/today").unwrap();
        let tabs = [
            (&a, "https://paper.example/archive"),
            (&b, "https://paper.example/today?edition=1121"),
            (&c, "https://paper.example/today"),
        ];
        assert_eq!(showing(tabs, &url), Some(b));
        assert_eq!(showing([(&a, "https://other.example/today")], &url), None);
        assert_eq!(showing([(&a, "not an address")], &url), None);
    }
}
