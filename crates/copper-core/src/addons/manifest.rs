//! `manifest.json`, schema `copper-addon/1` (contract §2), and the host
//! allowlist rules addons declare for `copper.fetch` and
//! `copper.oauth.authorize` (§2, §5.2).
//!
//! [`Manifest::validate`] applies the same rules as the cloud and the Mac;
//! each failure carries a [`ManifestErrorCode`]. Unknown top-level keys are
//! kept (`extra`) and ignored.

use super::Platform;
use super::bundle::valid_path;
use super::slot::SlotId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use url::Url;

/// The only manifest schema this build reads.
pub const SCHEMA: &str = "copper-addon/1";
/// At most this many `permissions.hosts`.
pub const MAX_HOSTS: usize = 32;
/// `name` length, in characters.
pub const MAX_NAME: usize = 60;
/// `description` length, in characters.
pub const MAX_DESCRIPTION: usize = 280;

/// What an addon may ask Copper for (`permissions.capabilities`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    Identity,
    Storage,
    Fetch,
    Oauth,
    OpenUrl,
    Page,
    Agent,
}

impl Capability {
    /// Every capability.
    pub const ALL: &'static [Capability] = &[
        Capability::Identity,
        Capability::Storage,
        Capability::Fetch,
        Capability::Oauth,
        Capability::OpenUrl,
        Capability::Page,
        Capability::Agent,
    ];

    /// The manifest spelling (`"open-url"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Identity => "identity",
            Capability::Storage => "storage",
            Capability::Fetch => "fetch",
            Capability::Oauth => "oauth",
            Capability::OpenUrl => "open-url",
            Capability::Page => "page",
            Capability::Agent => "agent",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Capability::ALL
            .iter()
            .copied()
            .find(|c| c.as_str() == s)
            .ok_or(())
    }
}

impl Serialize for Capability {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Which manifest rule failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ManifestErrorCode {
    /// Not a JSON object, or a field has the wrong JSON type.
    Json,
    Schema,
    Id,
    Name,
    Version,
    Description,
    Slot,
    Entry,
    Bridge,
    Hosts,
    Capabilities,
    Settings,
    Platforms,
}

impl ManifestErrorCode {
    /// `manifest_<rule>`-style short code (`"id"`, `"hosts"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            ManifestErrorCode::Json => "json",
            ManifestErrorCode::Schema => "schema",
            ManifestErrorCode::Id => "id",
            ManifestErrorCode::Name => "name",
            ManifestErrorCode::Version => "version",
            ManifestErrorCode::Description => "description",
            ManifestErrorCode::Slot => "slot",
            ManifestErrorCode::Entry => "entry",
            ManifestErrorCode::Bridge => "bridge",
            ManifestErrorCode::Hosts => "hosts",
            ManifestErrorCode::Capabilities => "capabilities",
            ManifestErrorCode::Settings => "settings",
            ManifestErrorCode::Platforms => "platforms",
        }
    }
}

/// One failed manifest rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("manifest {}: {message}", code.as_str())]
pub struct ManifestError {
    pub code: ManifestErrorCode,
    pub message: String,
}

impl ManifestError {
    fn new(code: ManifestErrorCode, message: impl Into<String>) -> Self {
        ManifestError {
            code,
            message: message.into(),
        }
    }
}

/// `permissions`: hosts and capabilities, as written (validated by
/// [`Manifest::validate`], read through [`Permissions::capabilities`] and
/// [`Permissions::host_allow`]).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Permissions {
    /// The known capabilities granted (unknown names dropped).
    pub fn capabilities(&self) -> Vec<Capability> {
        let mut out: Vec<Capability> = self
            .capabilities
            .iter()
            .filter_map(|c| c.parse().ok())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether `cap` is granted.
    pub fn grants(&self, cap: Capability) -> bool {
        self.capabilities.iter().any(|c| c == cap.as_str())
    }

    /// The host rules that parse (bad ones dropped: a validated manifest has
    /// none).
    pub fn host_allow(&self) -> HostAllow {
        HostAllow {
            rules: self
                .hosts
                .iter()
                .filter_map(|h| HostRule::parse(h).ok())
                .collect(),
        }
    }
}

/// One `settings` entry.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SettingSpec {
    /// `string`, `number`, `boolean`, `object` or `array`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The setting types.
