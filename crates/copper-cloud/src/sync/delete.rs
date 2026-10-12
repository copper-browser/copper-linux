//! Deleting this account's history on Copper Cloud — `DELETE
//! /v1/sync/history`, on a copper-cloud that lists `history_delete` in `GET
//! /v1/info`'s `features` (0.8.0). Mac: `CloudHistoryDelete.swift`.
//!
//! Only the person picks what goes, and only this device reads history to
//! find it. The server never opens history to filter it: it deletes the rows
//! the client names by `seq`, or wholesale — a time window, a device,
//! everything. So a window / device / everything is one request with the
//! server's own filters; a site or a page is found here by paging through
//! every device's rows and deleting the matches by seq, 5,000 a request.
//! Never seqs and filters in one request, and nothing chosen is never
//! "everything". Nothing is deleted until the looking is done.

use super::history::{Visit, parse_stamp, stamp};
use crate::error::Failure;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;

/// What `GET /v1/info`'s `features` lists when the server can delete.
pub const FEATURE: &str = "history_delete";
/// The most seqs one request may name.
pub const BATCH: usize = 5_000;
/// Rows asked for per page while looking.
pub const PAGE_SIZE: usize = 500;
/// Pages read at most in one delete — 500,000 visits — so it always ends.
pub const PAGE_LIMIT: usize = 1_000;
/// Said instead of the controls when the cloud can't.
pub const UNSUPPORTED: &str =
    "This cloud can't delete history — it needs copper-cloud 0.8.0 or later.";

/// What to delete. The window, the device and the sites or pages all have to
/// agree; among themselves, sites and pages are either-or.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Selector {
    /// `visited_at` from (inclusive), Unix seconds…
    pub since: Option<f64>,
    /// …up to (not including).
    pub until: Option<f64>,
    /// One device's rows only (lowercase UUID).
    pub device: Option<String>,
    /// Sites as [`site`] writes them, each with its subdomains.
    pub hosts: Vec<String>,
    /// Pages, by history key.
    pub pages: Vec<String>,
    /// All of it. Has to be said: nothing chosen is never everything.
    pub everything: bool,
}

impl Selector {
    /// Sites or pages: only this device can tell which rows those are.
    pub fn scans(&self) -> bool {
        !self.hosts.is_empty() || !self.pages.is_empty()
    }

    pub fn narrowed(&self) -> bool {
        self.since.is_some() || self.until.is_some() || self.device.is_some() || self.scans()
    }

    /// Something chosen, or everything — never both, never neither — and a
    /// window that isn't empty.
    pub fn valid(&self) -> bool {
        if let (Some(s), Some(u)) = (self.since, self.until)
            && s >= u
        {
            return false;
        }
        self.narrowed() != self.everything
    }

    /// History cleared here: everything there.
    pub fn cleared() -> Self {
        Selector {
            everything: true,
            ..Default::default()
        }
    }

    /// A page forgotten here: its rows there, every device's.
    pub fn forgot(key: &str) -> Self {
        Selector {
            pages: vec![key.to_string()],
            ..Default::default()
        }
    }
}

/// One request to `/v1/sync/history`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub method: &'static str,
    pub query: Vec<(&'static str, String)>,
    /// `{"seqs": […]}`, and then no query.
    pub body: Option<Vec<u8>>,
}

impl Ask {
    /// The query as `request` takes it.
    pub fn query_pairs(&self) -> Vec<(&str, &str)> {
        self.query.iter().map(|(k, v)| (*k, v.as_str())).collect()
    }
}

/// The page after `since`, every device's rows (no `exclude_device`).
pub fn page(since: i64) -> Ask {
    Ask {
        method: "GET",
        query: vec![
            ("since", since.to_string()),
            ("limit", PAGE_SIZE.to_string()),
        ],
        body: None,
    }
}

/// Deleting by the server's own filters, RFC 3339 in UTC.
pub fn window(selector: &Selector) -> Ask {
    let mut query = vec![];
    if let Some(s) = selector.since {
        query.push(("since", stamp(s)));
    }
    if let Some(u) = selector.until {
        query.push(("until", stamp(u)));
    }
    if let Some(d) = &selector.device {
        query.push(("device", d.to_ascii_lowercase()));
    }
    Ask {
        method: "DELETE",
        query,
        body: None,
    }
}

