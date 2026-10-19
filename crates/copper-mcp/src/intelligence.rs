//! The models Jev mode asks (Fork/Intelligence.swift): TypeSafe's Jev
//! (System One) for typed choices, and the router — an OpenAI-compatible
//! gateway, or the Claude account lane through the host — for free text.
//!
//! Keys: this machine's `intelligence.json` merged with what Copper Cloud
//! provides (`cloud-intelligence.json`), field by field as
//! `Intelligence.merge` does. Never log a key.
//!
//! A headless Copper (an agent's own browser, where the cloud's gateway is
//! out of reach) can name its router in the environment instead:
//! `COPPER_ROUTER_URL` and `COPPER_ROUTER_KEY` (both, or neither counts),
//! and optionally `COPPER_ROUTER_MODEL` for every tier. They beat both
//! files, and the sources say `env` ([`RouterOverride`]).
//!
//! Jev authenticates with `Authorization: Bearer <jevKey>` and a
//! `copper/<version>` user agent, POSTing `{model, state, questions}` to
//! `jevEndpoint` (default `https://api.typesafe.ai/v1/systemone`); the answer
//! is `{answers: {name: {choice, confidence, probabilities} | {noul} |
//! {score}}, usage}`. The router POSTs a chat completion to
//! `<routerURL>/v1/chat/completions` with `Authorization: Bearer <routerKey>`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use copper_core::cloud::CloudProvided;
use copper_core::intelligence::IntelligenceKeys;
use copper_core::{DataFile, Store};
use serde_json::{Map, Value, json};

use crate::host::IntelligenceSource;

/// Where the model comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lane {
    /// An API key for the gateway.
    #[default]
    Key,
    /// The Claude account.
    Claude,
}

/// The three sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Haiku,
    #[default]
    Sonnet,
    Opus,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Haiku => "haiku",
            Tier::Sonnet => "sonnet",
            Tier::Opus => "opus",
        }
    }
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "haiku" => Some(Tier::Haiku),
            "sonnet" => Some(Tier::Sonnet),
            "opus" => Some(Tier::Opus),
            _ => None,
        }
    }
}

pub const DEFAULT_JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
pub const DEFAULT_JEV_MODEL: &str = "jev-latest";
pub const DEFAULT_ROUTER_URL: &str = "https://llm.dev.exowatt.com";

fn default_claude_models() -> BTreeMap<String, String> {
    [
        ("haiku", "claude-haiku-4-5"),
        ("sonnet", "claude-sonnet-5"),
        ("opus", "claude-opus-5-5"),
    ]
    .into_iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect()
}

fn default_router_models() -> BTreeMap<String, String> {
    ["haiku", "sonnet", "opus"]
        .into_iter()
        .map(|t| (t.to_string(), t.to_string()))
        .collect()
}

/// `Intelligence.Keys`, decoded and defaulted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub jev_key: String,
    pub jev_model: String,
    pub jev_endpoint: String,
    pub router_key: String,
    pub router_url: String,
    pub router_model: String,
    pub lane: Lane,
    pub tier: Tier,
    pub claude_models: BTreeMap<String, String>,
    pub router_models: BTreeMap<String, String>,
    /// Jev mode's text writer; empty: the chosen model.
    pub text_model: String,
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            jev_key: String::new(),
            jev_model: DEFAULT_JEV_MODEL.into(),
            jev_endpoint: DEFAULT_JEV_ENDPOINT.into(),
            router_key: String::new(),
            router_url: DEFAULT_ROUTER_URL.into(),
            router_model: "sonnet".into(),
            lane: Lane::Key,
            tier: Tier::Sonnet,
            claude_models: default_claude_models(),
            router_models: default_router_models(),
            text_model: String::new(),
        }
    }
}