pub const SETTING_TYPES: &[&str] = &["string", "number", "boolean", "object", "array"];

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub slot: String,
    pub entry: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Bridge version the addon was written for (≥ 1). Absent reads as 0,
    /// which fails validation.
    #[serde(default)]
    pub bridge: u32,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, SettingSpec>,
    /// Absent (or empty) = every platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Manifest {
    /// Parse `manifest.json` bytes (not validated).
    pub fn from_slice(bytes: &[u8]) -> Result<Manifest, ManifestError> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| ManifestError::new(ManifestErrorCode::Json, e.to_string()))?;
        Manifest::from_value(value)
    }

    /// Read a manifest from JSON (not validated).
    pub fn from_value(value: Value) -> Result<Manifest, ManifestError> {
        if !value.is_object() {
            return Err(ManifestError::new(
                ManifestErrorCode::Json,
                "the manifest is not a JSON object",
            ));
        }
        serde_json::from_value(value)
            .map_err(|e| ManifestError::new(ManifestErrorCode::Json, e.to_string()))
    }

    /// The slot, if this build knows it.
    pub fn slot_id(&self) -> Option<SlotId> {
        self.slot.parse().ok()
    }

    /// Whether the addon is meant for `platform` (absent or empty = all).
    pub fn runs_on(&self, platform: Platform) -> bool {
        runs_on(self.platforms.as_deref(), platform)
    }

    /// The settings with their manifest defaults (`{key: default}`; keys
    /// without a default are left out).
    pub fn default_settings(&self) -> Value {
        Value::Object(
            self.settings
                .iter()
                .filter_map(|(k, s)| s.default.clone().map(|d| (k.clone(), d)))
                .collect(),
        )
    }

    /// The first rule this manifest breaks, if any.
    pub fn validate(&self) -> Result<(), ManifestError> {
        match self.problems().into_iter().next() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Every rule this manifest breaks, in field order.
    pub fn problems(&self) -> Vec<ManifestError> {
        use ManifestErrorCode as C;
        let mut out = Vec::new();
        let mut fail = |code, message: String| out.push(ManifestError::new(code, message));
        if self.schema != SCHEMA {
            fail(
                C::Schema,
                format!("schema must be {SCHEMA:?}, not {:?}", self.schema),
            );
        }
        if !valid_id(&self.id) {
            fail(
                C::Id,
                format!(
                    "id {:?} must be 3–64 of a-z 0-9 . - with a vendor prefix (a dot), no \"..\"",
                    self.id
                ),
            );
        }
        let name = self.name.chars().count();
        if self.name.trim().is_empty() || name > MAX_NAME {
            fail(C::Name, format!("name must be 1–{MAX_NAME} characters"));
        }
        if !valid_version(&self.version) {
            fail(
                C::Version,
                format!(
                    "version {:?} must be MAJOR.MINOR.PATCH (optional -pre)",
                    self.version
                ),
            );
        }
        if self
            .description
            .as_deref()
            .is_some_and(|d| d.chars().count() > MAX_DESCRIPTION)
        {
            fail(
                C::Description,
                format!("description must be at most {MAX_DESCRIPTION} characters"),
            );
        }
        if self.slot_id().is_none() {
            fail(C::Slot, format!("unknown slot {:?}", self.slot));
        }
        if !valid_path(&self.entry) || !self.entry.ends_with(".html") {
            fail(
                C::Entry,
                format!("entry {:?} must be a bundle path ending .html", self.entry),
            );
        }
        if self.bridge < 1 {
            fail(C::Bridge, "bridge must be an integer ≥ 1".into());
        }
        if self.permissions.hosts.len() > MAX_HOSTS {
            fail(C::Hosts, format!("at most {MAX_HOSTS} hosts"));
        }
        for host in &self.permissions.hosts {
            if let Err(why) = HostRule::parse(host) {
                fail(C::Hosts, format!("host {host:?}: {why}"));
            }
        }
        for cap in &self.permissions.capabilities {
            if cap.parse::<Capability>().is_err() {
                fail(C::Capabilities, format!("unknown capability {cap:?}"));
            }
        }
        for (key, spec) in &self.settings {
            if key.is_empty() {
                fail(C::Settings, "a setting key is empty".into());
            }
            if !SETTING_TYPES.contains(&spec.kind.as_str()) {
                fail(
                    C::Settings,
                    format!(
                        "setting {key:?} has type {:?}; use one of {}",
                        spec.kind,
                        SETTING_TYPES.join(", ")
                    ),
                );
            }
        }
        for p in self.platforms.iter().flatten() {
            if p.parse::<Platform>().is_err() {
                fail(
                    C::Platforms,
                    format!("unknown platform {p:?} (mac, linux, windows)"),
                );
            }
        }
        out
    }
}

/// `platforms` absent or empty = every platform.
pub(crate) fn runs_on(platforms: Option<&[String]>, platform: Platform) -> bool {
    match platforms {
        None => true,
        Some([]) => true,
        Some(list) => list.iter().any(|p| p == platform.as_str()),
    }
}

