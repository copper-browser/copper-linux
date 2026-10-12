//! `history.json` and the field's completion (Mac: `History.swift`).
//!
//! One [`Visit`] per *place* (keyed by [`crate::address::history_key`]), not
//! per visit: a count and the last time. Visiting a deep page also credits
//! its domain, so typing three letters offers the front door. Ranking is a
//! straight port of the Mac's: match position, then frecency
//! (`count × e^(−days/30)`), then a bonus for bare domains.

use crate::address::{self, strip_typed};
use crate::date::SwiftDate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use url::Url;

/// The file keeps at most this many places, least frecent dropped first.
pub const HISTORY_CAP: usize = 20_000;

/// One place you have been (`Visit`). `last` is a Swift date (2001 epoch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub count: i64,
    pub last: SwiftDate,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Visit {
    /// A domain credit with no title of its own (`example.com`, not a page).
    pub fn is_front_door_credit(&self) -> bool {
        self.title.is_empty() && !self.key.contains('/')
    }

    /// `count × e^(−days/30)`.
    pub fn frecency(&self, now: SwiftDate) -> f64 {
        let days = ((now.0 - self.last.0) / 86_400.0).max(0.0);
        self.count as f64 * (-days / 30.0).exp()
    }
}

/// What a completion row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuggestionKind {
    /// A page open right now.
    Open,
    /// Somewhere you have been.
    Visited,
    /// A well-known address, offered only before you have history there.
    Known,
    /// Words, and an engine to ask.
    Search,
    Bookmark,
    Command,
}

/// A completion row.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// What you would have typed: no scheme, no `www.`.
    pub key: String,
    pub title: String,
    pub url: String,
    pub kind: SuggestionKind,
    /// Set when the page is open in a tab.
    pub tab: Option<crate::browser::TabId>,
    /// The ranking score (higher first).
    pub score: f64,
}

/// A row of the history panel, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub key: String,
    pub title: String,
    pub url: String,
    pub last: SwiftDate,
    pub count: i64,
}

/// Every place you have been, with the Mac's ranking. Mutations bump
/// [`History::generation`]; persist with [`History::to_file`].
#[derive(Debug, Clone, Default)]
pub struct History {
    visits: HashMap<String, Visit>,
    generation: u64,
}

impl History {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// From the file's list. A duplicate key keeps the later entry.
    pub fn from_file(list: Vec<Visit>) -> Self {
        let visits = list.into_iter().map(|v| (v.key.clone(), v)).collect();
        History {
            visits,
            generation: 0,
        }
    }

