//! `trails.json` (Mac: `Fork/Trails/Trails.swift`, `Trails.Disk`). Written
//! with sorted keys. All times are **Unix** seconds (`Double`), not dates.

use crate::date::UnixTime;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrailsFile {
    #[serde(default = "one")]
    pub version: i64,
    #[serde(default)]
    pub trails: Vec<Trail>,
    #[serde(default)]
    pub pages: Vec<TrailPage>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> i64 {
    1
}

/// One trail: a search or a founding tab and the pages opened from it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Trail {
    pub id: String,
    /// The founding page's `key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub founder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    pub created: UnixTime,
    pub used: UnixTime,
    pub rank: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One page of a trail. `key` is the tab's id when written; `space`, `url`
/// and `ordinal` find the tab again after a restore.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TrailPage {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    pub url: String,
    pub ordinal: i64,
    pub trail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub opened: UnixTime,
    pub used: UnixTime,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