/// `^[a-z0-9][a-z0-9.-]{2,63}$`, no `..`, at least one `.`.
pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    (3..=64).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'.' || *c == b'-')
        && !id.contains("..")
        && id.contains('.')
}

/// `MAJOR.MINOR.PATCH` with an optional `-pre` (`[0-9A-Za-z.-]+`); numbers
/// without leading zeros.
pub fn valid_version(v: &str) -> bool {
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let number = |p: &&str| {
        !p.is_empty()
            && p.len() <= 9
            && p.bytes().all(|c| c.is_ascii_digit())
            && (p.len() == 1 || !p.starts_with('0'))
    };
    parts.len() == 3
        && parts.iter().all(number)
        && pre.is_none_or(|p| {
            !p.is_empty()
                && p.split('.').all(|s| !s.is_empty())
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
        })
}

/// One `permissions.hosts` entry: `host`, `*.host`, either with an
/// optional `:port`, and optionally an explicit `https://` (or, for
/// `127.0.0.1` / `localhost` only, `http://`) in front.
///
/// Matching ([`HostRule::matches`]): a plain host matches exactly that host;
/// `*.example.com` matches any subdomain (`a.example.com`,
/// `a.b.example.com`) but not `example.com` itself; no port means any port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRule {
    /// `*.` was given.
    pub wildcard: bool,
    /// Lowercase, without `*.` and port.
    pub host: String,
    pub port: Option<u16>,
    /// `http` / `https` when written explicitly.
    pub scheme: Option<String>,
}

impl HostRule {
    /// Parse one entry. The error says why in words.
    pub fn parse(text: &str) -> Result<HostRule, String> {
        let mut rest = text;
        let mut scheme = None;
        for s in ["https://", "http://"] {
            if let Some(r) = rest.strip_prefix(s) {
                scheme = Some(s.trim_end_matches("://").to_string());
                rest = r;
            }
        }
        let (wildcard, rest) = match rest.strip_prefix("*.") {
            Some(r) => (true, r),
            None => (false, rest),
        };
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => {
                let port = p
                    .parse::<u16>()
                    .ok()
                    .filter(|n| *n > 0 && p.bytes().all(|c| c.is_ascii_digit()))
                    .ok_or_else(|| format!("bad port {p:?}"))?;
                (h, Some(port))
            }
            None => (rest, None),
        };
        if host != host.to_ascii_lowercase() {
            return Err("hosts are written in lowercase".into());
        }
        if !valid_hostname(host) {
            return Err("not a hostname".into());
        }
        if wildcard && !host.contains('.') {
            return Err("a wildcard needs at least two labels after \"*.\"".into());
        }
        if scheme.as_deref() == Some("http") && (wildcard || !is_loopback_host(host)) {
            return Err("http:// is only for 127.0.0.1 and localhost".into());
        }
        Ok(HostRule {
            wildcard,
            host: host.to_string(),
            port,
            scheme,
        })
    }

    /// Whether `url`'s host (and port) fall under this rule. The scheme rule
    /// (https, or http for loopback) is [`HostAllow::allows`]'s.
    pub fn matches(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let host = host.to_ascii_lowercase();
        let host_ok = if self.wildcard {
            host.len() > self.host.len() + 1
                && host.ends_with(&self.host)
                && host.as_bytes()[host.len() - self.host.len() - 1] == b'.'
        } else {
            host == self.host
        };
        let port_ok = match self.port {
            None => true,
            Some(p) => url.port_or_known_default() == Some(p),
        };
        let scheme_ok = self.scheme.as_deref().is_none_or(|s| s == url.scheme());
        host_ok && port_ok && scheme_ok
    }
}

/// `127.0.0.1` or `localhost`.
pub fn is_loopback_host(host: &str) -> bool {
    host == "127.0.0.1" || host.eq_ignore_ascii_case("localhost")
}

/// Lowercase-or-not DNS name (labels of `a-z0-9-`, 1–63, not starting or
/// ending with `-`; ≤ 253 total) or a dotted IPv4 address.
fn valid_hostname(host: &str) -> bool {
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    host.split('.').all(|label| {
        let b = label.as_bytes();
        (1..=63).contains(&b.len())
            && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
            && b[0] != b'-'
            && b[b.len() - 1] != b'-'
    })
}

/// A manifest's host rules plus the scheme rule: `https`, or `http` only to
/// `127.0.0.1` / `localhost`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostAllow {
    pub rules: Vec<HostRule>,
}

