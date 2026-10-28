//! One calendar's file (RFC 5545, what a secret iCal address answers) read
//! into the meetings in a span of time.
//!
//! `icalendar` parses it and `rrule` expands repeating events (RRULE,
//! RDATE, EXDATE) in their own time zones (TZID); what is left here is
//! what makes an event a meeting worth listing, the way the Mac's card
//! decides (`CalendarPeek.meeting`): no all-day events, nothing cancelled,
//! nothing the calendar's owner declined. One instance of a repeating
//! event that was moved or cancelled on its own comes as an event of its
//! own naming the start it replaces (RECURRENCE-ID): the series skips that
//! start and the instance speaks for itself.

use crate::Meeting;
use chrono::TimeZone as _;
use icalendar::{
    Calendar, CalendarComponent, CalendarDateTime, Component, DatePerhapsTime, Event, EventLike,
    EventStatus, PartStat, Tz,
};
use std::collections::HashSet;

/// What one calendar's file gave.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    /// The calendar's own name (`X-WR-CALNAME`).
    pub name: Option<String>,
    /// Its meetings that overlap the span read, in no order, with no colour
    /// yet (the calendar's is the caller's to give).
    pub meetings: Vec<Meeting>,
}

/// Words for an answer that isn't a calendar (a removed calendar's address
/// answers with a web page).
pub const NOT_A_CALENDAR: &str = "the address didn't answer with a calendar";

/// The most instances of one repeating event in a span: more than a day
/// holds of anything that is a meeting.
const MOST: u16 = 100;

/// Read `ics` into the meetings that overlap `from..to` (Unix seconds).
/// `owner` is the attendee whose answer counts ([`crate::feeds::owner`]).
pub fn read(ics: &str, owner: Option<&str>, from: i64, to: i64) -> Result<Read, String> {
    if !ics
        .trim_start_matches('\u{feff}')
        .trim_start()
        .starts_with("BEGIN:VCALENDAR")
    {
        return Err(NOT_A_CALENDAR.into());
    }
    let calendar: Calendar = ics.parse().map_err(|_| NOT_A_CALENDAR.to_string())?;
    let events: Vec<&Event> = calendar
        .components
        .iter()
        .filter_map(|c| match c {
            CalendarComponent::Event(e) => Some(e),
            _ => None,
        })
        .collect();
    let replaced: HashSet<(&str, i64)> = events
        .iter()
        .filter_map(|e| match e.get_recurrence_id()? {
            DatePerhapsTime::DateTime(at) => Some((e.get_uid()?, unix(&at)?)),
            DatePerhapsTime::Date(_) => None,
        })
        .collect();
    let mut meetings = Vec::new();
    for e in events {
        // A date with no time is an all-day event.
        let Some(DatePerhapsTime::DateTime(start)) = e.get_start() else {
            continue;
        };
        let Some(begin) = unix(&start) else {
            tracing::debug!(target: "copper::calendar", "an event in a time zone this build doesn't know; left out");
            continue;
        };
        if !counts(e, owner) {
            continue;
        }
        let length = length(e, begin);
        let series = e.get_recurrence_id().is_none()
            && (e.property_value("RRULE").is_some() || e.multi_properties().contains_key("RDATE"));
        let starts = if series {
            match instances(e, from - length, to) {
                Some(s) => s,
                None => continue,
            }
        } else {
            vec![begin]
        };
        let uid = e.get_uid().unwrap_or_default();
        let title = e.get_summary().map(str::trim).unwrap_or_default();
        for start in starts {
            if start + length <= from || start >= to || (series && replaced.contains(&(uid, start)))
            {
                continue;
            }
            meetings.push(Meeting {
                id: format!("{uid}@{start}"),
                title: if title.is_empty() { "Busy" } else { title }.to_string(),
                start,
                end: start + length,
                color: String::new(),
                join: join_link(
                    e.get_url()
                        .or_else(|| e.property_value("X-GOOGLE-CONFERENCE")),
                    e.get_location(),
                    e.get_description(),
                ),
            });
        }
    }
    Ok(Read {
        name: calendar
            .get_name()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(String::from),
        meetings,
    })
}

/// Whether an event is one to list: not cancelled, and not declined by the
/// calendar's owner.
pub fn counts(e: &Event, owner: Option<&str>) -> bool {
    if e.get_status() == Some(EventStatus::Cancelled) {
        return false;
    }
    let Some(owner) = owner else { return true };
    !e.get_attendees().iter().any(|a| {
        a.part_stat == Some(PartStat::Declined)
            && a.cal_address
                .trim_start_matches("mailto:")
                .trim_start_matches("MAILTO:")
                .eq_ignore_ascii_case(owner)
    })
}

