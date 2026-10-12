//! The ad blocker (port of `Shield.swift`) and the hide-anything stylesheet
//! lookup (`Curtain.css` over `hidden.json`).
//!
//! No settings, no counter: one WebKit content-blocker list, compiled once,
//! enforced inside WebKit's networking before a request is made. The rule
//! list is built exactly as macOS builds it (same domains, same slots, same
//! JSON shape; `tests/golden/mac-rules.json` is the Mac's own output), and
//! it is compiled under the identifier family `office-shield`.
//!
//! - [`rules_json`] — the list, as the Mac builds it.
//! - [`List`] — the JSON plus its cache id `office-shield-<hash>`, so a list
//!   that changes compiles fresh and an unchanged one loads from the store.
//! - [`Shield`] — the per-tab state machine: `protect` (tabs opened before the
//!   list is ready get it on completion), `tune` (on or off for the site a tab
//!   is heading to, at navigation — the Mac's per-site exemption), `apply`
//!   (switched on/off for every open tab).
//! - [`List::exempting`] — an alternative per-site exemption baked into the
//!   list (`ignore-previous-rules` + `if-domain`), for callers that cannot
//!   swap filters at navigation time.
//! - [`curtain`] — `hidden.json` → the user stylesheet for one site.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use copper_engine::{ContentFilter, Engine, EngineError, PageView};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub mod curtain;

/// The rule list's identifier on macOS; compiled ids here start with it.
pub const IDENTIFIER: &str = "office-shield";

/// Third parties whose only job is to watch or to sell. First-party requests
/// are untouched: a site's own scripts are the site. (`Shield.unwanted`)
pub const UNWANTED: [&str; 44] = [
    "doubleclick.net",
    "googlesyndication.com",
    "googleadservices.com",
    "googletagservices.com",
    "google-analytics.com",
    "googletagmanager.com",
    "adservice.google.com",
    "amazon-adsystem.com",
    "adnxs.com",
    "adsrvr.org",
    "criteo.com",
    "criteo.net",
    "taboola.com",
    "outbrain.com",
    "rubiconproject.com",
    "pubmatic.com",
    "openx.net",
    "casalemedia.com",
    "smartadserver.com",
    "sharethrough.com",
    "indexww.com",
    "bidswitch.net",
    "33across.com",
    "teads.tv",
    "moatads.com",
    "adroll.com",
    "scorecardresearch.com",
    "quantserve.com",
    "chartbeat.com",
    "hotjar.com",
    "mouseflow.com",
    "fullstory.com",
    "clarity.ms",
    "mixpanel.com",
    "amplitude.com",
    "segment.com",
    "segment.io",
    "branch.io",
    "appsflyer.com",
    "adjust.com",
    "analytics.tiktok.com",
    "connect.facebook.net",
    "ads-twitter.com",
    "analytics.twitter.com",
];

/// The few slots that are reliably an advertisement and nothing else, kept
/// deliberately short. (`Shield.slots`)
pub const SLOTS: [&str; 9] = [
    ".adsbygoogle",
    "ins.adsbygoogle",
    "[id^=\"google_ads_\"]",
    "[id^=\"div-gpt-ad\"]",
    "[id^=\"taboola-\"]",
    "#taboola-below-article",
    "iframe[src*=\"doubleclick.net\"]",
    "iframe[src*=\"googlesyndication\"]",
    "iframe[src*=\"amazon-adsystem\"]",
];