impl HostAllow {
    /// Whether `copper.fetch` / `copper.oauth.authorize` may go to `url`.
    pub fn allows(&self, url: &Url) -> bool {
        let scheme_ok = match url.scheme() {
            "https" => true,
            "http" => url.host_str().is_some_and(is_loopback_host),
            _ => false,
        };
        scheme_ok && self.rules.iter().any(|r| r.matches(url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub(crate) fn sample() -> Value {
        json!({
            "schema": "copper-addon/1",
            "id": "example.sidebar",
            "name": "Example sidebar",
            "version": "1.0.0",
            "slot": "agent-sidebar",
            "entry": "index.html",
            "description": "A test addon.",
            "author": "Example",
            "bridge": 1,
            "permissions": {
                "hosts": ["api.example.com", "*.example.org", "127.0.0.1:8080"],
                "capabilities": ["identity", "storage", "fetch", "oauth", "open-url", "page", "agent"]
            },
            "settings": {
                "apiUrl": {"type": "string", "default": "https://api.example.com/v1", "title": "API"},
                "limit": {"type": "number", "default": 5}
            },
            "platforms": ["mac", "linux", "windows"],
            "futureKey": {"kept": true}
        })
    }

    fn with(edit: impl FnOnce(&mut Map<String, Value>)) -> Result<(), ManifestError> {
        let mut v = sample();
        edit(v.as_object_mut().unwrap());
        Manifest::from_value(v)?.validate()
    }

    fn code(r: Result<(), ManifestError>) -> Option<ManifestErrorCode> {
        r.err().map(|e| e.code)
    }

    #[test]
    fn valid_fixture_round_trips() {
        let m = Manifest::from_value(sample()).unwrap();
        m.validate().unwrap();
        assert_eq!(m.slot_id(), Some(SlotId::AgentSidebar));
        assert!(m.extra.contains_key("futureKey"));
        let back = serde_json::to_value(&m).unwrap();
        assert_eq!(back["futureKey"], json!({"kept": true}));
        assert_eq!(
            m.default_settings(),
            json!({"apiUrl": "https://api.example.com/v1", "limit": 5})
        );
        assert!(m.runs_on(Platform::Linux));
        assert_eq!(
            m.permissions.capabilities().len(),
            Capability::ALL.len(),
            "all granted"
        );
    }

    #[test]
    fn each_rule() {
        use ManifestErrorCode as C;
        type Edit = Box<dyn FnOnce(&mut Map<String, Value>)>;
        let cases: Vec<(C, Edit)> = vec![
            (
                C::Schema,
                Box::new(|m| {
                    m.insert("schema".into(), json!("copper-addon/2"));
                }),
            ),
            (
                C::Id,
                Box::new(|m| {
                    m.insert("id".into(), json!("nodot"));
                }),
            ),
            (
                C::Id,
                Box::new(|m| {
                    m.insert("id".into(), json!("a..b"));
                }),
            ),
            (
                C::Id,
                Box::new(|m| {
                    m.insert("id".into(), json!("Upper.case"));
                }),
            ),
            (
                C::Id,
                Box::new(|m| {
                    m.insert("id".into(), json!("-x.y"));
                }),
            ),
            (
                C::Id,
                Box::new(|m| {
                    m.insert("id".into(), json!(format!("a.{}", "b".repeat(63))));
                }),
            ),
            (
                C::Name,
                Box::new(|m| {
                    m.insert("name".into(), json!("  "));
                }),
            ),
            (
                C::Name,
                Box::new(|m| {
                    m.insert("name".into(), json!("n".repeat(61)));
                }),
            ),
            (
                C::Version,
                Box::new(|m| {
                    m.insert("version".into(), json!("1.0"));
                }),
            ),
            (
                C::Version,
                Box::new(|m| {
                    m.insert("version".into(), json!("01.0.0"));
                }),
            ),
            (
                C::Version,
                Box::new(|m| {
                    m.insert("version".into(), json!("1.0.0+build"));
                }),
            ),
            (
                C::Version,
                Box::new(|m| {
                    m.insert("version".into(), json!("1.0.0-"));
                }),
            ),
            (
                C::Description,
                Box::new(|m| {
                    m.insert("description".into(), json!("d".repeat(281)));
                }),
            ),
            (
                C::Slot,
                Box::new(|m| {
                    m.insert("slot".into(), json!("left-rail"));
                }),
            ),
            (
                C::Entry,
                Box::new(|m| {
                    m.insert("entry".into(), json!("index.js"));
                }),
            ),
            (
                C::Entry,
                Box::new(|m| {
                    m.insert("entry".into(), json!("../index.html"));
                }),
            ),
            (
                C::Entry,
                Box::new(|m| {
                    m.insert("entry".into(), json!("/index.html"));
                }),
            ),
            (
                C::Bridge,
                Box::new(|m| {
                    m.insert("bridge".into(), json!(0));
                }),
            ),
            (
                C::Bridge,
                Box::new(|m| {
                    m.remove("bridge");
                }),
            ),
            (
                C::Json,
                Box::new(|m| {
                    m.insert("bridge".into(), json!("1"));
                }),
            ),
            (
                C::Json,
                Box::new(|m| {
                    m.remove("entry");
                }),
            ),
            (
                C::Hosts,
                Box::new(|m| {
                    m["permissions"]["hosts"] = json!(["API.example.com"]);
                }),
            ),
            (
                C::Hosts,
                Box::new(|m| {
                    m["permissions"]["hosts"] = json!(["*.com"]);
                }),
            ),
            (
                C::Hosts,
                Box::new(|m| {
                    m["permissions"]["hosts"] = json!(["http://api.example.com"]);
                }),
            ),
            (
                C::Hosts,
                Box::new(|m| {
                    m["permissions"]["hosts"] = json!(["api.example.com:0"]);
                }),
            ),
            (
                C::Hosts,
                Box::new(|m| {
                    m["permissions"]["hosts"] = json!(["api.example.com/path"]);
                }),
            ),
            (
                C::Hosts,
                Box::new(|m| {
                    m["permissions"]["hosts"] = json!(
                        (0..33)
                            .map(|i| format!("h{i}.example.com"))
                            .collect::<Vec<_>>()
                    );
                }),
            ),
            (
                C::Capabilities,
                Box::new(|m| {
                    m["permissions"]["capabilities"] = json!(["camera"]);
                }),
            ),
            (
                C::Settings,
                Box::new(|m| {
                    m["settings"]["x"] = json!({"type": "date"});
                }),
            ),
            (
                C::Platforms,
                Box::new(|m| {
                    m.insert("platforms".into(), json!(["beos"]));
                }),
            ),
        ];
        for (want, edit) in cases {
            assert_eq!(code(with(edit)), Some(want));
        }
        // Optional fields may go.
        assert_eq!(
            code(with(|m| {
                m.remove("description");
                m.remove("author");
                m.remove("settings");
                m.remove("platforms");
                m.remove("permissions");
            })),
            None
        );
        assert!(Manifest::from_slice(b"[1]").is_err());
        assert_eq!(
            Manifest::from_slice(b"{").unwrap_err().code,
            ManifestErrorCode::Json
        );
    }

    #[test]
    fn versions() {
        for ok in ["0.0.0", "1.2.3", "10.20.30", "1.0.0-beta.1", "1.0.0-rc-1"] {
            assert!(valid_version(ok), "{ok}");
        }
        for bad in [
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.x",
            "1.02.3",
            "1.2.3-a..b",
        ] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    #[test]
    fn host_rules() {
        let u = |s: &str| Url::parse(s).unwrap();
        let exact = HostRule::parse("api.example.com").unwrap();
        assert!(exact.matches(&u("https://api.example.com/x")));
        assert!(exact.matches(&u("https://API.example.com:8443/x")));
        assert!(!exact.matches(&u("https://example.com/")));
        assert!(!exact.matches(&u("https://evilapi.example.com/")));
        let wild = HostRule::parse("*.example.org").unwrap();
        assert!(wild.matches(&u("https://a.example.org/")));
        assert!(wild.matches(&u("https://a.b.example.org/")));
        assert!(!wild.matches(&u("https://example.org/")));
        assert!(!wild.matches(&u("https://badexample.org/")));
        let port = HostRule::parse("127.0.0.1:8080").unwrap();
        assert!(port.matches(&u("http://127.0.0.1:8080/")));
        assert!(!port.matches(&u("http://127.0.0.1:8081/")));
        let https_port = HostRule::parse("api.example.com:443").unwrap();
        assert!(https_port.matches(&u("https://api.example.com/")));
        assert!(HostRule::parse("http://localhost:3000").is_ok());
        assert!(HostRule::parse("https://api.example.com").is_ok());

        let allow = HostAllow {
            rules: vec![exact, port, HostRule::parse("localhost").unwrap()],
        };
        assert!(allow.allows(&u("https://api.example.com/v1")));
        assert!(!allow.allows(&u("http://api.example.com/v1")), "https only");
        assert!(allow.allows(&u("http://127.0.0.1:8080/")));
        assert!(allow.allows(&u("http://localhost:9999/")));
        assert!(!allow.allows(&u("ftp://api.example.com/")));
        assert!(!allow.allows(&u("https://other.example.com/")));
    }
}