impl Keys {
    /// The file's fields with the Mac's defaults and name migration
    /// (`Keys.init(from:)`).
    pub fn from_file(file: &IntelligenceKeys) -> Keys {
        let fresh = Keys::default();
        let mut k = Keys {
            jev_key: file.jev_key.clone().unwrap_or_default(),
            jev_model: file.jev_model.clone().unwrap_or(fresh.jev_model),
            jev_endpoint: file.jev_endpoint.clone().unwrap_or(fresh.jev_endpoint),
            router_key: file.router_key.clone().unwrap_or_default(),
            router_url: file.router_url.clone().unwrap_or(fresh.router_url),
            router_model: file.router_model.clone().unwrap_or(fresh.router_model),
            lane: match file.lane.as_deref() {
                Some("claude") => Lane::Claude,
                _ => Lane::Key,
            },
            tier: file
                .tier
                .as_deref()
                .and_then(Tier::parse)
                .unwrap_or_default(),
            claude_models: fresh.claude_models,
            router_models: fresh.router_models,
            text_model: file.text_model.clone().unwrap_or_default(),
        };
        if let Some(saved) = &file.claude_models {
            k.claude_models
                .extend(saved.iter().map(|(a, b)| (a.clone(), b.clone())));
        }
        if let Some(saved) = &file.router_models {
            k.router_models
                .extend(saved.iter().map(|(a, b)| (a.clone(), b.clone())));
        } else if family(&k.router_model).is_none() {
            k.router_models
                .insert("sonnet".into(), k.router_model.clone());
        }
        k.migrate_model_names();
        k
    }

    fn migrate_model_names(&mut self) {
        for name in self.router_models.values_mut() {
            *name = gateway_name(name);
        }
        for (tier, name) in self.claude_models.iter_mut() {
            *name = claude_name(name, Tier::parse(tier));
        }
        self.router_model = gateway_name(&self.router_model);
        let text = self.text_model.trim();
        if !text.is_empty() {
            self.text_model = match family(text) {
                Some(t) => t.name().to_string(),
                None => unprefixed(text),
            };
        }
    }
}

/// The tier a Claude model name belongs to, however it is spelled.
pub fn family(raw: &str) -> Option<Tier> {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(
            r"^(?:claude-)?(?:\d+(?:[.-]\d+)*-)?(haiku|sonnet|opus)(?:-\d+(?:[.-]\d+)*)?(?:-latest)?$",
        )
        .expect("valid pattern")
    });
    let mut name = unprefixed(raw).to_lowercase();
    if let Some(rest) = name.strip_prefix("anthropic/") {
        name = rest.to_string();
    }
    let caps = pattern.captures(&name)?;
    Tier::parse(caps.get(1)?.as_str())
}

/// Without phi's `exowatt/` provider prefix.
pub fn unprefixed(raw: &str) -> String {
    let name = raw.trim();
    if name.to_lowercase().starts_with("exowatt/") {
        name["exowatt/".len()..].to_string()
    } else {
        name.to_string()
    }
}

/// What the gateway is sent for a name.
pub fn gateway_name(raw: &str) -> String {
    match family(raw) {
        Some(t) => t.name().to_string(),
        None => unprefixed(raw),
    }
}

/// What the Claude-account lane is sent for a name.
pub fn claude_name(raw: &str, tier: Option<Tier>) -> String {
    let name = raw.trim();
    let fam = family(name).or(if name.is_empty() { tier } else { None });
    if let Some(f) = fam
        && let Some(current) = default_claude_models().get(f.name())
    {
        return current.clone();
    }
    unprefixed(name)
}

/// The name a tier is sent as on a lane.
pub fn name_for(tier: Tier, lane: Lane, keys: &Keys) -> String {
    if lane == Lane::Claude {
        return claude_name(
            keys.claude_models
                .get(tier.name())
                .map_or("", String::as_str),
            Some(tier),
        );
    }
    let named = keys.router_models.get(tier.name()).map_or("", |s| s.trim());
    if named.is_empty() {
        tier.name().to_string()
    } else {
        gateway_name(named)
    }
}

/// Resolve a tier name or pass a full model name through
/// (`Intelligence.model`); `keys` are the effective ones.
pub fn model(keys: &Keys, named: Option<&str>) -> String {
    let candidate = named.unwrap_or("").trim();
    if !candidate.is_empty() {
        if let Some(t) = family(candidate) {
            return name_for(t, keys.lane, keys);
        }
        return if keys.lane == Lane::Key {
            unprefixed(candidate)
        } else {
            candidate.to_string()
        };
    }
    name_for(keys.tier, keys.lane, keys)
}

fn web(s: Option<&str>) -> Option<String> {
    let v = s.unwrap_or("").trim();
    let url = url::Url::parse(v).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host().is_some()).then(|| v.to_string())
}