/// Deleting rows by name. Written by hand: an encoder that could fail would
/// leave a body-less, query-less DELETE — which is everything.
pub fn seqs(seqs: &[i64]) -> Ask {
    let list: Vec<String> = seqs.iter().map(i64::to_string).collect();
    Ask {
        method: "DELETE",
        query: vec![],
        body: Some(format!("{{\"seqs\":[{}]}}", list.join(",")).into_bytes()),
    }
}

/// `{"deleted": n}`.
pub fn deleted(answer: &[u8]) -> usize {
    serde_json::from_slice::<Value>(answer)
        .ok()
        .and_then(|v| v.get("deleted").and_then(Value::as_u64))
        .unwrap_or(0) as usize
}

/// A row as `GET /v1/sync/history` sends it, read leniently.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub seq: i64,
    pub device: Option<String>,
    pub url: Option<String>,
    pub stamp: Option<String>,
    pub payload_stamp: Option<String>,
    pub payload_seconds: Option<f64>,
}

impl Row {
    fn read(v: &Value) -> Option<Row> {
        let payload = v.get("payload");
        let pv = payload.and_then(|p| p.get("visited_at"));
        Some(Row {
            seq: v.get("seq")?.as_i64()?,
            device: v
                .get("device_id")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase),
            url: payload
                .and_then(|p| p.get("url"))
                .and_then(Value::as_str)
                .map(str::to_string),
            stamp: v
                .get("visited_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            payload_stamp: pv.and_then(Value::as_str).map(str::to_string),
            payload_seconds: pv.and_then(Value::as_f64),
        })
    }

    /// When: the server's own `visited_at`, else the payload's.
    pub fn at(&self) -> Option<f64> {
        self.stamp
            .as_deref()
            .and_then(parse_stamp)
            .or_else(|| self.payload_stamp.as_deref().and_then(parse_stamp))
            .or_else(|| {
                self.payload_seconds
                    .map(|s| if s > 1e11 { s / 1000.0 } else { s })
            })
    }
}

/// One page of history while looking.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub entries: Vec<Row>,
    pub next: i64,
    pub more: Option<bool>,
}

impl Page {
    pub fn read(data: &[u8]) -> Option<Page> {
        #[derive(Deserialize)]
        struct Wire {
            entries: Vec<Value>,
            next: i64,
            #[serde(default)]
            more: Option<bool>,
        }
        let wire: Wire = serde_json::from_slice(data).ok()?;
        Some(Page {
            entries: wire.entries.iter().filter_map(Row::read).collect(),
            next: wire.next,
            more: wire.more,
        })
    }
}

fn bare(host: &str) -> String {
    host.to_lowercase().trim_end_matches('.').to_string()
}

/// `host` is `site` or one of its subdomains.
pub fn within(host: &str, site: &str) -> bool {
    host == site || host.ends_with(&format!(".{site}"))
}

fn url_host(raw: &str) -> Option<(url::Url, String)> {
    let url = url::Url::parse(raw).ok()?;
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    Some((url, host))
}

/// Whether a place, visited at `at` from `device`, is one `selector` names.
pub fn covers(
    selector: &Selector,
    url: Option<&str>,
    at: impl FnOnce() -> Option<f64>,
    device: Option<&str>,
) -> bool {
    if let Some(wanted) = &selector.device
        && device.map(str::to_ascii_lowercase).as_deref()
            != Some(wanted.to_ascii_lowercase().as_str())
    {
        return false;
    }
    if selector.since.is_some() || selector.until.is_some() {
        let Some(at) = at() else { return false };
        if selector.since.is_some_and(|s| at < s) {
            return false;
        }
        if selector.until.is_some_and(|u| at >= u) {
            return false;
        }
    }
    if !selector.scans() {
        return true;
    }
    let Some((parsed, host)) = url.and_then(url_host) else {
        return false;
    };
    let here = bare(&host);
    if selector.hosts.iter().any(|s| within(&here, s)) {
        return true;
    }
    !selector.pages.is_empty()
        && selector
            .pages
            .contains(&copper_core::address::history_key(&parsed))
}

