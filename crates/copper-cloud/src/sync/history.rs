//! History going up — `POST /v1/sync/history`, from where the last push
//! stopped (Mac: `CloudHistory.swift`). Free of the network, so its rules can
//! be checked against a pretend server.
//!
//! A copper-cloud measures each entry as the JSON text it receives (16 KiB by
//! default, `max_history_entry_bytes`). So:
//!
//! - the limits come from `GET /v1/info` ([`Limits::from_info`]);
//! - each entry is encoded here, once, exactly as it is sent, and measured. A
//!   title too long is shortened to fit (with `…`); a place whose address
//!   alone is too big is left out and said in the log — a cut address is
//!   another address;
//! - a refused request is split until what is refused stands alone, and that
//!   is skipped while the rest goes up. A server that names the entry
//!   (`entry 3 …`) saves the splitting; one that names a smaller limit
//!   (`exceeds N bytes`, `at most N entries`) or turns a body away as too
//!   large (413) gets smaller requests from then on;
//! - eight refusals in a row with nothing taken between them is the server
//!   refusing history, not an entry: nothing more is skipped, the push stops,
//!   and the engine tries again later, waiting longer each time.
//!
//! The cursor only moves past visits that went up or were deliberately left
//! out — never past one still to send, and never past now.

use crate::error::Failure;
use serde_json::Value;
use std::collections::BTreeMap;

/// The most visits sent in one request, whatever the server allows.
pub const MOST: usize = 500;
/// Refusals in a row, with nothing accepted between them, that stop a push.
pub const BRAKE: usize = 8;
/// Chromium's epoch, 1601: no visit is older.
pub const EARLIEST: f64 = -11_644_473_600.0;

/// What a history push may weigh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `max_history_entry_bytes`: the most one entry's JSON may weigh.
    pub entry_bytes: usize,
    /// `max_history_batch`, never more than [`MOST`].
    pub batch: usize,
    /// One request's body at most.
    pub request_bytes: usize,
}

impl Limits {
    /// copper-cloud's own defaults, for a server that hasn't said.
    pub const STANDARD: Limits = Limits {
        entry_bytes: 16_384,
        batch: MOST,
        request_bytes: 1 << 20,
    };

    /// `/v1/info`'s `limits`; the standard ones for what it doesn't say.
    pub fn from_info(info: Option<&Value>) -> Limits {
        let limits = info.and_then(|i| i.get("limits"));
        let number = |key: &str| -> Option<usize> {
            let v = limits?.get(key)?;
            let n = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))?;
            (n > 0).then_some(n as usize)
        };
        let standard = Limits::STANDARD;
        let entry_bytes = number("max_history_entry_bytes")
            .unwrap_or(standard.entry_bytes)
            .max(256);
        let most = number("max_history_batch").unwrap_or(2_000);
        Limits {
            entry_bytes,
            batch: MOST.min(most),
            request_bytes: (entry_bytes + 64)
                .max(standard.request_bytes.min(most * (entry_bytes + 8) + 1_024)),
        }
    }
}

/// A place as history has it, reduced to what goes up.
#[derive(Debug, Clone, PartialEq)]
pub struct Visit {
    /// The history key (lowercased pretty address).
    pub key: String,
    pub url: String,
    pub title: String,
    /// The last visit, Unix seconds.
    pub at: f64,
}

/// One visit ready to go: its entry as it will be sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub visit: Visit,
    /// `visited_at`: the visit's time, never later than now.
    pub sent_at: f64,
    pub data: Vec<u8>,
    pub shortened: bool,
}

/// A visit that doesn't go up, and why — for the log. Never the address
/// itself: one this long usually carries a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub host: String,
    pub bytes: usize,
    pub why: String,
}

/// What [`item`] makes of a visit.
#[derive(Debug, Clone, PartialEq)]
pub enum Fit {
    Fits(Item),
    TooBig(Skip),
}

/// Each visit newer than the cursor, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot {
    Send(Item),
    Known,
    Left(Skip),
}