/// One content-blocker rule, in WebKit's JSON shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rule {
    pub trigger: Trigger,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trigger {
    #[serde(rename = "url-filter")]
    pub url_filter: String,
    #[serde(rename = "load-type", skip_serializing_if = "Option::is_none")]
    pub load_type: Option<Vec<String>>,
    #[serde(rename = "if-domain", skip_serializing_if = "Option::is_none")]
    pub if_domain: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

/// `^https?://([^/]+\.)?<domain with dots escaped>` — any subdomain of it.
pub fn url_filter(domain: &str) -> String {
    format!("^https?://([^/]+\\.)?{}", domain.replace('.', "\\."))
}

/// The rules `Shield.compile()` builds: one third-party block per unwanted
/// domain, then one `css-display-none` for the slots.
pub fn rules() -> Vec<Rule> {
    let mut rules: Vec<Rule> = UNWANTED
        .iter()
        .map(|domain| Rule {
            trigger: Trigger {
                url_filter: url_filter(domain),
                load_type: Some(vec!["third-party".into()]),
                if_domain: None,
            },
            action: Action {
                kind: "block".into(),
                selector: None,
            },
        })
        .collect();
    rules.push(Rule {
        trigger: Trigger {
            url_filter: ".*".into(),
            load_type: None,
            if_domain: None,
        },
        action: Action {
            kind: "css-display-none".into(),
            selector: Some(SLOTS.join(", ")),
        },
    });
    rules
}

/// The list as JSON (compact, keys in source order).
pub fn rules_json() -> String {
    serde_json::to_string(&rules()).expect("rules serialise")
}

/// A host as a content-blocker domain: lowercase ASCII, no trailing dot.
/// WebKit refuses anything else, so others are left out.
fn domain_key(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    ok.then_some(host)
}

/// A compiled-list cache key: `office-shield-<first 16 hex of sha256(json)>`.
pub fn cache_id(json: &str) -> String {
    let digest = Sha256::digest(json.as_bytes());
    let mut id = String::with_capacity(IDENTIFIER.len() + 17);
    id.push_str(IDENTIFIER);
    id.push('-');
    for byte in &digest[..8] {
        let _ = write!(id, "{byte:02x}");
    }
    id
}

/// A rule list ready to compile: its JSON and the id it is stored under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub json: String,
    pub id: String,
}

impl List {
    /// The Mac's list.
    pub fn standard() -> Self {
        let json = rules_json();
        List {
            id: cache_id(&json),
            json,
        }
    }

    /// The Mac's list plus a final `ignore-previous-rules` for pages on
    /// `hosts` (exact hosts, as `shield.paused` keeps them). With no usable
    /// host this is [`List::standard`].
    pub fn exempting<'a>(hosts: impl IntoIterator<Item = &'a str>) -> Self {
        let hosts: BTreeSet<String> = hosts.into_iter().filter_map(domain_key).collect();
        if hosts.is_empty() {
            return Self::standard();
        }
        let mut rules = rules();
        rules.push(Rule {
            trigger: Trigger {
                url_filter: ".*".into(),
                load_type: None,
                if_domain: Some(hosts.into_iter().collect()),
            },
            action: Action {
                kind: "ignore-previous-rules".into(),
                selector: None,
            },
        });
        let json = serde_json::to_string(&rules).expect("rules serialise");
        List {
            id: cache_id(&json),
            json,
        }
    }
}

/// Load `list` from the engine's filter store, or compile and store it when
/// it was never compiled. `previous` (an id compiled by an earlier list) is
/// removed when it differs, so the store keeps one list.
pub async fn compiled<E: Engine>(
    engine: &E,
    list: &List,
    previous: Option<&str>,
) -> Result<E::Filter, EngineError> {
    if let Some(old) = previous.filter(|old| *old != list.id) {
        let _ = engine.remove_content_filter(old).await;
    }
    if let Some(filter) = engine.load_content_filter(&list.id).await? {
        return Ok(filter);
    }
    engine.compile_content_filter(&list.id, &list.json).await
}

/// Per-tab bookkeeping, as `Shield` keeps it on macOS. Main thread only.
pub struct Shield<P: PageView> {
    list: Option<P::Filter>,
    waiting: Vec<P>,
    trouble: Option<String>,
    /// On unless somebody said otherwise.
    pub enabled: bool,
    paused: BTreeSet<String>,
}

impl<P: PageView + Clone> Shield<P> {
    /// `paused` is `shield.paused` from settings.
    pub fn new(enabled: bool, paused: impl IntoIterator<Item = String>) -> Self {
        Shield {
            list: None,
            waiting: Vec::new(),
            trouble: None,
            enabled,
            paused: paused.into_iter().collect(),
        }
    }

    /// The compiled list, once it is ready.
    pub fn list(&self) -> Option<&P::Filter> {
        self.list.as_ref()
    }