/// A site as typed — `x.com`, `https://www.x.com/a`, `X.COM.`, `*.x.com` — as
/// [`Selector::hosts`] keeps it. `None` for what isn't one.
pub fn site(typed: &str) -> Option<String> {
    let mut text = typed.trim().to_lowercase();
    if let Some(rest) = text.strip_prefix("*.") {
        text = rest.to_string();
    }
    if text.is_empty() || text.contains(' ') {
        return None;
    }
    let full = if text.contains("://") {
        text.clone()
    } else {
        format!("https://{text}")
    };
    let (_, host) = url_host(&full)?;
    let mut site = bare(&host);
    if let Some(rest) = site.strip_prefix("www.") {
        site = rest.to_string();
    }
    (site == "localhost" || (site.contains('.') && !site.starts_with('.') && !site.contains("..")))
        .then_some(site)
}

/// `30m`, `1h`, `24h`, `7d`, `2w` before `now`, or an RFC 3339 time.
pub fn time(text: &str, now: f64) -> Option<f64> {
    let text = text.trim();
    let mut chars = text.chars();
    if let Some(unit) = chars.next_back()
        && let Ok(count) = chars.as_str().parse::<f64>()
        && count >= 0.0
        && count.is_finite()
    {
        let seconds = match unit {
            'm' => 60.0,
            'h' => 3_600.0,
            'd' => 86_400.0,
            'w' => 604_800.0,
            _ => return parse_stamp(text),
        };
        return Some(now - count * seconds);
    }
    parse_stamp(text)
}

/// What a delete came to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub deleted: usize,
    /// Rows read here while looking (none for a window).
    pub looked: usize,
    pub matched: usize,
    pub requests: usize,
    /// Looked through here, rather than left to the server's filters.
    pub scanned: bool,
    /// Looked through the first [`PAGE_LIMIT`] pages only.
    pub cut: bool,
}

impl Outcome {
    pub fn json(&self) -> Value {
        serde_json::json!({
            "deleted": self.deleted, "looked": self.looked, "matched": self.matched,
            "requests": self.requests, "scanned": self.scanned, "complete": !self.cut,
        })
    }
}

/// One delete, start to end. `ask` makes one request and returns the
/// answer's bytes, or the server's refusal.
pub async fn run<F, Fut>(
    selector: &Selector,
    page_limit: usize,
    mut ask: F,
) -> Result<Outcome, Failure>
where
    F: FnMut(Ask) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, Failure>>,
{
    if !selector.valid() {
        return Err(Failure::new(
            0,
            "bad_selector",
            "Say what to delete — a time, a site, a page, a device — or ask for everything",
        ));
    }
    let mut outcome = Outcome::default();
    if !selector.scans() {
        outcome.deleted = deleted(&ask(window(selector)).await?);
        outcome.requests = 1;
        return Ok(outcome);
    }
    outcome.scanned = true;
    let mut since = 0_i64;
    let mut pages = 0;
    let mut found: Vec<i64> = vec![];
    loop {
        let data = ask(page(since)).await?;
        outcome.requests += 1;
        let Some(p) = Page::read(&data) else {
            return Err(Failure::new(
                200,
                "decode",
                "The cloud's history didn't read — nothing was deleted",
            ));
        };
        pages += 1;
        outcome.looked += p.entries.len();
        for row in &p.entries {
            if covers(
                selector,
                row.url.as_deref(),
                || row.at(),
                row.device.as_deref(),
            ) {
                found.push(row.seq);
            }
        }
        let moved = p.next > since;
        since = since.max(p.next);
        if p.more != Some(true) || p.entries.is_empty() || !moved {
            break;
        }
        if pages >= page_limit {
            outcome.cut = true;
            break;
        }
    }
    outcome.matched = found.len();
    let mut count = 0;
    for group in found.chunks(BATCH) {
        match ask(seqs(group)).await {
            Ok(answer) => {
                count += deleted(&answer);
                outcome.requests += 1;
            }
            Err(failure) => {
                if count == 0 {
                    return Err(failure);
                }
                return Err(Failure::new(
                    0,
                    "partial",
                    format!(
                        "Deleted {count} visit{}, then: {}",
                        if count == 1 { "" } else { "s" },
                        failure.message
                    ),
                ));
            }
        }
    }
    outcome.deleted = count;
    Ok(outcome)
}

