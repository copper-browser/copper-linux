//! Calendar peek for Copper on Linux and Windows (Mac:
//! `Fork/CalendarPeek.swift`).
//!
//! Rest the pointer on a Google Calendar pin and a card beside it lists the
//! next meetings, with a Join button for a video call. The Mac reads them
//! with EventKit; here they come from each calendar's secret iCal address
//! (Google Calendar › Settings › the calendar › "Secret address in iCal
//! format"), which Copper reads over HTTPS itself.
//!
//! * [`feeds`] — `calendar.json` in Copper's data folder (0600, atomic):
//!   the addresses. Copper Cloud never carries it.
//! * [`ics`] — one calendar's file read into meetings.
//! * [`Cache`] — what each address last gave, read again at most every
//!   five minutes, and what the card shows ([`Card`]).
//! * [`fetch`] — the addresses read (blocking: a thread of its own).
//!
//! The shell's `calendar` lane keeps the one [`Cache`]; each app draws the
//! card. Nothing here logs or prints an address: it is a password to the
//! calendar.

pub mod feeds;
pub mod ics;

pub use feeds::Feed;
pub use ics::Read;

use std::collections::HashMap;
use std::time::Duration;

/// How far ahead the card looks (seconds), and how many meetings it lists.
pub const AHEAD: i64 = 12 * 60 * 60;
pub const LIMIT: usize = 4;
/// A calendar is read again once what it gave is this old (seconds).
pub const FRESH: i64 = 5 * 60;
/// How far past a reading its meetings are kept: a card drawn from a
/// reading up to 12 hours old (the network down since) still has its whole
/// 12 hours.
const HORIZON: i64 = 2 * AHEAD;

/// One meeting on the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meeting {
    /// Its event and start: each instance of a repeating event is its own.
    pub id: String,
    pub title: String,
    /// Unix seconds.
    pub start: i64,
    pub end: i64,
    /// Its calendar's dot, `#rrggbb`.
    pub color: String,
    /// The video call to join (Meet, Zoom, Teams, Webex), if it has one.
    pub join: Option<String>,
}

/// What the card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Card {
    /// No calendar yet: the card says where to add one.
    NoFeeds,
    /// The first reading is on its way.
    Loading,
    /// The next meetings (none: "Nothing in the next 12 hours."), and why
    /// a calendar's last reading failed, in one line.
    Meetings {
        list: Vec<Meeting>,
        error: Option<String>,
    },
}

/// Pins that peek: Google Calendar's.
pub fn peeks(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.host_str() == Some("calendar.google.com"))
}

/// The ones not over yet that start within [`AHEAD`], soonest first, at
/// most [`LIMIT`].
pub fn next(mut meetings: Vec<Meeting>, now: i64) -> Vec<Meeting> {
    meetings.retain(|m| m.end > now && m.start < now + AHEAD);
    meetings.sort_by(|a, b| (a.start, &a.title).cmp(&(b.start, &b.title)));
    meetings.truncate(LIMIT);
    meetings
}

#[derive(Default)]
struct Kept {
    /// When it was last read (Unix seconds).
    asked: Option<i64>,
    /// Being read now.
    busy: bool,
    /// The last good reading.
    read: Option<Read>,
    /// Why the last reading failed.
    error: Option<String>,
}

/// The calendars, and what each last gave.
#[derive(Default)]
pub struct Cache {
    feeds: Vec<Feed>,
    kept: HashMap<String, Kept>,
}

impl Cache {
    /// The calendars as `calendar.json` has them now. What a removed one
    /// gave is forgotten.
    pub fn set_feeds(&mut self, feeds: Vec<Feed>) {
        self.kept
            .retain(|url, _| feeds.iter().any(|f| &f.url == url));
        self.feeds = feeds;
    }

    pub fn feeds(&self) -> &[Feed] {
        &self.feeds
    }

    /// The calendars to read now: not read in the last [`FRESH`] and not
    /// being read. They count as being read from here.
    pub fn due(&mut self, now: i64) -> Vec<Feed> {
        let mut due = Vec::new();
        for f in &self.feeds {
            let k = self.kept.entry(f.url.clone()).or_default();
            if !k.busy && k.asked.is_none_or(|at| now - at >= FRESH) {
                k.asked = Some(now);
                k.busy = true;
                due.push(f.clone());
            }
        }
        due
    }

    /// What reading `url` gave. A failure keeps the last good reading.
    pub fn landed(&mut self, url: &str, got: Result<Read, String>) {
        let Some(k) = self.kept.get_mut(url) else {
            return;
        };
        k.busy = false;
        match got {
            Ok(read) => {
                k.read = Some(read);
                k.error = None;
            }
            Err(e) => k.error = Some(e),
        }
    }