    /// Set when compiling failed: nothing is blocked until it is cleared.
    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }

    /// Pages still waiting for the list.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    pub fn is_paused(&self, host: Option<&str>) -> bool {
        host.is_some_and(|h| self.paused.contains(h))
    }

    /// Off (or back on) for one site. Returns the sorted list to store as
    /// `shield.paused`.
    pub fn pause(&mut self, host: &str, off: bool) -> Vec<String> {
        if off {
            self.paused.insert(host.to_string());
        } else {
            self.paused.remove(host);
        }
        self.paused.iter().cloned().collect()
    }

    /// The hosts it is off for, sorted.
    pub fn paused(&self) -> Vec<String> {
        self.paused.iter().cloned().collect()
    }

    /// Whether a page on `host` should carry the list right now.
    pub fn applies_to(&self, host: Option<&str>) -> bool {
        self.enabled && !self.is_paused(host)
    }

    /// The compile finished: keep the list (or the trouble) and hand it to
    /// every page that asked meanwhile.
    pub fn ready(&mut self, result: Result<P::Filter, String>) {
        match result {
            Ok(filter) => {
                self.trouble = None;
                if self.enabled {
                    for page in &self.waiting {
                        page.add_content_filter(&filter);
                    }
                }
                self.waiting.clear();
                self.list = Some(filter);
            }
            Err(why) => {
                self.trouble = Some(if why.is_empty() {
                    "Compiling the block list failed".into()
                } else {
                    why
                });
            }
        }
    }

    /// Every tab asks for it; whoever asks before it is ready is remembered.
    pub fn protect(&mut self, page: &P) {
        match &self.list {
            Some(list) => {
                if self.enabled {
                    page.add_content_filter(list);
                }
            }
            None => self.waiting.push(page.clone()),
        }
    }

    /// Before each page: the list goes on or off for the site this tab is
    /// heading to.
    pub fn tune(&self, page: &P, host: Option<&str>) {
        let Some(list) = &self.list else { return };
        page.remove_content_filter(list.id());
        if self.applies_to(host) {
            page.add_content_filter(list);
        }
    }

    /// Switched on or off for every page that is already open.
    pub fn apply<'a>(&self, pages: impl IntoIterator<Item = &'a P>)
    where
        P: 'a,
    {
        let Some(list) = &self.list else { return };
        for page in pages {
            page.remove_content_filter(list.id());
            if self.enabled {
                page.add_content_filter(list);
            }
        }
    }

    /// A page closed before the list arrived.
    pub fn forget(&mut self, is_it: impl Fn(&P) -> bool) {
        self.waiting.retain(|p| !is_it(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn same_json_as_the_mac() {
        let mac: Value =
            serde_json::from_str(include_str!("../tests/golden/mac-rules.json")).unwrap();
        let ours: Value = serde_json::from_str(&rules_json()).unwrap();
        assert_eq!(ours, mac);
        assert_eq!(ours.as_array().unwrap().len(), 45);
    }

    #[test]
    fn rule_shapes() {
        let r = rules();
        assert_eq!(
            r[0].trigger.url_filter,
            r"^https?://([^/]+\.)?doubleclick\.net"
        );
        assert_eq!(
            r[44].action.selector.as_deref().unwrap(),
            ".adsbygoogle, ins.adsbygoogle, [id^=\"google_ads_\"], [id^=\"div-gpt-ad\"], \
             [id^=\"taboola-\"], #taboola-below-article, iframe[src*=\"doubleclick.net\"], \
             iframe[src*=\"googlesyndication\"], iframe[src*=\"amazon-adsystem\"]"
        );
        let json = rules_json();
        assert!(json.starts_with(r#"[{"trigger":{"url-filter":"^https?://([^/]+\\.)?doubleclick\\.net","load-type":["third-party"]},"action":{"type":"block"}}"#));
    }

    #[test]
    fn cache_ids_follow_the_json() {
        let a = List::standard();
        assert!(a.id.starts_with("office-shield-"));
        assert_eq!(a.id.len(), "office-shield-".len() + 16);
        assert_eq!(a, List::standard());
        let b = List::exempting(["news.example", "Shop.Example."]);
        assert_ne!(a.id, b.id);
        let v: Value = serde_json::from_str(&b.json).unwrap();
        assert_eq!(
            v[45],
            serde_json::json!({"trigger": {"url-filter": ".*", "if-domain": ["news.example", "shop.example"]},
                               "action": {"type": "ignore-previous-rules"}})
        );
        assert_eq!(List::exempting(["bücher.de", ""]), a);
    }
}
