//! Copper Cloud's local files and sync documents.
//!
//! * `cloud.json` — [`CloudFile`] (Mac: `Fork/Cloud/Cloud.swift`, `Cloud.Saved`). 0600.
//! * `cloud-sync.json` — [`CloudSyncFile`] (Mac: `Fork/Cloud/CloudSync.swift`).
//!   Each base's `payload` is base64 of the document's JSON, kept verbatim.
//! * `cloud-intelligence.json` — [`CloudProvided`]
//!   (Mac: `Fork/Cloud/CloudIntelligence.swift`), `.iso8601` dates. 0600.
//! * The sync documents themselves — [`SpacesDoc`], [`SettingsDoc`],
//!   [`BookmarksDoc`], [`TabsDoc`] (Mac: `Fork/Cloud/CloudMerge.swift`). The
//!   Mac encodes these with sorted keys; use [`encode_doc`] so two devices
//!   produce the same bytes for the same document.

use crate::bookmarks::Bookmark;
use crate::date::{IsoDate, UnixTime};
use crate::json::{self, Style};
use crate::session::Space;
use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// `cloud.json` (`Cloud.Saved`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudFile {
    /// This device's UUID.
    pub device_id: String,
    pub device_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<CloudLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<CloudAccount>,
    /// The session token. Never log it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub sync: SyncPrefs,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Which copper-cloud this device is linked to (`Cloud.Link`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloudLink {
    /// `https://host[:port]` (`http://` only for loopback).
    pub url: String,
    /// The instance key every `/v1` request carries.
    pub key: String,
    /// SHA-256 of the leaf certificate (DER), lowercase hex; `None` = system trust.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The signed-in account (`Cloud.Account`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccount {
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    pub device_id: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The sync switches (`Cloud.SyncPrefs`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPrefs {
    #[serde(default)]
    pub spaces: bool,
    #[serde(default)]
    pub settings: bool,
    #[serde(default)]
    pub bookmarks: bool,
    #[serde(default)]
    pub history: bool,
    #[serde(default)]
    pub tabs: bool,
    /// "Turn on sync" was pressed.
    #[serde(default)]
    pub on: bool,
    /// History: newest local visit already pushed, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_through: Option<UnixTime>,
    /// History: newest server sequence already pulled.
    #[serde(default)]
    pub pulled_seq: i64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `cloud-sync.json` (`CloudSync.Saved`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloudSyncFile {
    /// The account the bases belong to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Domain (`spaces`, `settings`, `bookmarks`, `tabs:<device>`) → base.
    #[serde(default)]
    pub docs: BTreeMap<String, SyncBase>,
    /// Domain → remote timestamp, Unix seconds.
    #[serde(default)]
    pub remote: BTreeMap<String, f64>,
    /// The Personal canvas switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The last agreed version of one document (`CloudSync.Base`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SyncBase {
    pub version: i64,
    /// Base64 (standard, padded) of the document's JSON bytes.
    pub payload: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Errors decoding a sync payload.
#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error("payload is not base64")]
    Base64,
    #[error("payload JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Decode base64 the way `CloudSync.decode` does: standard first, then
/// base64url without padding.
pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
    STANDARD
        .decode(text)
        .ok()
        .or_else(|| URL_SAFE_NO_PAD.decode(text.trim_end_matches('=')).ok())
}

/// Standard padded base64, as Swift's `base64EncodedString()`.
pub fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A sync document's bytes, as the Mac makes them: compact, sorted keys.
pub fn encode_doc<T: Serialize>(doc: &T) -> serde_json::Result<Vec<u8>> {
    json::encode(doc, Style::Compact)
}

impl SyncBase {
    /// A base for `doc` at `version`.
    pub fn from_doc<T: Serialize>(version: i64, doc: &T) -> serde_json::Result<Self> {
        Ok(SyncBase {
            version,
            payload: encode_base64(&encode_doc(doc)?),
            extra: Map::new(),
        })
    }
    /// The raw document bytes.
    pub fn payload_bytes(&self) -> Result<Vec<u8>, PayloadError> {
        decode_base64(&self.payload).ok_or(PayloadError::Base64)
    }
    /// The document, typed.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, PayloadError> {
        Ok(serde_json::from_slice(&self.payload_bytes()?)?)
    }
}

/// The `spaces` document (`CloudDocs.Spaces`): spaces plus pinned and Saved tabs.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SpacesDoc {
    #[serde(default = "one")]
    pub v: i64,
    pub spaces: Vec<Space>,
    pub tabs: Vec<KeptTab>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A pinned or Saved tab in [`SpacesDoc`] (`CloudDocs.KeptTab`). `key` is
/// how two devices agree it is the same tab.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct KeptTab {
    pub key: String,
    pub space: String,
    pub url: String,
    pub title: String,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The `settings` document (`CloudDocs.Settings`): allowed keys only, see
/// [`crate::settings::CLOUD_SETTINGS_KEYS`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SettingsDoc {
    #[serde(default = "one")]
    pub v: i64,
    pub values: BTreeMap<String, String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The `bookmarks` document (`CloudDocs.Bookmarks`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BookmarksDoc {
    #[serde(default = "one")]
    pub v: i64,
    pub roots: Vec<Bookmark>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A device's open tabs (`CloudDocs.Tabs`, domain `tabs:<device-id>`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TabsDoc {
    #[serde(default = "one")]
    pub v: i64,
    /// The device's name, as people read it.
    pub device: String,
    /// Unix seconds.
    pub updated: UnixTime,
    pub tabs: Vec<OpenTab>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One open tab in a [`TabsDoc`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OpenTab {
    pub url: String,
    pub title: String,
    /// The space's *name*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> i64 {
    1
}

/// `cloud-intelligence.json` (`CloudProvided`): keys and agent settings the
/// org's cloud hands out. 0600; never log `key` fields.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloudProvided {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<CloudJev>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<CloudRouter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<CloudAgent>,
    /// The server's own `updated_at` (RFC 3339), as sent.
    #[serde(
        default,
        rename = "updated_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<String>,
    /// `host:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// `link URL|user id`: whose cache this is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(
        default,
        rename = "fetched_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub fetched_at: Option<IsoDate>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Jev credentials from the cloud.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloudJev {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// LLM router credentials from the cloud.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloudRouter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The org's agent settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CloudAgent {
    /// 1…500; anything else means "not provided".
    #[serde(default, rename = "max_turns", skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CloudProvided {
    /// The org's tool-call rounds per question, when usable (1…500).
    pub fn agent_max_turns(&self) -> Option<i64> {
        self.agent
            .as_ref()?
            .max_turns
            .filter(|n| (1..=500).contains(n))
    }
}