    /// The list to write: most frecent first, capped at [`HISTORY_CAP`].
    pub fn to_file(&self, now: SwiftDate) -> Vec<Visit> {
        let mut list: Vec<(f64, &Visit)> =
            self.visits.values().map(|v| (v.frecency(now), v)).collect();
        list.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
        list.into_iter()
            .take(HISTORY_CAP)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// Bumped on every change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How many places.
    pub fn len(&self) -> usize {
        self.visits.len()
    }

    /// No places at all.
    pub fn is_empty(&self) -> bool {
        self.visits.is_empty()
    }

    /// The place with this key.
    pub fn get(&self, key: &str) -> Option<&Visit> {
        self.visits.get(key)
    }

    /// Every place, in no particular order.
    pub fn visits(&self) -> impl Iterator<Item = &Visit> {
        self.visits.values()
    }

    /// A visit now. Only `http`/`https`. Returns false when nothing was kept.
    pub fn record(&mut self, url: &str, title: &str) -> bool {
        self.record_at(url, title, SwiftDate::now())
    }

    /// A visit at `now` (for tests and imports with a clock).
    pub fn record_at(&mut self, url: &str, title: &str, now: SwiftDate) -> bool {
        let Ok(parsed) = Url::parse(url) else {
            return false;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            return false;
        }
        let key = address::history_key(&parsed);
        if key.is_empty() {
            return false;
        }
        if let Some(host) = parsed.host_str()
            && key.contains('/')
        {
            let root = host.strip_prefix("www.").unwrap_or(host).to_lowercase();
            let home = self.visits.entry(root.clone()).or_insert_with(|| Visit {
                url: format!("https://{root}/"),
                key: root,
                title: String::new(),
                count: 0,
                last: now,
                extra: Map::new(),
            });
            home.count += 1;
            home.last = now;
        }
        match self.visits.get_mut(&key) {
            Some(seen) => {
                seen.count += 1;
                seen.last = now;
                seen.url = parsed.to_string();
                if !title.is_empty() {
                    seen.title = title.to_string();
                }
            }
            None => {
                self.visits.insert(
                    key.clone(),
                    Visit {
                        url: parsed.to_string(),
                        key,
                        title: title.to_string(),
                        count: 1,
                        last: now,
                        extra: Map::new(),
                    },
                );
            }
        }
        self.generation += 1;
        true
    }

    /// Somewhere another browser has been, counted as it was counted there
    /// (an import; taking it twice does not double the count).
    pub fn take(&mut self, url: &str, title: &str, count: i64, last: SwiftDate) {
        let Ok(parsed) = Url::parse(url) else { return };
        if !matches!(parsed.scheme(), "http" | "https") {
            return;
        }
        let key = address::history_key(&parsed);
        if key.is_empty() {
            return;
        }
        match self.visits.get_mut(&key) {
            Some(seen) => {
                seen.count = seen.count.max(count);
                if last.0 > seen.last.0 {
                    seen.last = last;
                }
                if !title.is_empty() && (seen.title.is_empty() || last.0 >= seen.last.0) {
                    seen.title = title.to_string();
                }
            }
            None => {
                self.visits.insert(
                    key.clone(),
                    Visit {
                        url: parsed.to_string(),
                        key,
                        title: title.to_string(),
                        count,
                        last,
                        extra: Map::new(),
                    },
                );
            }
        }
        self.generation += 1;
    }

    /// The title usually lands a beat after the page.
    pub fn retitle(&mut self, url: &str, title: &str) {
        let Ok(parsed) = Url::parse(url) else { return };
        let key = address::history_key(&parsed);
        if let Some(seen) = self.visits.get_mut(&key)
            && !title.is_empty()
            && seen.title != title
        {
            seen.title = title.to_string();
            self.generation += 1;
        }
    }

    /// Remove one place.
    pub fn forget(&mut self, key: &str) -> bool {
        let gone = self.visits.remove(key).is_some();
        if gone {
            self.generation += 1;
        }
        gone
    }

    /// Remove everything.
    pub fn clear(&mut self) {
        self.visits.clear();
        self.generation += 1;
    }

    /// The history panel: titled places matching `typed` in key or title,
    /// newest first. Domain credits without a title are left out.
    pub fn search(&self, typed: &str) -> Vec<Trace> {
        let needle = typed.trim().to_lowercase();
        let mut out: Vec<&Visit> = self
            .visits
            .values()
            .filter(|v| !v.is_front_door_credit())
            .filter(|v| {
                needle.is_empty()
                    || v.key.contains(&needle)
                    || v.title.to_lowercase().contains(&needle)
            })
            .collect();
        out.sort_by(|a, b| {
            b.last
                .0
                .total_cmp(&a.last.0)
                .then_with(|| a.key.cmp(&b.key))
        });
        out.into_iter().map(trace).collect()
    }

    /// The newest `limit` rows of [`History::search`] with no filter.
    pub fn recent(&self, limit: usize) -> Vec<Trace> {
        let mut all = self.search("");
        all.truncate(limit);
        all
    }

    /// The most frecent places for a new-tab page: titled pages and front
    /// doors alike, one per key, best first.
    pub fn top_sites(&self, limit: usize, now: SwiftDate) -> Vec<Trace> {
        let mut list: Vec<(f64, &Visit)> = self
            .visits
            .values()
            .map(|v| {
                (
                    v.frecency(now) + if v.key.contains('/') { 0.0 } else { 1.5 },
                    v,
                )
            })
            .collect();
        list.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.key.cmp(&b.1.key)));
        list.into_iter()
            .take(limit)
            .map(|(_, v)| trace(v))
            .collect()
    }

    /// Best matches first for what was typed (`History.suggestions`). Empty
    /// text proposes nothing.
    pub fn suggestions(&self, typed: &str, limit: usize) -> Vec<Suggestion> {
        self.suggestions_at(typed, limit, SwiftDate::now())
    }

    /// [`History::suggestions`] with an explicit clock.
    pub fn suggestions_at(&self, typed: &str, limit: usize, now: SwiftDate) -> Vec<Suggestion> {
        let needle = strip_typed(typed);
        if needle.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<Suggestion> = Vec::new();
        for v in self.visits.values() {
            let Some(rank) = rank(&v.key, &needle) else {
                continue;
            };
            if Url::parse(&v.url).is_err() {
                continue;
            }
            let door = if v.key.contains('/') { 0.0 } else { 1.5 };
            scored.push(Suggestion {
                key: v.key.clone(),
                title: v.title.clone(),
                url: v.url.clone(),
                kind: SuggestionKind::Visited,
                tab: None,
                score: rank + 4.0 + v.frecency(now) + door,
            });
        }
        for (key, title) in KNOWN {
            if self.visits.contains_key(*key) {
                continue;
            }
            let Some(rank) = rank(key, &needle) else {
                continue;
            };
            scored.push(Suggestion {
                key: key.to_string(),
                title: title.to_string(),
                url: format!("https://{key}"),
                kind: SuggestionKind::Known,
                tab: None,
                score: rank,
            });
        }
        sort_suggestions(&mut scored);
        scored.truncate(limit);
        scored
    }
}