/// This machine's keys merged with Copper Cloud's, and where each came
/// from (`Intelligence.merge`).
pub fn merge(local: &Keys, cloud: Option<&CloudProvided>) -> (Keys, BTreeMap<String, String>) {
    let fresh = Keys::default();
    let mut out = local.clone();
    let mut sources = BTreeMap::new();
    let mut src = |k: &str, v: &str| {
        sources.insert(k.to_string(), v.to_string());
    };
    let trimmed = |s: Option<&str>| s.unwrap_or("").trim().to_string();
    let custom = |value: &str, standard: &str| {
        let v = value.trim();
        !v.is_empty() && v.trim_matches('/') != standard.trim_matches('/')
    };
    let cloud_jev = cloud.and_then(|c| c.jev.as_ref());
    let cloud_router = cloud.and_then(|c| c.router.as_ref());

    // Jev
    let jev_source = if !local.jev_key.trim().is_empty() {
        "local"
    } else if !trimmed(cloud_jev.and_then(|j| j.key.as_deref())).is_empty() {
        out.jev_key = trimmed(cloud_jev.and_then(|j| j.key.as_deref()));
        "cloud"
    } else {
        "none"
    };
    src("jevKey", jev_source);
    if custom(&local.jev_endpoint, &fresh.jev_endpoint) {
        src("jevEndpoint", "local");
    } else if jev_source == "cloud"
        && let Some(endpoint) = web(cloud_jev.and_then(|j| j.endpoint.as_deref()))
    {
        out.jev_endpoint = endpoint;
        src("jevEndpoint", "cloud");
    } else {
        out.jev_endpoint = fresh.jev_endpoint.clone();
        src("jevEndpoint", "default");
    }
    if custom(&local.jev_model, &fresh.jev_model) {
        src("jevModel", "local");
    } else if !trimmed(cloud_jev.and_then(|j| j.model.as_deref())).is_empty() {
        out.jev_model = trimmed(cloud_jev.and_then(|j| j.model.as_deref()));
        src("jevModel", "cloud");
    } else {
        out.jev_model = fresh.jev_model.clone();
        src("jevModel", "default");
    }

    // The router: the cloud's gateway key wins the key, its address and the lane.
    let router_source = if !trimmed(cloud_router.and_then(|r| r.key.as_deref())).is_empty() {
        out.router_key = trimmed(cloud_router.and_then(|r| r.key.as_deref()));
        "cloud"
    } else if !local.router_key.trim().is_empty() {
        "local"
    } else {
        "none"
    };
    src("routerKey", router_source);
    if router_source == "cloud" {
        out.lane = Lane::Key;
        src("lane", "cloud");
    } else {
        src("lane", "local");
    }
    if router_source == "cloud"
        && let Some(url) = web(cloud_router.and_then(|r| r.url.as_deref()))
    {
        out.router_url = url;
        src("routerURL", "cloud");
    } else if custom(&local.router_url, &fresh.router_url) {
        src("routerURL", "local");
    } else {
        out.router_url = fresh.router_url.clone();
        src("routerURL", "default");
    }
    (out, sources)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `intelligence.json` and `cloud-intelligence.json`, read without side
/// effects (an unreadable file reads as absent).
pub fn read_files(store: &Store) -> IntelligenceSource {
    IntelligenceSource {
        local: read_json(&store.path(DataFile::Intelligence)).unwrap_or_default(),
        cloud: read_json(&store.path(DataFile::CloudIntelligence)),
    }
}

pub const ROUTER_URL_ENV: &str = "COPPER_ROUTER_URL";
pub const ROUTER_KEY_ENV: &str = "COPPER_ROUTER_KEY";
pub const ROUTER_MODEL_ENV: &str = "COPPER_ROUTER_MODEL";

/// A router named by the environment: an OpenAI-compatible gateway that
/// beats both `intelligence.json` and the cloud's (for a headless Copper
/// whose cloud gateway is out of reach, such as a loopback shim).
#[derive(Clone, PartialEq, Eq)]
pub struct RouterOverride {
    /// `http(s)://host[:port][/path]`; `/v1/chat/completions` is added.
    pub url: String,
    /// Never log this.
    pub key: String,
    /// Sent for every tier (and as the text writer), when set.
    pub model: Option<String>,
}

impl std::fmt::Debug for RouterOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterOverride")
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl RouterOverride {
    /// From `COPPER_ROUTER_URL`, `COPPER_ROUTER_KEY` and
    /// `COPPER_ROUTER_MODEL`.
    pub fn from_env() -> Option<RouterOverride> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// From any lookup. Both the URL (an http or https address) and the key
    /// must be there; with only one, or a URL that isn't one, nothing is
    /// overridden.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<RouterOverride> {
        let value = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let (url, key) = (value(ROUTER_URL_ENV), value(ROUTER_KEY_ENV));
        let (Some(url), Some(key)) = (url, key) else {
            return None;
        };
        let url = web(Some(&url))?;
        Some(RouterOverride {
            url,
            key,
            model: value(ROUTER_MODEL_ENV),
        })
    }

    /// Put this router in `keys`, and say so in `sources`.
    pub fn apply(&self, keys: &mut Keys, sources: &mut BTreeMap<String, String>) {
        keys.router_url = self.url.clone();
        keys.router_key = self.key.clone();
        keys.lane = Lane::Key;
        for name in ["routerURL", "routerKey", "lane"] {
            sources.insert(name.into(), "env".into());
        }
        if let Some(model) = &self.model {
            for tier in [Tier::Haiku, Tier::Sonnet, Tier::Opus] {
                keys.router_models
                    .insert(tier.name().to_string(), model.clone());
            }
            keys.router_model = model.clone();
            keys.text_model = String::new();
            sources.insert("routerModel".into(), "env".into());
        }
    }
}

