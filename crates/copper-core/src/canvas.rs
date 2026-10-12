//! `canvas/canvases.json` (Mac: `Fork/Canvas/Canvas.swift`, `[Canvases.Entry]`).
//! Pretty-printed, sorted keys, `.iso8601` dates.

use crate::date::IsoDate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The Personal canvas's id.
pub const PERSONAL_CANVAS_ID: &str = "personal";

/// One canvas.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEntry {
    pub id: String,
    pub name: String,
    /// `personal`, `shared` or `local`.
    pub kind: String,
    /// The server's id; `None` for a local canvas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    pub created_at: IsoDate,
    pub updated_at: IsoDate,
    /// Shared canvases: `owner` or `editor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// The owner's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<i64>,
    /// The account (user id) this row came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CanvasEntry {
    /// Can rename / delete / invite rather than only leave.
    pub fn is_owner(&self) -> bool {
        self.kind != "shared" || self.role.as_deref().is_none_or(|r| r == "owner")
    }
}