/// `2026-10-01T12:00:00.123Z` — ISO 8601 with milliseconds, UTC (Mac:
/// `CloudSync.stamp`).
pub fn stamp(at: f64) -> String {
    let millis_total = (at * 1000.0).floor() as i64;
    let secs = millis_total.div_euclid(1000);
    let millis = millis_total.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// RFC 3339 → Unix seconds (fractions kept), lenient like `IsoDate::parse`.
pub fn parse_stamp(text: &str) -> Option<f64> {
    let whole = copper_core::date::IsoDate::parse(text)?.unix() as f64;
    // IsoDate drops the fraction; add it back.
    let frac = text
        .split_once('.')
        .map(|(_, rest)| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|d| !d.is_empty())
        .and_then(|d| format!("0.{d}").parse::<f64>().ok())
        .unwrap_or(0.0);
    Some(whole + frac)
}

/// The largest double below `x` (Swift's `nextDown`).
pub(crate) fn next_down(x: f64) -> f64 {
    if x.is_nan() || x == f64::NEG_INFINITY {
        return x;
    }
    if x == 0.0 {
        return -f64::from_bits(1);
    }
    let bits = x.to_bits();
    f64::from_bits(if x > 0.0 { bits - 1 } else { bits + 1 })
}

/// The host of an address, or its first 40 characters.
pub fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.chars().take(40).collect())
}

/// One entry's JSON — keys sorted, slashes as they are — so a request is the
/// same bytes each time and an address weighs what it reads.
pub fn encode_entry(url: &str, title: &str, visited_at: &str) -> Vec<u8> {
    let s = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
    format!(
        "{{\"title\":{},\"url\":{},\"visited_at\":{}}}",
        s(title),
        s(url),
        s(visited_at)
    )
    .into_bytes()
}

/// The entry for `visit`, its title shortened if that is what it takes to
/// fit `limit`. A [`Skip`] when even the address alone is too big.
pub fn item(visit: &Visit, now: f64, limit: usize) -> Fit {
    let sent_at = visit.at.max(EARLIEST).min(now);
    let stamp = stamp(sent_at);
    let encode = |title: &str| encode_entry(&visit.url, title, &stamp);
    let full = encode(&visit.title);
    if full.len() <= limit {
        return Fit::Fits(Item {
            visit: visit.clone(),
            sent_at,
            data: full,
            shortened: false,
        });
    }
    let bare = encode("");
    if bare.len() > limit {
        return Fit::TooBig(Skip {
            host: host(&visit.url),
            bytes: full.len(),
            why: format!("over the cloud's {limit} bytes per visit"),
        });
    }
    // The longest start of the title that fits with an ellipsis after it.
    let chars: Vec<char> = visit.title.chars().collect();
    let (mut low, mut high) = (0_i64, chars.len() as i64 - 1);
    let mut best = (String::new(), bare);
    while low <= high {
        let middle = (low + high) / 2;
        let mut title: String = chars[..middle as usize].iter().collect();
        title.push('…');
        let data = encode(&title);
        if data.len() <= limit {
            best = (title, data);
            low = middle + 1;
        } else {
            high = middle - 1;
        }
    }
    let mut shortened = visit.clone();
    shortened.title = best.0;
    Fit::Fits(Item {
        visit: shortened,
        sent_at,
        data: best.1,
        shortened: true,
    })
}

/// Every visit newer than `through`, oldest first (ties by key). `known` is
/// what is on the server already at that time.
pub fn plan(
    visits: &[Visit],
    through: f64,
    known: &BTreeMap<String, f64>,
    now: f64,
    limits: Limits,
) -> Vec<(Visit, Slot)> {
    let mut newer: Vec<&Visit> = visits.iter().filter(|v| v.at > through).collect();
    newer.sort_by(|a, b| {
        a.at.partial_cmp(&b.at)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.key.cmp(&b.key))
    });
    newer
        .into_iter()
        .map(|visit| {
            if let Some(theirs) = known.get(&visit.key)
                && (theirs - visit.at).abs() < 1.0
            {
                return (visit.clone(), Slot::Known);
            }
            let slot = match item(visit, now, limits.entry_bytes) {
                Fit::Fits(item) => Slot::Send(item),
                Fit::TooBig(skip) => Slot::Left(skip),
            };
            (visit.clone(), slot)
        })
        .collect()
}