/// The effective keys and readiness for one call.
#[derive(Clone, Debug)]
pub struct Effective {
    pub keys: Keys,
    pub sources: BTreeMap<String, String>,
    /// The Claude account is signed in.
    pub claude_ready: bool,
}

impl Effective {
    /// The files merged, then the environment's router
    /// ([`RouterOverride::from_env`]) on top.
    pub fn new(source: &IntelligenceSource, claude_ready: bool) -> Effective {
        Self::with_router(source, claude_ready, RouterOverride::from_env().as_ref())
    }

    /// [`Effective::new`] with the override given rather than read.
    pub fn with_router(
        source: &IntelligenceSource,
        claude_ready: bool,
        router: Option<&RouterOverride>,
    ) -> Effective {
        let local = Keys::from_file(&source.local);
        let (mut keys, mut sources) = merge(&local, source.cloud.as_ref());
        if let Some(router) = router {
            router.apply(&mut keys, &mut sources);
        }
        Effective {
            keys,
            sources,
            claude_ready,
        }
    }

    /// Jev can be asked at all.
    pub fn jev_ready(&self) -> bool {
        !self.keys.jev_key.trim().is_empty()
    }

    /// The gateway can be asked at all.
    pub fn router_ready(&self) -> bool {
        !self.keys.router_key.trim().is_empty() && url::Url::parse(&self.keys.router_url).is_ok()
    }

    /// The active model lane can be asked.
    pub fn model_ready(&self) -> bool {
        match self.keys.lane {
            Lane::Key => self.router_ready(),
            Lane::Claude => self.claude_ready,
        }
    }
}

// MARK: - the network

/// Where model requests run: a tokio runtime (the server's), so the host's
/// own loop never has to be one.
#[derive(Clone)]
pub struct Net {
    runtime: tokio::runtime::Handle,
    client: reqwest::Client,
    user_agent: String,
}

impl Net {
    pub fn new(runtime: tokio::runtime::Handle, version: &str) -> Net {
        Net {
            runtime,
            client: reqwest::Client::new(),
            user_agent: format!("copper/{version}"),
        }
    }

    /// POST JSON and read the whole answer, on the runtime.
    async fn post(
        &self,
        url: String,
        bearer: String,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>), String> {
        let request = self
            .client
            .post(url)
            .timeout(timeout)
            .header("Authorization", format!("Bearer {bearer}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", self.user_agent.clone())
            .body(body);
        let task = self.runtime.spawn(async move {
            let response = request.send().await.map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            Ok::<_, String>((status, bytes.to_vec()))
        });
        task.await.map_err(|e| e.to_string())?
    }
}

/// One answered choice.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub key: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

/// Jev's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answer {
    pub choices: BTreeMap<String, Choice>,
    pub nouls: BTreeMap<String, f64>,
    pub scores: BTreeMap<String, f64>,
    pub latency_ms: f64,
    pub input_tokens: i64,
}

/// A Jev failure: `kind` is `not_configured`, `bad_endpoint`, `transport`,
/// `http_<status>` or `bad_response`.
#[derive(Clone, Debug, PartialEq)]
pub struct JevFailure {
    pub kind: String,
    pub detail: String,
}

impl std::fmt::Display for JevFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{}", self.kind)
        } else {
            write!(f, "{}: {}", self.kind, self.detail)
        }
    }
}

