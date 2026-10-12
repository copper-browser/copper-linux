//! `groups.json` (Mac: `Fork/Groups.swift`) and `windows.json`
//! (Mac: `Fork/Windows.swift`).

use crate::session::SessionEntry;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A tab group / folder (`TabGroup`). Folder paths are written into `name`
/// with ` › ` between segments.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TabGroup {
    /// UUID, uppercase.
    pub id: String,
    pub name: String,
    /// 0…1, or `None` for grey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<f64>,
    #[serde(default)]
    pub collapsed: bool,
    /// The space an *empty* folder stands in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Where an empty folder stands among its parent's tabs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl TabGroup {
    /// A new group with a fresh id.
    pub fn new(name: impl Into<String>, hue: Option<f64>) -> Self {
        TabGroup {
            id: crate::ids::new_uuid(),
            name: name.into(),
            hue,
            ..Default::default()
        }
    }

    /// The folder path segments of `name` (split on ` › `).
    pub fn path(&self) -> Vec<&str> {
        self.name.split(" › ").collect()
    }
}

/// The whole of `groups.json` (`Groups.File`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GroupsFile {
    #[serde(default)]
    pub groups: Vec<TabGroup>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One saved window (`Windows.Saved`). `windows.json` is `[SavedWindow]`.
///
/// Modern rows carry the window's space and its active URL. Legacy rows
/// carried their own `tabs` and an `active` index; those decode and are kept
/// here so nothing is lost, though the Mac no longer writes them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SavedWindow {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Legacy only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<Vec<SessionEntry>>,
    /// Legacy only: index into `tabs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