/// `{"entries":[…]}` from entries already encoded: what was measured is what
/// is sent.
pub fn body(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"{\"entries\":[".to_vec();
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        out.extend_from_slice(entry);
    }
    out.extend_from_slice(b"]}");
    out
}

/// A 0.5.0 server stores what it can and lists the entries it skipped.
pub fn rejected(answer: &[u8]) -> Vec<(usize, String)> {
    let Ok(object) = serde_json::from_slice::<Value>(answer) else {
        return vec![];
    };
    let Some(list) = object.get("rejected").and_then(Value::as_array) else {
        return vec![];
    };
    list.iter()
        .filter_map(|row| {
            let index = row.get("index")?.as_u64()? as usize;
            let why = row
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| row.get("reason").and_then(Value::as_str))
                .unwrap_or("refused");
            Some((index, why.to_string()))
        })
        .collect()
}

/// A refusal of what was sent rather than of the asking: split, or skip.
pub fn refuses(failure: &Failure) -> bool {
    matches!(failure.status, 400 | 413 | 422)
}

/// The number after `word` in a server message: `entry 3 …`, `at most 500`,
/// `exceeds 16384 bytes`.
pub fn number_after(word: &str, text: &str) -> Option<usize> {
    let at = text.find(word)? + word.len();
    let digits: String = text[at..]
        .trim_start_matches(' ')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// One push: hands out requests ([`Upload::next_request`]), is told how each went
/// ([`Upload::accepted`], [`Upload::refused`]), and says how far the cursor may
/// go ([`Upload::through`]). No network here.
#[derive(Debug)]
pub struct Upload {
    limits: Limits,
    now: f64,
    start: f64,
    visits: Vec<Visit>,
    slots: Vec<Slot>,
    done: Vec<bool>,
    prefix: usize,
    queue: Vec<Vec<usize>>,
    asked: Option<Vec<usize>>,
    held: Vec<(usize, String)>,
    /// Visits that went up.
    pub sent: usize,
    /// Titles shortened to fit.
    pub shortened: usize,
    /// Visits left out or refused, in the order it happened.
    pub skipped: Vec<Skip>,
    /// Requests made.
    pub requests: usize,
}

impl Upload {
    pub fn new(plan: Vec<(Visit, Slot)>, limits: Limits, through: f64, now: f64) -> Self {
        let (visits, slots): (Vec<Visit>, Vec<Slot>) = plan.into_iter().unzip();
        let done: Vec<bool> = slots.iter().map(|s| !matches!(s, Slot::Send(_))).collect();
        let mut skipped = vec![];
        let mut shortened = 0;
        for slot in &slots {
            match slot {
                Slot::Left(skip) => skipped.push(skip.clone()),
                Slot::Send(item) if item.shortened => shortened += 1,
                _ => {}
            }
        }
        let mut up = Upload {
            limits,
            now,
            start: through,
            visits,
            slots,
            done,
            prefix: 0,
            queue: vec![],
            asked: None,
            held: vec![],
            sent: 0,
            shortened,
            skipped,
            requests: 0,
        };
        let waiting: Vec<usize> = (0..up.slots.len()).filter(|i| !up.done[*i]).collect();
        up.queue = up.chunks(&waiting);
        up.advance();
        up
    }

    /// The limits now (smaller after a refusal said so).
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The entries of the next request, or `None` when every visit is dealt
    /// with.
    pub fn next_request(&mut self) -> Option<Vec<Vec<u8>>> {
        assert!(self.asked.is_none(), "answer the last request first");
        if self.queue.is_empty() {
            // Refused at the very end, with fewer than BRAKE of them: skipped.
            self.settle_held();
            self.advance();
            return None;
        }
        let indexes = self.queue.remove(0);
        let entries = indexes
            .iter()
            .filter_map(|i| match &self.slots[*i] {
                Slot::Send(item) => Some(item.data.clone()),
                _ => None,
            })
            .collect();
        self.asked = Some(indexes);
        self.requests += 1;
        Some(entries)
    }

    /// 2xx. `rejected`: entries a newer server skipped while storing the rest.
    pub fn accepted(&mut self, rejected: &[(usize, String)]) {
        let Some(indexes) = self.asked.take() else {
            return;
        };
        let mut refused_here: Vec<usize> = rejected.iter().map(|r| r.0).collect();
        refused_here.sort_unstable();
        refused_here.dedup();
        for (position, index) in indexes.iter().enumerate() {
            self.done[*index] = true;
            if let Some((_, why)) = rejected.iter().find(|r| r.0 == position) {
                let skip = self.skip(*index, &format!("the cloud skipped it: {why}"));
                self.skipped.push(skip);
            }
        }
        self.sent += indexes.len() - refused_here.iter().filter(|i| **i < indexes.len()).count();
        self.settle_held();
        self.advance();
    }

    /// A 400/413 for the last request (see [`refuses`]). An error when it is
    /// the server refusing history rather than an entry.
    pub fn refused(&mut self, failure: &Failure) -> Result<(), Failure> {
        let Some(indexes) = self.asked.take() else {
            return Ok(());
        };
        let text = failure.message.clone();
        if failure.status == 413 && indexes.len() > 1 {
            let size = indexes.iter().fold(14, |total, i| match &self.slots[*i] {
                Slot::Send(item) => total + item.data.len() + 1,
                _ => total,
            });
            self.limits.request_bytes =
                (self.limits.entry_bytes + 64).max(self.limits.request_bytes.min(size / 2));
            self.requeue(&indexes);
            return Ok(());
        }
        let named = number_after("entry ", &text);
        if named.is_none()
            && let Some(most) = number_after("at most ", &text)
            && most >= 1
            && most < indexes.len()
        {
            self.limits.batch = self.limits.batch.min(most);
            self.requeue(&indexes);
            return Ok(());
        }
        if let Some(position) = named
            && position < indexes.len()
        {
            if let Some(size) = number_after("exceeds ", &text)
                && size >= 64
                && size < self.limits.entry_bytes
            {
                self.limits.entry_bytes = size;
                self.limits.request_bytes = self.limits.request_bytes.max(size + 64);
                self.refit();
                self.requeue(&indexes);
                return Ok(());
            }
            self.hold(indexes[position], &text)?;
            let mut rest = indexes.clone();
            rest.remove(position);
            if !rest.is_empty() {
                self.queue.insert(0, rest);
            }
            return Ok(());
        }
        if indexes.len() == 1 {
            return self.hold(indexes[0], &text);
        }
        let middle = indexes.len() / 2;
        self.queue.insert(0, indexes[middle..].to_vec());
        self.queue.insert(0, indexes[..middle].to_vec());
        Ok(())
    }

    /// How far `pushedThrough` may go: past every visit dealt with from the
    /// start, never past one still to send, and never past now.
    pub fn through(&self) -> f64 {
        if self.prefix == 0 {
            return self.start;
        }
        let mut through = self
            .start
            .max(self.visits[self.prefix - 1].at.min(self.now));
        if self.prefix < self.visits.len() && self.visits[self.prefix].at <= through {
            through = self.start.max(next_down(self.visits[self.prefix].at));
        }
        through
    }

    /// Visits dealt with that are stamped later than now: the cursor stops at
    /// now, so they are remembered instead (`cloud-sync.json` `remote`).
    pub fn ahead(&self) -> Vec<(String, f64)> {
        (0..self.visits.len())
            .filter(|i| self.done[*i] && self.visits[*i].at > self.now)
            .map(|i| (self.visits[i].key.clone(), self.visits[i].at))
            .collect()
    }

    fn skip(&self, index: usize, why: &str) -> Skip {
        let bytes = match &self.slots[index] {
            Slot::Send(item) => item.data.len(),
            _ => 0,
        };
        Skip {
            host: host(&self.visits[index].url),
            bytes,
            why: why.to_string(),
        }
    }

    fn hold(&mut self, index: usize, why: &str) -> Result<(), Failure> {
        self.held.push((index, why.to_string()));
        if self.held.len() >= BRAKE {
            return Err(Failure::new(
                400,
                "history_refused",
                format!(
                    "The cloud refused {} visits in a row ({why}) — none skipped; will try again later",
                    self.held.len()
                ),
            ));
        }
        Ok(())
    }

    fn settle_held(&mut self) {
        let held = std::mem::take(&mut self.held);
        for (index, why) in held {
            self.done[index] = true;
            let skip = self.skip(index, &format!("the cloud refused it: {why}"));
            self.skipped.push(skip);
        }
    }

    fn advance(&mut self) {
        while self.prefix < self.done.len() && self.done[self.prefix] {
            self.prefix += 1;
        }
    }

    fn requeue(&mut self, indexes: &[usize]) {
        let mut waiting: Vec<usize> = indexes
            .iter()
            .copied()
            .chain(self.queue.iter().flatten().copied())
            .filter(|i| !self.done[*i])
            .collect();
        waiting.sort_unstable();
        self.queue = self.chunks(&waiting);
    }

    fn refit(&mut self) {
        let holding: Vec<usize> = self.held.iter().map(|h| h.0).collect();
        for index in 0..self.slots.len() {
            if self.done[index] || holding.contains(&index) {
                continue;
            }
            let Slot::Send(item) = &self.slots[index] else {
                continue;
            };
            if item.data.len() <= self.limits.entry_bytes {
                continue;
            }
            let was_short = item.shortened;
            match self::item(&self.visits[index], self.now, self.limits.entry_bytes) {
                Fit::Fits(fitted) => {
                    if !was_short && fitted.shortened {
                        self.shortened += 1;
                    }
                    self.slots[index] = Slot::Send(fitted);
                }
                Fit::TooBig(skip) => {
                    self.skipped.push(skip.clone());
                    self.slots[index] = Slot::Left(skip);
                    self.done[index] = true;
                }
            }
        }
        self.advance();
    }

    fn chunks(&self, indexes: &[usize]) -> Vec<Vec<usize>> {
        let mut out = vec![];
        let mut group: Vec<usize> = vec![];
        let mut bytes = 14;
        for &index in indexes {
            let Slot::Send(item) = &self.slots[index] else {
                continue;
            };
            let size = item.data.len() + 1;
            if !group.is_empty()
                && (group.len() >= self.limits.batch || bytes + size > self.limits.request_bytes)
            {
                out.push(std::mem::take(&mut group));
                bytes = 14;
            }
            group.push(index);
            bytes += size;
        }
        if !group.is_empty() {
            out.push(group);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visit(n: usize, at: f64) -> Visit {
        Visit {
            key: format!("site{n}.test"),
            url: format!("https://site{n}.test/"),
            title: format!("Site {n}"),
            at,
        }
    }

    #[test]
    fn limits_from_info() {
        let info: Value = serde_json::json!({"limits": {"max_blob_bytes": 8000000, "max_history_batch": 2000, "max_history_entry_bytes": 16384}});
        let l = Limits::from_info(Some(&info));
        assert_eq!(l.entry_bytes, 16_384);
        assert_eq!(l.batch, 500);
        assert_eq!(l.request_bytes, 1 << 20);
        let l = Limits::from_info(Some(
            &serde_json::json!({"limits": {"max_history_entry_bytes": 10, "max_history_batch": 3}}),
        ));
        assert_eq!(l.entry_bytes, 256);
        assert_eq!(l.batch, 3);
        assert_eq!(l.request_bytes, 3 * (256 + 8) + 1024);
        assert_eq!(
            Limits::from_info(None),
            Limits {
                entry_bytes: 16_384,
                batch: 500,
                request_bytes: 1 << 20
            }
        );
    }

    #[test]
    fn stamps() {
        assert_eq!(stamp(0.0), "1970-01-01T00:00:00.000Z");
        assert_eq!(stamp(1_790_000_000.25), "2026-09-21T14:13:20.250Z");
        assert_eq!(stamp(EARLIEST), "1601-01-01T00:00:00.000Z");
        assert_eq!(
            parse_stamp("2026-09-21T14:13:20.250Z"),
            Some(1_790_000_000.25)
        );
        assert_eq!(parse_stamp("2026-09-21T14:13:20Z"), Some(1_790_000_000.0));
    }

    #[test]
    fn entries_are_sorted_and_slashes_kept() {
        let e = encode_entry("https://a.test/x", "T \"q\"", "2026-01-01T00:00:00.000Z");
        assert_eq!(
            String::from_utf8(e).unwrap(),
            r#"{"title":"T \"q\"","url":"https://a.test/x","visited_at":"2026-01-01T00:00:00.000Z"}"#
        );
    }

    #[test]
    fn long_titles_shorten_long_addresses_are_left_out() {
        let now = 2_000_000_000.0;
        let mut v = visit(1, 1_000.0);
        v.title = "x".repeat(20_000);
        let Fit::Fits(fitted) = item(&v, now, 16_384) else {
            panic!("fits")
        };
        assert!(fitted.shortened);
        assert!(fitted.data.len() <= 16_384);
        assert!(fitted.data.len() > 16_300);
        assert!(fitted.visit.title.ends_with('…'));
        let mut v = visit(2, 1_000.0);
        v.url = format!("https://big.test/?{}", "a".repeat(20_000));
        let Fit::TooBig(skip) = item(&v, now, 16_384) else {
            panic!("too big")
        };
        assert_eq!(skip.host, "big.test");
        // A visit from the future goes up stamped now.
        let v = visit(3, now + 100.0);
        let Fit::Fits(fitted) = item(&v, now, 16_384) else {
            panic!()
        };
        assert_eq!(fitted.sent_at, now);
    }

    fn run(
        up: &mut Upload,
        mut server: impl FnMut(&[Vec<u8>]) -> Result<Vec<(usize, String)>, Failure>,
    ) -> Result<(), Failure> {
        while let Some(entries) = up.next_request() {
            match server(&entries) {
                Ok(rejected) => up.accepted(&rejected),
                Err(f) => up.refused(&f)?,
            }
        }
        Ok(())
    }

    #[test]
    fn upload_splits_and_skips_the_refused_one() {
        let visits: Vec<Visit> = (0..10).map(|n| visit(n, 100.0 + n as f64)).collect();
        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, Limits::STANDARD);
        let mut up = Upload::new(p, Limits::STANDARD, 0.0, 1_000.0);
        let mut stored = vec![];
        run(&mut up, |entries| {
            if entries
                .iter()
                .any(|e| String::from_utf8_lossy(e).contains("site4."))
            {
                return Err(Failure::new(400, "bad_request", "invalid entry"));
            }
            stored.extend(entries.iter().cloned());
            Ok(vec![])
        })
        .unwrap();
        assert_eq!(stored.len(), 9);
        assert_eq!(up.sent, 9);
        assert_eq!(up.skipped.len(), 1);
        assert_eq!(up.through(), 109.0);
    }

    #[test]
    fn named_entries_smaller_batches_and_413() {
        let visits: Vec<Visit> = (0..20).map(|n| visit(n, 100.0 + n as f64)).collect();
        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, Limits::STANDARD);
        let mut up = Upload::new(p, Limits::STANDARD, 0.0, 1_000.0);
        let mut calls = vec![];
        run(&mut up, |entries| {
            calls.push(entries.len());
            if entries.len() > 5 {
                return Err(Failure::new(
                    400,
                    "bad_request",
                    "at most 5 entries per request",
                ));
            }
            Ok(vec![])
        })
        .unwrap();
        assert_eq!(up.sent, 20);
        assert_eq!(calls, vec![20, 5, 5, 5, 5]);
        assert_eq!(up.limits().batch, 5);

        // 413: requests get smaller until they are taken.
        let small = Limits {
            entry_bytes: 256,
            ..Limits::STANDARD
        };
        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, small);
        let mut up = Upload::new(p, small, 0.0, 1_000.0);
        run(&mut up, |entries| {
            if entries.len() > 4 {
                return Err(Failure::new(413, "payload_too_large", "too large"));
            }
            Ok(vec![])
        })
        .unwrap();
        assert_eq!(up.sent, 20);
        assert!(up.limits().request_bytes < 1 << 20);

        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, Limits::STANDARD);
        let mut up = Upload::new(p, Limits::STANDARD, 0.0, 1_000.0);
        run(&mut up, |entries| {
            if let Some(i) = entries
                .iter()
                .position(|e| String::from_utf8_lossy(e).contains("site7."))
            {
                return Err(Failure::new(
                    400,
                    "bad_request",
                    format!("entry {i} is not valid"),
                ));
            }
            Ok(vec![])
        })
        .unwrap();
        assert_eq!(up.sent, 19);
        assert_eq!(up.requests, 2);
    }

    #[test]
    fn server_rejections_and_the_brake() {
        let visits: Vec<Visit> = (0..20).map(|n| visit(n, 100.0 + n as f64)).collect();
        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, Limits::STANDARD);
        let mut up = Upload::new(p, Limits::STANDARD, 0.0, 1_000.0);
        run(&mut up, |_| Ok(vec![(3, "too_large".into())])).unwrap();
        assert_eq!(up.sent, 19);
        assert_eq!(up.skipped.len(), 1);
        assert_eq!(up.through(), 119.0);

        // Refusing everything: the push stops; nothing skipped, cursor unmoved.
        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, Limits::STANDARD);
        let mut up = Upload::new(p, Limits::STANDARD, 0.0, 1_000.0);
        let err = run(&mut up, |_| Err(Failure::new(400, "bad_request", "no"))).unwrap_err();
        assert_eq!(err.code, "history_refused");
        assert_eq!(up.skipped.len(), 0);
        assert_eq!(up.through(), 0.0);
    }

    #[test]
    fn known_and_cursor_rules() {
        let mut visits: Vec<Visit> = (0..3).map(|n| visit(n, 100.0 + n as f64)).collect();
        visits.push(visit(9, 5_000.0)); // in the future
        let mut known = BTreeMap::new();
        known.insert("site1.test".to_string(), 101.4);
        let p = plan(&visits, 100.0, &known, 1_000.0, Limits::STANDARD);
        assert_eq!(p.len(), 3);
        assert!(matches!(p[0].1, Slot::Known));
        let mut up = Upload::new(p, Limits::STANDARD, 100.0, 1_000.0);
        run(&mut up, |_| Ok(vec![])).unwrap();
        assert_eq!(up.through(), 1_000.0);
        assert_eq!(up.ahead(), vec![("site9.test".to_string(), 5_000.0)]);
        // A later visit at the same instant keeps the cursor just short.
        let visits = vec![visit(1, 200.0), visit(2, 200.0)];
        let p = plan(&visits, 0.0, &BTreeMap::new(), 1_000.0, Limits::STANDARD);
        let mut up = Upload::new(
            p,
            Limits {
                batch: 1,
                ..Limits::STANDARD
            },
            0.0,
            1_000.0,
        );
        let first = up.next_request().unwrap();
        assert_eq!(first.len(), 1);
        up.accepted(&[]);
        assert_eq!(up.through(), next_down(200.0));
    }

    #[test]
    fn numbers_in_messages() {
        assert_eq!(
            number_after("entry ", "entry 3 exceeds 4096 bytes"),
            Some(3)
        );
        assert_eq!(
            number_after("exceeds ", "entry 3 exceeds 4096 bytes"),
            Some(4096)
        );
        assert_eq!(number_after("at most ", "at most  100 entries"), Some(100));
        assert_eq!(number_after("entry ", "nothing"), None);
    }
}