/// How long the event lasts, in seconds: to DTEND, else its DURATION,
/// else no time at all (RFC 5545 §3.6.1).
fn length(e: &Event, begin: i64) -> i64 {
    if let Some(DatePerhapsTime::DateTime(end)) = e.get_end()
        && let Some(end) = unix(&end)
    {
        return (end - begin).max(0);
    }
    e.property_value("DURATION")
        .and_then(|d| iso8601::duration(d).ok())
        .map(|d| std::time::Duration::from(d).as_secs() as i64)
        .unwrap_or(0)
}

/// The starts of a repeating event from `from` through `to`. `None`: the
/// rule doesn't read (the event is left out rather than the calendar).
fn instances(e: &Event, from: i64, to: i64) -> Option<Vec<i64>> {
    let set = match e.get_recurrence() {
        Ok(set) => set,
        Err(err) => {
            tracing::debug!(target: "copper::calendar", "a repeating event whose rule doesn't read ({err}); left out");
            return None;
        }
    };
    let at = |t: i64| Tz::UTC.timestamp_opt(t, 0).single();
    let starts = set.after(at(from)?).before(at(to)?).all(MOST).dates;
    Some(starts.iter().map(|d| d.timestamp()).collect())
}

/// A date and time as Unix seconds: in UTC, in its TZID's zone, or (a
/// floating time) in this computer's.
fn unix(t: &CalendarDateTime) -> Option<i64> {
    match t {
        CalendarDateTime::Floating(naive) => Tz::LOCAL
            .from_local_datetime(naive)
            .earliest()
            .map(|d| d.timestamp()),
        other => other.try_into_utc().map(|d| d.timestamp()),
    }
}

/// Video-call hosts a Join button is for. Any other link in an invite (a
/// doc, an agenda) is not a way into the meeting.
pub const CALL_HOSTS: [&str; 5] = [
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
];

/// The meeting's call link: the event's own URL if it is one, else the
/// first call link in its location, then its notes.
pub fn join_link(url: Option<&str>, location: Option<&str>, notes: Option<&str>) -> Option<String> {
    if let Some(url) = url.map(str::trim).filter(|u| is_call(u)) {
        return Some(url.to_string());
    }
    [location, notes]
        .into_iter()
        .flatten()
        .flat_map(links)
        .find(|l| is_call(l))
        .map(String::from)
}

fn is_call(link: &str) -> bool {
    let Ok(url) = url::Url::parse(link) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            CALL_HOSTS
                .iter()
                .any(|c| host == *c || host.strip_suffix(c).is_some_and(|h| h.ends_with('.')))
        })
}

/// The `https://` links in free text (plain, or the HTML Google writes):
/// each runs to a space, a quote, a bracket or an escape, less the
/// punctuation that ends a sentence.
fn links(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices("https://").map(|(i, _)| {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '\\'))
            .unwrap_or(rest.len());
        rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']'])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-28 15:00:00 UTC, a Wednesday (10:00 in Chicago, CDT).
    const NOW: i64 = 1_793_199_600;
    const HOUR: i64 = 3600;
    const OWNER: Option<&str> = Some("collin@exowatt.com");

    fn calendar(events: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\n\
             X-WR-CALNAME:collin@exowatt.com\r\nX-WR-TIMEZONE:America/Chicago\r\n{}END:VCALENDAR\r\n",
            events.replace('\n', "\r\n")
        )
    }

    fn titles(ics: &str) -> Vec<(String, i64)> {
        let mut got: Vec<(String, i64)> = read(ics, OWNER, NOW, NOW + 12 * HOUR)
            .unwrap()
            .meetings
            .into_iter()
            .map(|m| (m.title, m.start))
            .collect();
        got.sort_by_key(|(_, start)| *start);
        got
    }

    const STANDUP: &str = "BEGIN:VEVENT