    /// What the card shows at `now`.
    pub fn card(&self, now: i64) -> Card {
        if self.feeds.is_empty() {
            return Card::NoFeeds;
        }
        let kept = |f: &Feed| self.kept.get(&f.url);
        if self
            .feeds
            .iter()
            .all(|f| kept(f).is_none_or(|k| k.read.is_none() && k.error.is_none()))
        {
            return Card::Loading;
        }
        let mut all = Vec::new();
        for (i, f) in self.feeds.iter().enumerate() {
            let Some(read) = kept(f).and_then(|k| k.read.as_ref()) else {
                continue;
            };
            let color = f
                .color
                .clone()
                .unwrap_or_else(|| feeds::COLORS[i % feeds::COLORS.len()].to_string());
            all.extend(read.meetings.iter().map(|m| Meeting {
                color: color.clone(),
                ..m.clone()
            }));
        }
        let error = self.feeds.iter().find_map(|f| {
            let e = kept(f)?.error.as_ref()?;
            Some(format!("Couldn't read {}: {e}", self.name(f)))
        });
        Card::Meetings {
            list: next(all, now),
            error,
        }
    }

    /// What Settings and an error call a calendar: its label, else the
    /// name it gave, else its owner, else "Calendar".
    pub fn name(&self, feed: &Feed) -> String {
        let label = feed.label.trim();
        if !label.is_empty() {
            return label.to_string();
        }
        self.kept
            .get(&feed.url)
            .and_then(|k| k.read.as_ref()?.name.clone())
            .or_else(|| feeds::owner(&feed.url))
            .unwrap_or_else(|| "Calendar".to_string())
    }
}

/// Read `feeds` for their meetings from `now` on, all at once, with an
/// answer for each. Blocking: run it on a thread of its own.
pub fn fetch(feeds: &[Feed], now: i64) -> Vec<(String, Result<Read, String>)> {
    let failed = |why: String| {
        feeds
            .iter()
            .map(|f| (f.url.clone(), Err(why.clone())))
            .collect()
    };
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return failed(e.to_string()),
    };
    let client = match reqwest::Client::builder()
        .user_agent(concat!("Copper/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => return failed(e.to_string()),
    };
    rt.block_on(async move {
        let asks: Vec<_> = feeds
            .iter()
            .map(|f| {
                let (client, url) = (client.clone(), f.url.clone());
                tokio::spawn(async move {
                    let text = get(&client, &url).await?;
                    let owner = feeds::owner(&url);
                    ics::read(&text, owner.as_deref(), now, now + HORIZON)
                })
            })
            .collect();
        let mut out = Vec::new();
        for (f, ask) in feeds.iter().zip(asks) {
            let got = ask
                .await
                .unwrap_or_else(|_| Err("reading it failed".to_string()));
            out.push((f.url.clone(), got));
        }
        out
    })
}

async fn get(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let response = client.get(url).send().await.map_err(|e| words(&e))?;
    if !response.status().is_success() {
        return Err(format!(
            "the address answered {}",
            response.status().as_u16()
        ));
    }
    response.text().await.map_err(|e| words(&e))
}

