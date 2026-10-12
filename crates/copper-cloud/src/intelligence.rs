//! Copper Cloud-provided model keys: `GET /v1/intelligence` on the linked
//! instance (Mac: `CloudIntelligence.swift`). What it answers is kept in
//! memory and in `cloud-intelligence.json` (0600) so an offline launch still
//! has them; it is never written into `intelligence.json`. A cache is only
//! good for the link and account that fetched it (`owner`). Signed out or
//! unlinked, it goes.
//!
//! Asked after a sign-in, at launch when linked and signed in, when settings
//! open, and every 30 minutes. A 404 (a cloud from before this route), 401
//! or 403 means "nothing provided"; a network failure keeps what is known.

use crate::client::CloudClient;
use copper_core::DataFile;
use copper_core::cloud::{CloudAgent, CloudJev, CloudProvided, CloudRouter};
use copper_core::date::IsoDate;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How often to ask again.
pub const INTERVAL: Duration = Duration::from_secs(30 * 60);

/// The server's body, read leniently: any field may be null, absent or the
/// wrong type, and is then simply not provided. `None` only when the body is
/// not a JSON object at all.
pub fn parse(data: &[u8]) -> Option<CloudProvided> {
    let object: Value = serde_json::from_slice(data).ok()?;
    let object = object.as_object()?;
    let text = |v: Option<&Value>| -> Option<String> {
        let t = v?.as_str()?.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    let mut out = CloudProvided::default();
    if let Some(jev) = object.get("jev").and_then(Value::as_object) {
        let part = CloudJev {
            key: text(jev.get("key")),
            endpoint: text(jev.get("endpoint")),
            model: text(jev.get("model")),
            extra: Default::default(),
        };
        if part.key.is_some() || part.endpoint.is_some() || part.model.is_some() {
            out.jev = Some(part);
        }
    }
    if let Some(router) = object.get("router").and_then(Value::as_object) {
        let part = CloudRouter {
            key: text(router.get("key")),
            url: text(router.get("url")),
            extra: Default::default(),
        };
        if part.key.is_some() || part.url.is_some() {
            out.router = Some(part);
        }
    }
    // A whole number in 1…500 only: not a fraction, a boolean or a string.
    if let Some(n) = object
        .get("agent")
        .and_then(|a| a.get("max_turns"))
        .and_then(|v| match v {
            Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
            _ => None,
        })
        .filter(|n| (1..=500).contains(n))
    {
        out.agent = Some(CloudAgent {
            max_turns: Some(n),
            extra: Default::default(),
        });
    }
    out.updated_at = text(object.get("updated_at"));
    Some(out)
}

/// The parts that carry a key: `jev`, `router`. Never the key itself.
pub fn provides(p: &CloudProvided) -> Vec<&'static str> {
    let has = |k: &Option<String>| k.as_deref().is_some_and(|k| !k.trim().is_empty());
    let mut out = vec![];
    if p.jev.as_ref().is_some_and(|j| has(&j.key)) {
        out.push("jev");
    }
    if p.router.as_ref().is_some_and(|r| has(&r.key)) {
        out.push("router");
    }
    out
}

/// Nothing worth keeping.
pub fn is_empty(p: &CloudProvided) -> bool {
    let blank = |k: &Option<String>| k.as_deref().is_none_or(str::is_empty);
    provides(p).is_empty()
        && p.jev
            .as_ref()
            .is_none_or(|j| blank(&j.endpoint) && blank(&j.model))
        && p.router.as_ref().is_none_or(|r| blank(&r.url))
        && p.agent_max_turns().is_none()
}