/// Parse Jev's response body (`Jev.ask`).
pub fn parse_answer(body: &[u8], latency_ms: f64) -> Result<Answer, JevFailure> {
    let payload: Value = serde_json::from_slice(body).map_err(|_| JevFailure {
        kind: "bad_response".into(),
        detail: "no answers in body".into(),
    })?;
    let answers = payload
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(JevFailure {
            kind: "bad_response".into(),
            detail: "no answers in body".into(),
        })?;
    let mut out = Answer {
        latency_ms,
        input_tokens: payload["usage"]["input_tokens"].as_i64().unwrap_or(0),
        ..Answer::default()
    };
    for (name, raw) in answers {
        let Some(a) = raw.as_object() else { continue };
        if let Some(picked) = a.get("choice").and_then(Value::as_str) {
            let probabilities = a
                .get("probabilities")
                .and_then(Value::as_object)
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0)))
                        .collect()
                })
                .unwrap_or_default();
            out.choices.insert(
                name.clone(),
                Choice {
                    key: picked.to_string(),
                    confidence: a.get("confidence").and_then(Value::as_f64).unwrap_or(0.0),
                    probabilities,
                },
            );
        } else if let Some(p) = a.get("noul").and_then(Value::as_f64) {
            out.nouls.insert(name.clone(), p);
        } else if let Some(s) = a.get("score").and_then(Value::as_f64) {
            out.scores.insert(name.clone(), s);
        }
    }
    Ok(out)
}

/// One System One call with a hard budget.
pub async fn jev_ask(
    net: &Net,
    keys: &Keys,
    state: &Value,
    questions: &Value,
    timeout: Duration,
) -> Result<Answer, JevFailure> {
    if keys.jev_key.is_empty() {
        return Err(JevFailure {
            kind: "not_configured".into(),
            detail: "No Jev key — Settings › Intelligence".into(),
        });
    }
    if url::Url::parse(&keys.jev_endpoint).is_err() {
        return Err(JevFailure {
            kind: "bad_endpoint".into(),
            detail: keys.jev_endpoint.clone(),
        });
    }
    let body = json!({ "model": keys.jev_model, "state": state, "questions": questions });
    let started = Instant::now();
    let (status, data) = net
        .post(
            keys.jev_endpoint.clone(),
            keys.jev_key.clone(),
            serde_json::to_vec(&body).unwrap_or_default(),
            timeout,
        )
        .await
        .map_err(|detail| JevFailure {
            kind: "transport".into(),
            detail,
        })?;
    let latency = started.elapsed().as_secs_f64() * 1000.0;
    if status != 200 {
        return Err(JevFailure {
            kind: format!("http_{status}"),
            detail: String::from_utf8_lossy(&data[..data.len().min(300)]).into_owned(),
        });
    }
    parse_answer(&data, latency)
}

/// The router's reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub json: Map<String, Value>,
    pub text: String,
    pub latency_ms: f64,
    pub model: String,
}

/// The first JSON object in a reply, fences and chatter ignored.
pub fn json_in(text: &str) -> Map<String, Value> {
    let (Some(open), Some(close)) = (text.find('{'), text.rfind('}')) else {
        return Map::new();
    };
    if open >= close {
        return Map::new();
    }
    match serde_json::from_str::<Value>(&text[open..=close]) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

/// A chat-completions body (`Router.body`), with prompt-cache marks on the
/// system message for a Claude model (`PromptCache.chat`, `.prefix`).
pub fn router_body(model: &str, max_tokens: u32, system: &str, user: &str, marks: bool) -> Value {
    let system_message = if marks && family(model).is_some() && !system.is_empty() {
        json!({ "role": "system", "content": [
            { "type": "text", "text": system, "cache_control": { "type": "ephemeral" } }
        ] })
    } else {
        json!({ "role": "system", "content": system })
    };
    json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [system_message, { "role": "user", "content": user }],
    })
}

fn cache_refused(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&data[..data.len().min(4000)]).to_lowercase();
    text.contains("prompt caching") || text.contains("cache_control") || text.contains("cachepoint")
}