/// What became of History here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Here {
    /// Left as it was: a delete from Settings, the CLI or an agent.
    Kept,
    /// Cleared, all of it.
    Cleared,
    /// One page forgotten.
    Forgot,
}

/// The push's cursor and `remote` once a delete has gone through, so the next
/// push sends back nothing it deleted.
pub fn push_after(
    selector: &Selector,
    here: Here,
    visits: &[Visit],
    through: Option<f64>,
    remote: &BTreeMap<String, f64>,
    device: Option<&str>,
    now: f64,
) -> (Option<f64>, BTreeMap<String, f64>) {
    match here {
        Here::Kept => {
            let held = held(visits, selector, through.unwrap_or(0.0).min(now), device);
            let mut out = remote.clone();
            for (k, v) in held {
                out.entry(k).or_insert(v);
            }
            (through, out)
        }
        Here::Cleared => (Some(through.unwrap_or(0.0).max(now)), BTreeMap::new()),
        Here::Forgot => (through, remote.clone()),
    }
}

/// Visits here that a delete covers and that haven't gone up yet.
pub fn held(
    visits: &[Visit],
    selector: &Selector,
    through: f64,
    device: Option<&str>,
) -> BTreeMap<String, f64> {
    visits
        .iter()
        .filter(|v| v.at > through && covers(selector, Some(&v.url), || Some(v.at), device))
        .map(|v| (v.key.clone(), v.at))
        .collect()
}

/// Visits without the pages forgotten here this run, unless visited again
/// since.
pub fn unforgotten(visits: Vec<Visit>, forgotten: &BTreeMap<String, f64>) -> Vec<Visit> {
    if forgotten.is_empty() {
        return visits;
    }
    visits
        .into_iter()
        .filter(|v| forgotten.get(&v.key).is_none_or(|at| v.at > *at))
        .collect()
}

/// What an agent or a CLI asked for: `since`, `until` (`1h`, `7d`, RFC 3339),
/// `host` and `page` (one or a list), `device` (an id, or `this`), `all`.
pub fn selector_from(args: &Value, now: f64, me: Option<&str>) -> Result<Selector, Failure> {
    let refuse = |t: String| Failure::new(0, "bad_selector", t);
    let strings = |key: &str| -> Vec<String> {
        match args.get(key) {
            Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            _ => vec![],
        }
    };
    let mut selector = Selector::default();
    for key in ["since", "until"] {
        let Some(text) = strings(key).into_iter().next() else {
            continue;
        };
        let Some(date) = time(&text, now) else {
            return Err(refuse(format!(
                "{key}: {text} isn't a time — say 1h, 24h, 7d or an RFC 3339 time"
            )));
        };
        if key == "since" {
            selector.since = Some(date);
        } else {
            selector.until = Some(date);
        }
    }
    for typed in strings("host") {
        let Some(s) = site(&typed) else {
            return Err(refuse(format!(
                "host: {typed} isn't a site — say example.com"
            )));
        };
        if !selector.hosts.contains(&s) {
            selector.hosts.push(s);
        }
    }
    for typed in strings("page") {
        let url = copper_core::address::url_from_typed(&typed)
            .filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
        let Some(url) = url else {
            return Err(refuse(format!("page: {typed} isn't a web address")));
        };
        let key = copper_core::address::history_key(&url);
        if !selector.pages.contains(&key) {
            selector.pages.push(key);
        }
    }
    if let Some(typed) = strings("device").into_iter().next() {
        if typed.eq_ignore_ascii_case("this") {
            let Some(me) = me else {
                return Err(refuse("device: this device isn't signed in".into()));
            };
            selector.device = Some(me.to_ascii_lowercase());
        } else {
            if !copper_core::ids::is_uuid(&typed) {
                return Err(refuse(format!("device: {typed} isn't a device id")));
            }
            selector.device = Some(typed.to_ascii_lowercase());
        }
    }
    selector.everything = args.get("all").and_then(Value::as_bool) == Some(true);
    if let (Some(s), Some(u)) = (selector.since, selector.until)
        && s >= u
    {
        return Err(refuse("since has to be before until".into()));
    }
    if selector.everything && selector.narrowed() {
        return Err(refuse(
            "all deletes everything — leave out since, until, host, page and device, or leave out all".into(),
        ));
    }
    if !selector.valid() {
        return Err(refuse(
            "Say what to delete — since, until, host, page or device — or all: true for everything"
                .into(),
        ));
    }
    Ok(selector)
}