/// Partial-key display: the first 3 and last 4 characters.
pub fn masked(key: Option<&str>) -> String {
    let k = key.unwrap_or("").trim();
    let n = k.chars().count();
    if n <= 10 {
        return if k.is_empty() {
            String::new()
        } else {
            "••••".into()
        };
    }
    let head: String = k.chars().take(3).collect();
    let tail: String = k.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

#[derive(Default)]
struct IState {
    current: Option<String>,
    last_asked: Option<Instant>,
    last_outcome: String,
    offline: bool,
}

/// The provided keys for the linked, signed-in account.
#[derive(Clone)]
pub struct CloudIntelligence {
    client: CloudClient,
    state: Arc<Mutex<IState>>,
    provided: Arc<watch::Sender<Option<CloudProvided>>>,
}

impl std::fmt::Debug for CloudIntelligence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudIntelligence")
            .field("outcome", &self.last_outcome())
            .finish_non_exhaustive()
    }
}

impl CloudIntelligence {
    /// Read the cache (only if it is this link's and account's).
    pub fn new(client: CloudClient) -> Self {
        let (tx, _) = watch::channel(None);
        let me = CloudIntelligence {
            client,
            state: Arc::new(Mutex::new(IState {
                last_outcome: "never".into(),
                ..Default::default()
            })),
            provided: Arc::new(tx),
        };
        match me.identity() {
            None => me.forget(),
            Some(id) => {
                me.with(|s| s.current = Some(id.clone()));
                match me.read_cache() {
                    Some(cached) if cached.owner.as_deref() == Some(id.as_str()) => {
                        me.provided.send_replace(Some(cached));
                    }
                    _ => me.remove_cache(),
                }
            }
        }
        me
    }