/// The gateway lane (`Router.ask`, `lane == .key`).
pub async fn router_ask(
    net: &Net,
    keys: &Keys,
    model: &str,
    system: &str,
    user: &str,
    timeout: Duration,
    max_tokens: u32,
) -> Result<Reply, String> {
    if keys.router_key.is_empty() {
        return Err("No router key — Settings › Intelligence".into());
    }
    if url::Url::parse(&keys.router_url).is_err() {
        return Err(format!("Bad router address: {}", keys.router_url));
    }
    let url = format!(
        "{}/v1/chat/completions",
        keys.router_url.trim_end_matches('/')
    );
    let started = Instant::now();
    let send = |marks: bool| {
        let body = serde_json::to_vec(&router_body(model, max_tokens, system, user, marks))
            .unwrap_or_default();
        net.post(url.clone(), keys.router_key.clone(), body, timeout)
    };
    let (mut status, mut data) = send(true).await?;
    if status != 200 && cache_refused(&data) {
        (status, data) = send(false).await?;
    }
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    if status != 200 {
        return Err(format!(
            "router {status}: {}",
            String::from_utf8_lossy(&data[..data.len().min(300)])
        ));
    }
    let payload: Value =
        serde_json::from_slice(&data).map_err(|_| "router answered with no choices".to_string())?;
    let message = payload["choices"]
        .get(0)
        .and_then(|c| c.get("message"))
        .ok_or("router answered with no choices")?;
    let text = match &message["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<String>(),
        _ => String::new(),
    };
    Ok(Reply {
        json: json_in(&text),
        text,
        latency_ms,
        model: payload["model"].as_str().unwrap_or(model).to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        for (name, want) in [
            ("opus", Some(Tier::Opus)),
            ("exowatt/opus-5", Some(Tier::Opus)),
            ("claude-opus-5-5", Some(Tier::Opus)),
            ("anthropic/claude-opus-5", Some(Tier::Opus)),
            ("claude-sonnet-4-5-20250929", Some(Tier::Sonnet)),
            ("claude-3-5-haiku-latest", Some(Tier::Haiku)),
            ("gpt-4o-mini", None),
            ("sonnet-1m", None),
            ("my-opus-proxy", None),
            ("", None),
        ] {
            assert_eq!(family(name), want, "{name}");
        }
        assert_eq!(gateway_name("exowatt/luna"), "luna");
        assert_eq!(claude_name("", Some(Tier::Haiku)), "claude-haiku-4-5");
    }

    #[test]
    fn decode_migrates() {
        let file: IntelligenceKeys = serde_json::from_str(r#"{"routerKey":"sk-x","routerModel":"exowatt/sonnet","routerModels":{"opus":"exowatt/opus-5","haiku":"gpt-4o-mini"},"textModel":"exowatt/haiku-4-5","tier":"opus"}"#).unwrap();
        let k = Keys::from_file(&file);
        assert_eq!(k.router_models["opus"], "opus");
        assert_eq!(k.router_models["haiku"], "gpt-4o-mini");
        assert_eq!(k.router_model, "sonnet");
        assert_eq!(k.text_model, "haiku");
        assert_eq!(k.tier, Tier::Opus);
        assert_eq!(name_for(Tier::Opus, Lane::Claude, &k), "claude-opus-5-5");
        let legacy: IntelligenceKeys = serde_json::from_str(r#"{"routerModel":"gpt-4o"}"#).unwrap();
        assert_eq!(Keys::from_file(&legacy).router_models["sonnet"], "gpt-4o");
    }

    #[test]
    fn merge_rules() {
        let cloud: CloudProvided = serde_json::from_str(r#"{"jev":{"key":"ts-cloud","endpoint":"https://jev.cloud.example/v1","model":"jev-cloud"},"router":{"key":"sk-cloud","url":"https://gw.cloud.example"}}"#).unwrap();
        let (m, s) = merge(&Keys::default(), Some(&cloud));
        assert_eq!(m.jev_key, "ts-cloud");
        assert_eq!(m.jev_endpoint, "https://jev.cloud.example/v1");
        assert_eq!(m.router_url, "https://gw.cloud.example");
        assert_eq!(s["lane"], "cloud");
        let mut local = Keys {
            jev_key: "ts-mine".into(),
            lane: Lane::Claude,
            ..Keys::default()
        };
        let (m, s) = merge(&local, Some(&cloud));
        assert_eq!(m.jev_key, "ts-mine");
        assert_eq!(s["jevEndpoint"], "default");
        assert_eq!(m.lane, Lane::Key);
        local.router_url = "https://llm.dev.exowatt.com/".into();
        let (_, s) = merge(&local, None);
        assert_eq!(s["routerURL"], "default");
        assert_eq!(s["jevKey"], "local");
    }

    #[test]
    fn the_environment_beats_the_cloud_router() {
        let cloud: CloudProvided = serde_json::from_str(r#"{"jev":{"key":"ts-cloud"},"router":{"key":"sk-cloud","url":"https://gw.cloud.example"}}"#).unwrap();
        let source = IntelligenceSource {
            local: serde_json::from_str(
                r#"{"routerKey":"sk-mine","lane":"claude","textModel":"gpt-4o"}"#,
            )
            .unwrap(),
            cloud: Some(cloud),
        };
        let vars = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.to_string())
            }
        };
        // Unset (or half set, or not an address): exactly as before.
        let before = Effective::with_router(&source, false, None);
        assert_eq!(before.keys.router_key, "sk-cloud");
        assert_eq!(before.sources["routerKey"], "cloud");
        for half in [
            &[(ROUTER_URL_ENV, "http://127.0.0.1:8099")][..],
            &[(ROUTER_KEY_ENV, "sk-shim")][..],
            &[(ROUTER_URL_ENV, "not a url"), (ROUTER_KEY_ENV, "sk-shim")][..],
            &[(ROUTER_URL_ENV, " "), (ROUTER_KEY_ENV, "sk-shim")][..],
        ] {
            assert_eq!(RouterOverride::from_vars(vars(half)), None, "{half:?}");
        }
        // Both: the environment wins the key, the address and the lane; the
        // Jev key still comes from the cloud.
        let env = RouterOverride::from_vars(vars(&[
            (ROUTER_URL_ENV, " http://127.0.0.1:8099 "),
            (ROUTER_KEY_ENV, "sk-shim"),
        ]))
        .unwrap();
        assert!(
            !format!("{env:?}").contains("sk-shim"),
            "the key is never printed"
        );
        let e = Effective::with_router(&source, false, Some(&env));
        assert_eq!(e.keys.router_url, "http://127.0.0.1:8099");
        assert_eq!(e.keys.router_key, "sk-shim");
        assert_eq!(e.keys.lane, Lane::Key);
        assert_eq!(e.sources["routerKey"], "env");
        assert_eq!(e.sources["routerURL"], "env");
        assert_eq!(e.sources["lane"], "env");
        assert!(!e.sources.contains_key("routerModel"));
        assert_eq!(e.keys.jev_key, "ts-cloud");
        assert_eq!(e.sources["jevKey"], "cloud");
        assert_eq!(model(&e.keys, Some("opus")), "opus");
        assert!(e.router_ready() && e.model_ready());
        // A model for every tier, and for the text writer.
        let env = RouterOverride::from_vars(vars(&[
            (ROUTER_URL_ENV, "http://127.0.0.1:8099/"),
            (ROUTER_KEY_ENV, "sk-shim"),
            (ROUTER_MODEL_ENV, "us.anthropic.claude-sonnet-5"),
        ]))
        .unwrap();
        let e = Effective::with_router(&source, false, Some(&env));
        assert_eq!(e.sources["routerModel"], "env");
        for tier in ["haiku", "sonnet", "opus"] {
            assert_eq!(model(&e.keys, Some(tier)), "us.anthropic.claude-sonnet-5");
        }
        assert_eq!(model(&e.keys, None), "us.anthropic.claude-sonnet-5");
        assert_eq!(e.keys.text_model, "");
    }

    #[test]
    fn answers_parse() {
        let body = br#"{"answers":{"operation":{"choice":"CLICK","confidence":0.9,"probabilities":{"CLICK":0.9,"DONE":0.1}},"p":{"noul":0.3}},"usage":{"input_tokens":12}}"#;
        let a = parse_answer(body, 5.0).unwrap();
        assert_eq!(a.choices["operation"].key, "CLICK");
        assert_eq!(a.nouls["p"], 0.3);
        assert_eq!(a.input_tokens, 12);
        assert_eq!(parse_answer(b"{}", 1.0).unwrap_err().kind, "bad_response");
    }

    #[test]
    fn json_in_text() {
        assert_eq!(json_in("Sure: ```{\"text\": \"x\"}```")["text"], "x");
        assert!(json_in("nothing").is_empty());
        let body = router_body("sonnet", 10, "sys", "u", true);
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        let body = router_body("gpt-4o", 10, "sys", "u", true);
        assert_eq!(body["messages"][0]["content"], "sys");
    }
}