/// The choice in a few words. A page is said by its site only: an address can
/// carry a token.
pub fn words(selector: &Selector) -> String {
    if selector.everything {
        return "all history".into();
    }
    let mut what = vec![];
    if !selector.hosts.is_empty() {
        what.push(format!("visits to {}", selector.hosts.join(", ")));
    }
    let mut sites: Vec<String> = selector
        .pages
        .iter()
        .map(|p| p.split('/').next().unwrap_or("").to_string())
        .collect();
    sites.sort();
    sites.dedup();
    if let Some(first) = sites.first() {
        let on = if sites.len() == 1 {
            format!(" on {first}")
        } else {
            String::new()
        };
        what.push(if selector.pages.len() == 1 {
            format!("a page{on}")
        } else {
            format!("{} pages{on}", selector.pages.len())
        });
    }
    let mut parts = vec![if what.is_empty() {
        "visits".to_string()
    } else {
        what.join(" and ")
    }];
    match (selector.since, selector.until) {
        (Some(s), Some(u)) => parts.push(format!("between {} and {}", stamp(s), stamp(u))),
        (Some(s), None) => parts.push(format!("since {}", stamp(s))),
        (None, Some(u)) => parts.push(format!("before {}", stamp(u))),
        _ => {}
    }
    if let Some(d) = &selector.device {
        parts.push(format!(
            "from device {}",
            d.chars().take(8).collect::<String>()
        ));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sites_and_times() {
        assert_eq!(site("https://www.X.com/a").as_deref(), Some("x.com"));
        assert_eq!(site("*.x.com").as_deref(), Some("x.com"));
        assert_eq!(site("X.COM.").as_deref(), Some("x.com"));
        assert_eq!(site("localhost").as_deref(), Some("localhost"));
        assert_eq!(site("nodot"), None);
        assert_eq!(site("a b.com"), None);
        assert!(within("www.x.com", "x.com"));
        assert!(!within("notx.com", "x.com"));
        assert!(!within("x.com.evil.org", "x.com"));
        assert_eq!(time("1h", 10_000.0), Some(6_400.0));
        assert_eq!(time("2w", 2_000_000.0), Some(2_000_000.0 - 1_209_600.0));
        assert_eq!(time("2026-09-21T14:13:20Z", 0.0), Some(1_790_000_000.0));
        assert_eq!(time("soon", 0.0), None);
    }

    #[test]
    fn selectors_from_words() {
        let s = selector_from(
            &json!({"host": ["x.com", "www.x.com"], "since": "1h"}),
            10_000.0,
            None,
        )
        .unwrap();
        assert_eq!(s.hosts, vec!["x.com"]);
        assert_eq!(s.since, Some(6_400.0));
        assert!(selector_from(&json!({}), 0.0, None).is_err());
        assert!(selector_from(&json!({"all": true, "host": "x.com"}), 0.0, None).is_err());
        assert!(
            selector_from(&json!({"all": true}), 0.0, None)
                .unwrap()
                .everything
        );
        let s = selector_from(
            &json!({"device": "this"}),
            0.0,
            Some("ABCDEF00-0000-0000-0000-000000000000"),
        )
        .unwrap();
        assert_eq!(
            s.device.as_deref(),
            Some("abcdef00-0000-0000-0000-000000000000")
        );
        assert!(selector_from(&json!({"device": "nope"}), 0.0, None).is_err());
        assert!(selector_from(&json!({"since": "1h", "until": "2h"}), 10_000.0, None).is_err());
        let s = selector_from(&json!({"page": "https://www.example.com/a?b"}), 0.0, None).unwrap();
        assert_eq!(s.pages.len(), 1);
        assert_eq!(words(&s), "a page on example.com");
    }

    #[test]
    fn asks_never_widen_to_everything() {
        let a = seqs(&[1, 2, 3]);
        assert_eq!(a.body.as_deref(), Some(&b"{\"seqs\":[1,2,3]}"[..]));
        assert!(a.query.is_empty());
        let w = window(&Selector {
            since: Some(0.0),
            device: Some("ABC".into()),
            ..Default::default()
        });
        assert_eq!(
            w.query,
            vec![
                ("since", "1970-01-01T00:00:00.000Z".to_string()),
                ("device", "abc".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn scanning_deletes_only_the_matches() {
        let rows: Vec<Value> = (1..=1200)
            .map(|i| {
                let host = if i % 3 == 0 { "www.x.com" } else { "y.org" };
                json!({"seq": i, "device_id": "D", "visited_at": "2026-10-01T12:00:00Z",
                       "payload": {"url": format!("https://{host}/p{i}"), "title": "t"}})
            })
            .collect();
        let mut deleted_seqs: Vec<i64> = vec![];
        let selector = Selector {
            hosts: vec!["x.com".into()],
            ..Default::default()
        };
        let outcome = run(&selector, PAGE_LIMIT, |ask: Ask| {
            let result = if ask.method == "GET" {
                let since: i64 = ask.query[0].1.parse().unwrap();
                let page: Vec<Value> = rows
                    .iter()
                    .filter(|r| r["seq"].as_i64().unwrap() > since)
                    .take(500)
                    .cloned()
                    .collect();
                let next = page
                    .last()
                    .map(|r| r["seq"].as_i64().unwrap())
                    .unwrap_or(since);
                let more = next < 1200;
                serde_json::to_vec(&json!({"entries": page, "next": next, "more": more})).unwrap()
            } else {
                let body: Value = serde_json::from_slice(ask.body.as_ref().unwrap()).unwrap();
                let list: Vec<i64> = body["seqs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap())
                    .collect();
                let n = list.len();
                deleted_seqs.extend(list);
                serde_json::to_vec(&json!({"deleted": n})).unwrap()
            };
            async move { Ok(result) }
        })
        .await
        .unwrap();
        assert_eq!(outcome.deleted, 400);
        assert_eq!(outcome.looked, 1200);
        assert_eq!(outcome.requests, 3 + 1);
        assert!(deleted_seqs.iter().all(|s| s % 3 == 0));
    }

    #[test]
    fn pushes_never_send_back_what_went() {
        let visits = vec![
            Visit {
                key: "x.com/a".into(),
                url: "https://x.com/a".into(),
                title: "".into(),
                at: 50.0,
            },
            Visit {
                key: "x.com/b".into(),
                url: "https://x.com/b".into(),
                title: "".into(),
                at: 150.0,
            },
            Visit {
                key: "y.org".into(),
                url: "https://y.org/".into(),
                title: "".into(),
                at: 160.0,
            },
        ];
        let sel = Selector {
            hosts: vec!["x.com".into()],
            ..Default::default()
        };
        let (through, remote) = push_after(
            &sel,
            Here::Kept,
            &visits,
            Some(100.0),
            &BTreeMap::new(),
            None,
            1_000.0,
        );
        assert_eq!(through, Some(100.0));
        assert_eq!(remote.keys().collect::<Vec<_>>(), vec!["x.com/b"]);
        let (through, remote) = push_after(
            &sel,
            Here::Cleared,
            &visits,
            Some(100.0),
            &remote,
            None,
            1_000.0,
        );
        assert_eq!(through, Some(1_000.0));
        assert!(remote.is_empty());
        let mut forgotten = BTreeMap::new();
        forgotten.insert("x.com/b".to_string(), 200.0);
        assert_eq!(unforgotten(visits, &forgotten).len(), 2);
    }
}