    fn with<R>(&self, f: impl FnOnce(&mut IState) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// `link URL|user id` while linked and signed in.
    pub fn identity(&self) -> Option<String> {
        let link = self.client.link()?;
        let account = self.client.account()?;
        Some(format!(
            "{}|{}",
            link.endpoint.base(),
            account.user_id.to_ascii_lowercase()
        ))
    }

    /// What the cloud provides now, if anything.
    pub fn provided(&self) -> Option<CloudProvided> {
        self.provided.borrow().clone()
    }

    /// Every change to [`CloudIntelligence::provided`].
    pub fn watch(&self) -> watch::Receiver<Option<CloudProvided>> {
        self.provided.subscribe()
    }

    /// `never`, `ok`, `ok (nothing provided)`, `none (404)`, `offline`, …
    pub fn last_outcome(&self) -> String {
        self.with(|s| s.last_outcome.clone())
    }

    /// Ask now (and every [`INTERVAL`]), and follow sign-ins and sign-outs.
    /// Call once, inside a tokio runtime.
    pub fn start(&self) {
        let me = self.clone();
        tokio::spawn(async move {
            me.fetch().await;
            let mut status = me.client.watch_status();
            let mut tick = tokio::time::interval(INTERVAL);
            tick.tick().await;
            loop {
                tokio::select! {
                    changed = status.changed() => {
                        if changed.is_err() { return; }
                        me.cloud_changed().await;
                    }
                    _ = tick.tick() => {
                        me.fetch().await;
                    }
                }
            }
        });
    }

    /// Settings opened: ask again unless asked within a minute.
    pub async fn settings_opened(&self) {
        if self.with(|s| {
            s.last_asked
                .is_some_and(|t| t.elapsed() < Duration::from_secs(60))
        }) {
            return;
        }
        self.fetch().await;
    }

    async fn cloud_changed(&self) {
        let Some(id) = self.identity() else {
            if self.with(|s| s.current.take()).is_some() || self.provided().is_some() {
                self.forget();
            }
            return;
        };
        let current = self.with(|s| s.current.clone());
        if current.as_deref() != Some(id.as_str()) {
            if current.is_some() || self.provided().and_then(|p| p.owner) != Some(id.clone()) {
                self.forget();
            }
            self.with(|s| s.current = Some(id));
            self.fetch().await;
        } else if self.with(|s| s.offline) && self.client.status().reachable {
            self.fetch().await;
        }
    }

    /// `GET /v1/intelligence` once.
    pub async fn fetch(&self) -> Option<CloudProvided> {
        let id = self.identity()?;
        let host = self.client.link()?.host();
        self.with(|s| s.last_asked = Some(Instant::now()));
        match self
            .client
            .request("GET", "/v1/intelligence", None, &[])
            .await
        {
            Ok(answer) => {
                if self.identity().as_deref() != Some(id.as_str()) {
                    return None;
                }
                let mut p = parse(&answer.body).unwrap_or_default();
                p.host = Some(host);
                p.owner = Some(id);
                p.fetched_at = Some(IsoDate::now());
                self.with(|s| s.offline = false);
                if is_empty(&p) {
                    self.with(|s| s.last_outcome = "ok (nothing provided)".into());
                    self.provided.send_replace(None);
                    self.remove_cache();
                    None
                } else {
                    self.with(|s| s.last_outcome = "ok".into());
                    self.write_cache(&p);
                    self.provided.send_replace(Some(p.clone()));
                    Some(p)
                }
            }
            Err(f) => {
                if self.identity().as_deref() != Some(id.as_str()) {
                    return None;
                }
                match f.status {
                    404 | 401 | 403 => {
                        self.with(|s| {
                            s.offline = false;
                            s.last_outcome = format!("none ({})", f.status);
                        });
                        self.provided.send_replace(None);
                        self.remove_cache();
                    }
                    0 => self.with(|s| {
                        s.offline = true;
                        s.last_outcome = "offline".into();
                    }),
                    n => self.with(|s| s.last_outcome = format!("error ({n})")),
                }
                self.provided()
            }
        }
    }

    /// Signed out or unlinked: the keys go from memory and disk.
    pub fn forget(&self) {
        self.provided.send_replace(None);
        self.remove_cache();
        self.with(|s| {
            s.last_outcome = "never".into();
            s.last_asked = None;
            s.offline = false;
        });
    }

    fn read_cache(&self) -> Option<CloudProvided> {
        self.client
            .store()
            .load_opt::<CloudProvided>()
            .ok()
            .and_then(|l| l.value)
    }

    fn write_cache(&self, p: &CloudProvided) {
        if let Err(e) = self.client.store().save(p) {
            tracing::warn!(error = %e, "cloud-intelligence.json not written");
        }
    }

    fn remove_cache(&self) {
        let path = self.client.store().path(DataFile::CloudIntelligence);
        let _ = std::fs::remove_file(&path);
        let mut bak = path.into_os_string();
        bak.push(".bak");
        let _ = std::fs::remove_file(bak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lenient_parse() {
        let p = parse(br#"{"jev":{"key":" jk_123 ","endpoint":null,"model":7},"router":{"key":"","url":"https://r"},"agent":{"max_turns":40},"updated_at":"2026-10-01T00:00:00Z"}"#).unwrap();
        assert_eq!(p.jev.as_ref().unwrap().key.as_deref(), Some("jk_123"));
        assert_eq!(p.jev.as_ref().unwrap().model, None);
        assert_eq!(p.router.as_ref().unwrap().key, None);
        assert_eq!(p.agent_max_turns(), Some(40));
        assert_eq!(provides(&p), vec!["jev"]);
        assert!(!is_empty(&p));
        for bad in [
            r#"{"agent":{"max_turns":true}}"#,
            r#"{"agent":{"max_turns":0}}"#,
            r#"{"agent":{"max_turns":2.5}}"#,
            r#"{"agent":{"max_turns":"9"}}"#,
        ] {
            let p = parse(bad.as_bytes()).unwrap();
            assert!(p.agent.is_none(), "{bad}");
            assert!(is_empty(&p));
        }
        assert!(parse(b"[1]").is_none());
        assert!(parse(b"{}").map(|p| is_empty(&p)).unwrap());
    }

    #[test]
    fn masking() {
        assert_eq!(masked(Some("sk-abcdefghijkl")), "sk-…ijkl");
        assert_eq!(masked(Some("short")), "••••");
        assert_eq!(masked(None), "");
    }
}