DTSTART;TZID=America/Chicago:20261019T093000
DTEND;TZID=America/Chicago:20261019T094500
RRULE:FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR
UID:standup@google.com
SUMMARY:Daily standup
DESCRIPTION:Join with Google Meet: https://meet.google.com/abc-defg-hij\\nOr dial: (US) +1 555-0100
END:VEVENT
";

    #[test]
    fn a_daily_standup_repeats_in_its_own_zone() {
        // 09:30–09:45 in Chicago (CDT, UTC-5) is 14:30–14:45 UTC: on at
        // 14:40, over at 15:00.
        assert!(titles(&calendar(STANDUP)).is_empty());
        let got = read(&calendar(STANDUP), OWNER, NOW - 20 * 60, NOW + 12 * HOUR).unwrap();
        assert_eq!(got.name.as_deref(), Some("collin@exowatt.com"));
        assert_eq!(got.meetings.len(), 1);
        let m = &got.meetings[0];
        assert_eq!(m.start, NOW - 30 * 60);
        assert_eq!(m.end, NOW - 15 * 60);
        assert_eq!(m.id, format!("standup@google.com@{}", NOW - 30 * 60));
        assert_eq!(
            m.join.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        // Read for the next day, it is there again.
        let tomorrow = read(&calendar(STANDUP), OWNER, NOW + 20 * HOUR, NOW + 32 * HOUR).unwrap();
        assert_eq!(tomorrow.meetings[0].start, NOW - 30 * 60 + 24 * HOUR);
    }

    #[test]
    fn time_zones_and_daylight_saving() {
        // Chicago leaves daylight saving on Sunday 1 November 2026: the
        // Monday standup is 15:30 UTC, an hour later than this week's.
        let monday = NOW + 5 * 24 * HOUR - HOUR;
        let got = read(&calendar(STANDUP), OWNER, monday, monday + 12 * HOUR).unwrap();
        assert_eq!(got.meetings[0].start, NOW - 30 * 60 + 5 * 24 * HOUR + HOUR);
        // A one-off in UTC, one in Berlin and a floating one.
        let ics = calendar(
            "BEGIN:VEVENT
DTSTART:20261028T170000Z
DTEND:20261028T173000Z
UID:utc
SUMMARY:UTC
END:VEVENT
BEGIN:VEVENT
DTSTART;TZID=Europe/Berlin:20261028T190000
DURATION:PT45M
UID:berlin
SUMMARY:Berlin
END:VEVENT
",
        );
        let got = read(&ics, OWNER, NOW, NOW + 12 * HOUR).unwrap();
        let berlin = got.meetings.iter().find(|m| m.title == "Berlin").unwrap();
        // 19:00 in Berlin (CET from 25 October, UTC+1) is 18:00 UTC.
        assert_eq!(berlin.start, NOW + 3 * HOUR);
        assert_eq!(berlin.end, NOW + 3 * HOUR + 45 * 60);
        let utc = got.meetings.iter().find(|m| m.title == "UTC").unwrap();
        assert_eq!(utc.start, NOW + 2 * HOUR);
    }

    #[test]
    fn an_exdate_skips_one_day() {
        let skipped = STANDUP.replace("UID:", "EXDATE;TZID=America/Chicago:20261029T093000\nUID:");
        let ics = calendar(&skipped);
        // Thursday is skipped, Friday stays.
        let got = read(&ics, OWNER, NOW + 12 * HOUR, NOW + 48 * HOUR).unwrap();
        let starts: Vec<i64> = got.meetings.iter().map(|m| m.start).collect();
        assert_eq!(starts, vec![NOW - 30 * 60 + 48 * HOUR]);
    }

    #[test]
    fn a_moved_instance_replaces_its_day() {
        // Today's standup moved to 11:00 and retitled; tomorrow's cancelled.
        let ics = calendar(&format!(
            "{STANDUP}BEGIN:VEVENT
DTSTART;TZID=America/Chicago:20261028T110000
DTEND;TZID=America/Chicago:20261028T111500
RECURRENCE-ID;TZID=America/Chicago:20261028T093000
UID:standup@google.com
SUMMARY:Daily standup (moved)
END:VEVENT
BEGIN:VEVENT
DTSTART;TZID=America/Chicago:20261029T093000
DTEND;TZID=America/Chicago:20261029T094500
RECURRENCE-ID;TZID=America/Chicago:20261029T093000
UID:standup@google.com
STATUS:CANCELLED
SUMMARY:Daily standup
END:VEVENT
"
        ));
        assert_eq!(
            titles(&ics),
            vec![("Daily standup (moved)".to_string(), NOW + HOUR)]
        );
        let tomorrow = read(&ics, OWNER, NOW + 12 * HOUR, NOW + 30 * HOUR).unwrap();
        assert!(tomorrow.meetings.is_empty(), "{tomorrow:?}");
    }

    #[test]
    fn cancelled_declined_and_all_day_events_are_left_out() {
        let ics = calendar(
            "BEGIN:VEVENT
DTSTART;VALUE=DATE:20261028
DTEND;VALUE=DATE:20261029
UID:holiday
SUMMARY:Company holiday
END:VEVENT
BEGIN:VEVENT
DTSTART:20261028T160000Z
DTEND:20261028T170000Z
UID:cancelled
STATUS:CANCELLED
SUMMARY:Cancelled
END:VEVENT
BEGIN:VEVENT
DTSTART:20261028T180000Z
DTEND:20261028T190000Z
UID:declined
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Someone;X-NUM-GUESTS=0:mailto:someone@exowatt.com
ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=DECLINED;CN=Collin Rijock;X-NUM-GUESTS=0:mailto:Collin@Exowatt.com
SUMMARY:Declined
END:VEVENT
BEGIN:VEVENT
DTSTART:20261028T200000Z
DTEND:20261028T210000Z
UID:others-declined
ATTENDEE;PARTSTAT=DECLINED:mailto:someone@exowatt.com
ATTENDEE;PARTSTAT=ACCEPTED:mailto:collin@exowatt.com
SUMMARY:Others declined
END:VEVENT
BEGIN:VEVENT
DTSTART:20261028T120000Z
DTEND:20261028T130000Z
UID:over
SUMMARY:Over
END:VEVENT
",
        );
        assert_eq!(
            titles(&ics),
            vec![("Others declined".to_string(), NOW + 5 * HOUR)]
        );
        // With no owner to go by, a declined event is listed.
        let anyone = read(&ics, None, NOW, NOW + 12 * HOUR).unwrap();
        assert_eq!(anyone.meetings.len(), 2);
    }

    #[test]
    fn untitled_events_are_busy() {
        let ics = calendar(
            "BEGIN:VEVENT\nDTSTART:20261028T160000Z\nDTEND:20261028T170000Z\nUID:x\nEND:VEVENT\n",
        );
        assert_eq!(titles(&ics), vec![("Busy".to_string(), NOW + HOUR)]);
    }

    #[test]
    fn a_web_page_is_not_a_calendar() {
        assert_eq!(
            read(
                "<!DOCTYPE html><html>Not Found</html>",
                None,
                NOW,
                NOW + HOUR
            ),
            Err(NOT_A_CALENDAR.to_string())
        );
    }

    #[test]
    fn join_is_a_video_call_link_never_a_doc() {
        let meet = "https://meet.google.com/abc-defg-hij";
        assert_eq!(join_link(Some(meet), None, None).as_deref(), Some(meet));
        assert_eq!(
            join_link(
                Some("https://docs.google.com/document/d/1"),
                Some("Room 4"),
                Some("Agenda https://docs.google.com/document/d/1\nJoin: https://us02web.zoom.us/j/8812?pwd=x."),
            )
            .as_deref(),
            Some("https://us02web.zoom.us/j/8812?pwd=x")
        );
        // The location comes before the notes.
        assert_eq!(
            join_link(
                None,
                Some("https://teams.microsoft.com/l/meetup-join/19%3a"),
                Some("https://meet.google.com/x"),
            )
            .as_deref(),
            Some("https://teams.microsoft.com/l/meetup-join/19%3a")
        );
        // Google's HTML notes.
        assert_eq!(
            join_link(
                None,
                None,
                Some("<a href=\"https://acme.webex.com/meet/pat\">Join</a>"),
            )
            .as_deref(),
            Some("https://acme.webex.com/meet/pat")
        );
        assert_eq!(
            join_link(
                None,
                Some("HQ, 2nd floor"),
                Some("see https://notzoom.us.example.com/j/1 and https://fakezoom.us/j/2")
            ),
            None
        );
        assert_eq!(
            join_link(Some("http://meet.google.com/abc"), None, None),
            None
        );
        assert_eq!(
            join_link(
                None,
                None,
                Some("only https://docs.google.com/document/d/1")
            ),
            None
        );
    }

    #[test]
    fn google_conference_data_is_the_events_own_link() {
        let ics = calendar(
            "BEGIN:VEVENT
DTSTART:20261028T160000Z
DTEND:20261028T170000Z
UID:conf
SUMMARY:Review
DESCRIPTION:Notes: https://docs.google.com/document/d/1
X-GOOGLE-CONFERENCE:https://meet.google.com/xyz-abcd-efg
END:VEVENT
",
        );
        let got = read(&ics, OWNER, NOW, NOW + 12 * HOUR).unwrap();
        assert_eq!(
            got.meetings[0].join.as_deref(),
            Some("https://meet.google.com/xyz-abcd-efg")
        );
    }
}
