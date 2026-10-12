//! `updates.json` (Mac: `Fork/Updates.swift`, `Updates.Saved`). Dates are
//! Swift dates (2001 epoch); `latest.publishedAt` is the feed's string.

use crate::date::SwiftDate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The file. Every field is optional.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<SwiftDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announced_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_announced: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trouble_announced: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<LatestRelease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<StagedUpdate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<UpdateOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<PendingUpdate>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The newest release the feed announced (`Updates.Latest`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestRelease {
    pub version: String,
    pub release_id: String,
    pub commit: String,
    /// As the feed sent it (RFC 3339).
    pub published_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Downloaded, verified, waiting for the swap (`Updates.Staged`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StagedUpdate {
    pub version: String,
    pub path: String,
    pub sha256: String,
    pub at: SwiftDate,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What the last update left behind (`Updates.Outcome`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UpdateOutcome {
    pub ok: bool,
    pub detail: String,
    pub at: SwiftDate,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Written just before the swapped-in build launches (`Updates.Pending`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PendingUpdate {
    pub version: String,
    pub from: String,
    pub at: SwiftDate,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
