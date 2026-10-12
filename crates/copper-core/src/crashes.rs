//! `crashes/crashes.json` (Mac: `Fork/Crashes.swift`, `Crashes.Index`) and
//! `hidden.json` (Mac: `Curtain.swift`, `[String: [Veil]]`).
//!
//! The crash index is pretty-printed with sorted keys and `.iso8601` dates.
//! On Linux a web-process crash is recorded here with kind `report` (or
//! `unclean` when the browser itself died without a trace).

use crate::date::{IsoDate, SwiftDate};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// The index.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CrashIndex {
    /// The newest report taken so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through: Option<IsoDate>,
    #[serde(default)]
    pub entries: Vec<CrashEntry>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One crash or fault (`Crashes.Entry`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CrashEntry {
    pub id: String,
    /// `report`, `fault`, `exception` or `unclean`.
    pub kind: String,
    pub time: IsoDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched: Option<IsoDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<Vec<CrashFrame>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<CrashBinary>,
    /// Breadcrumbs from the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trail: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CrashEntry {
    /// A crash rather than a fault it survived.
    pub fn fatal(&self) -> bool {
        self.kind != "fault"
    }
}

/// One stack frame.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CrashFrame {
    pub image: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub offset: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The app's own binary in that run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CrashBinary {
    pub uuid: String,
    pub base: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One hidden element (`Veil`). `date` is a Swift date.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Veil {
    /// The CSS selector; also the row's identity.
    pub selector: String,
    /// What it was, in words.
    pub label: String,
    /// Size and position when hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub date: SwiftDate,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `hidden.json`: host (no `www.`) → what is hidden there.
pub type HiddenFile = BTreeMap<String, Vec<Veil>>;