/// Order rows best first; ties go to the shorter key, then alphabetical.
pub fn sort_suggestions(rows: &mut [Suggestion]) {
    rows.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.key.chars().count().cmp(&b.key.chars().count()))
            .then_with(|| a.key.cmp(&b.key))
    });
}

/// What the field draws greyed after the caret: the rest of the first
/// option whose address (or key) carries on from what was typed
/// (`History.completion`). Needs two characters.
pub fn completion(typed: &str, options: &[Suggestion]) -> Option<String> {
    let lower = strip_typed(typed);
    if lower.chars().count() < 2 {
        return None;
    }
    for hit in options {
        let address = strip_typed(&address::pretty_str(&hit.url));
        if let Some(rest) = address.strip_prefix(&lower)
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
        let key = strip_typed(&hit.key);
        if let Some(rest) = key.strip_prefix(&lower)
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

fn trace(v: &Visit) -> Trace {
    Trace {
        key: v.key.clone(),
        title: v.title.clone(),
        url: v.url.clone(),
        last: v.last,
        count: v.count,
    }
}

/// Where the match falls: the start of the key (6), the start of a host
/// label (4), inside the host from two letters (2); never the path.
fn rank(key: &str, needle: &str) -> Option<f64> {
    if key.starts_with(needle) {
        return Some(6.0);
    }
    let host = key.split('/').next().unwrap_or(key);
    if host.split('.').any(|label| label.starts_with(needle)) {
        return Some(4.0);
    }
    if needle.chars().count() >= 2 && host.contains(needle) {
        return Some(2.0);
    }
    None
}

/// Somewhere to start before there is any history (`History.known`).
pub const KNOWN: &[(&str, &str)] = &[
    ("google.com", "Google"),
    ("mail.google.com", "Gmail"),
    ("drive.google.com", "Google Drive"),
    ("calendar.google.com", "Google Calendar"),
    ("maps.google.com", "Google Maps"),
    ("youtube.com", "YouTube"),
    ("github.com", "GitHub"),
    ("figma.com", "Figma"),
    ("vercel.com", "Vercel"),
    ("notion.so", "Notion"),
    ("linear.app", "Linear"),
    ("slack.com", "Slack"),
    ("discord.com", "Discord"),
    ("x.com", "X"),
    ("linkedin.com", "LinkedIn"),
    ("instagram.com", "Instagram"),
    ("reddit.com", "Reddit"),
    ("news.ycombinator.com", "Hacker News"),
    ("stackoverflow.com", "Stack Overflow"),
    ("claude.ai", "Claude"),
    ("chatgpt.com", "ChatGPT"),
    ("dribbble.com", "Dribbble"),
    ("behance.net", "Behance"),
    ("awwwards.com", "Awwwards"),
    ("mobbin.com", "Mobbin"),
    ("siteinspire.com", "SiteInspire"),
    ("are.na", "Are.na"),
    ("pinterest.com", "Pinterest"),
    ("framer.com", "Framer"),
    ("webflow.com", "Webflow"),
    ("developer.apple.com", "Apple Developer"),
    ("swift.org", "Swift"),
    ("npmjs.com", "npm"),
    ("supabase.com", "Supabase"),
    ("stripe.com", "Stripe"),
    ("shopify.com", "Shopify"),
    ("cloudflare.com", "Cloudflare"),
    ("netlify.com", "Netlify"),
    ("apple.com", "Apple"),
    ("spotify.com", "Spotify"),
    ("netflix.com", "Netflix"),
    ("wikipedia.org", "Wikipedia"),
    ("deepl.com", "DeepL"),
    ("loom.com", "Loom"),
    ("amazon.fr", "Amazon"),
    ("leboncoin.fr", "leboncoin"),
    ("lemonde.fr", "Le Monde"),
];