/// A failure in a few words, never with the address (reqwest's own text
/// names the URL, and the address is secret).
fn words(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "it took too long to answer"
    } else if e.is_connect() {
        "no connection"
    } else {
        "the answer broke off"
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn meeting(id: &str, start: i64, end: i64) -> Meeting {
        Meeting {
            id: id.into(),
            title: id.into(),
            start: NOW + start * 60,
            end: NOW + end * 60,
            color: String::new(),
            join: None,
        }
    }

    fn feed(url: &str) -> Feed {
        Feed {
            url: url.into(),
            label: String::new(),
            color: None,
        }
    }

    #[test]
    fn only_google_calendar_pins_peek() {
        assert!(peeks("https://calendar.google.com/calendar/u/1/r"));
        assert!(peeks(
            "https://CALENDAR.google.com/calendar/r?authuser=a@b.com"
        ));
        assert!(!peeks("https://mail.google.com/mail/u/0/"));
        assert!(!peeks(""));
    }

    #[test]
    fn the_card_lists_what_isnt_over_soonest_first_four_at_most() {
        let list = vec![
            meeting("later", 90, 120),
            meeting("over", -60, -1),
            meeting("now", -10, 20),
            meeting("soon", 15, 45),
            meeting("tonight", 300, 330),
            meeting("evening", 200, 230),
            meeting("ends now", -30, 0),
            meeting("tomorrow", 13 * 60, 14 * 60),
        ];
        let ids: Vec<String> = next(list, NOW).into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["now", "soon", "later", "evening"]);
    }

    #[test]
    fn a_calendar_is_read_at_most_every_five_minutes() {
        let mut cache = Cache::default();
        assert_eq!(cache.card(NOW), Card::NoFeeds);
        cache.set_feeds(vec![feed("https://a/basic.ics")]);
        assert_eq!(cache.due(NOW).len(), 1);
        assert_eq!(cache.card(NOW), Card::Loading);
        // Being read: not asked for again.
        assert!(cache.due(NOW + 1).is_empty());
        cache.landed(
            "https://a/basic.ics",
            Ok(Read {
                name: Some("Exowatt".into()),
                meetings: vec![meeting("standup", 10, 25)],
            }),
        );
        assert!(cache.due(NOW + FRESH - 1).is_empty());
        let Card::Meetings { list, error } = cache.card(NOW) else {
            panic!("meetings");
        };
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].color, feeds::COLORS[0]);
        assert_eq!(error, None);
        assert_eq!(cache.due(NOW + FRESH).len(), 1);
    }

    #[test]
    fn a_failed_reading_keeps_the_last_good_one() {
        let mut cache = Cache::default();
        let mut work = feed("https://a/basic.ics");
        work.color = Some("#123456".into());
        cache.set_feeds(vec![work, feed("https://b/basic.ics")]);
        cache.due(NOW);
        cache.landed(
            "https://a/basic.ics",
            Ok(Read {
                name: Some("Exowatt".into()),
                meetings: vec![meeting("standup", 10, 25)],
            }),
        );
        cache.landed("https://b/basic.ics", Err("no connection".into()));
        let Card::Meetings { list, error } = cache.card(NOW) else {
            panic!("meetings");
        };
        assert_eq!(list[0].color, "#123456");
        assert_eq!(
            error.as_deref(),
            Some("Couldn't read Calendar: no connection")
        );
        cache.due(NOW + FRESH);
        cache.landed(
            "https://a/basic.ics",
            Err("the address answered 404".into()),
        );
        let Card::Meetings { list, error } = cache.card(NOW + FRESH) else {
            panic!("meetings");
        };
        assert_eq!(list.len(), 1, "still the last good reading");
        assert_eq!(
            error.as_deref(),
            Some("Couldn't read Exowatt: the address answered 404")
        );
        // Removing a calendar forgets it.
        cache.set_feeds(vec![feed("https://b/basic.ics")]);
        assert_eq!(
            cache.card(NOW),
            Card::Meetings {
                list: Vec::new(),
                error: Some("Couldn't read Calendar: no connection".into())
            }
        );
    }

    #[test]
    fn names() {
        let mut cache = Cache::default();
        let mut labelled = feed("https://a/basic.ics");
        labelled.label = " Exowatt ".into();
        assert_eq!(cache.name(&labelled), "Exowatt");
        let own = feed(
            "https://calendar.google.com/calendar/ical/collin%40exowatt.com/private-x/basic.ics",
        );
        assert_eq!(cache.name(&own), "collin@exowatt.com");
        cache.set_feeds(vec![own.clone()]);
        cache.due(NOW);
        cache.landed(
            &own.url,
            Ok(Read {
                name: Some("Work".into()),
                meetings: Vec::new(),
            }),
        );
        assert_eq!(cache.name(&own), "Work");
        assert_eq!(cache.name(&feed("http://127.0.0.1/x.ics")), "Calendar");
    }

    #[test]
    fn fetch_reads_a_served_calendar_and_words_a_refusal() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let mut s = stream.unwrap();
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap();
                let ask = String::from_utf8_lossy(&buf[..n]).to_string();
                let (status, body) = if ask.starts_with("GET /cal.ics") {
                    (
                        "200 OK",
                        "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Test\r\nBEGIN:VEVENT\r\nUID:a\r\n\
                         DTSTART:20270115T080000Z\r\nDTEND:20270115T083000Z\r\nSUMMARY:Standup\r\n\
                         END:VEVENT\r\nEND:VCALENDAR\r\n",
                    )
                } else {
                    ("404 Not Found", "gone")
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let at = |path: &str| feed(&format!("http://127.0.0.1:{port}{path}"));
        // 2027-01-15 07:00 UTC: the standup is in an hour.
        let now = 1_799_996_400;
        let got = fetch(&[at("/cal.ics"), at("/gone.ics")], now);
        let read = got[0].1.as_ref().unwrap();
        assert_eq!(read.name.as_deref(), Some("Test"));
        assert_eq!(read.meetings.len(), 1);
        assert_eq!(read.meetings[0].start, now + 3600);
        assert_eq!(got[1].1, Err("the address answered 404".to_string()));
    }
}
