//! What `GET /v1/addons` answers (contract §4.3): the addons that apply to
//! the signed-in person, at most one per slot, with resolved settings.
//! Unknown keys are kept; settings are free JSON.

use super::Platform;
use super::manifest::{Permissions, runs_on};
use super::slot::SlotId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The whole answer.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ResolvedSet {
    /// `r_<hex16>`: a hash of the list (ids, sha256s, settings). Empty for
    /// "no addons" (a cloud without the route).
    #[serde(default)]
    pub revision: String,
    #[serde(default)]
    pub addons: Vec<ResolvedAddon>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One resolved addon.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ResolvedAddon {
    pub id: String,
    pub slot: String,
    #[serde(default)]
    pub name: String,
    pub version: String,
    /// The served bundle's SHA-256 (lowercase hex).
    pub sha256: String,
    /// Served bytes.
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub entry: String,
    #[serde(default = "one")]
    pub bridge: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platforms: Option<Vec<String>>,
    #[serde(default)]
    pub permissions: Permissions,
    /// Manifest defaults overlaid by the admin's values.
    #[serde(default = "empty_object")]
    pub settings: Value,
    /// Path of the bundle on the cloud: `/v1/addons/<id>/bundle/<sha256>`.
    #[serde(default)]
    pub bundle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> u32 {
    1
}

pub(crate) fn empty_object() -> Value {
    Value::Object(Map::new())
}

impl ResolvedSet {
    /// Read the body leniently: an entry that doesn't read (no id, no sha256,
    /// a wrong type) is dropped and logged, the rest still apply. `Err` only
    /// when the body is not a JSON object.
    pub fn parse(bytes: &[u8]) -> Result<ResolvedSet, serde_json::Error> {
        let mut doc: Map<String, Value> = serde_json::from_slice(bytes)?;
        let revision = match doc.remove("revision") {
            Some(Value::String(s)) => s,
            _ => String::new(),
        };
        let addons = match doc.remove("addons") {
            Some(Value::Array(list)) => list
                .into_iter()
                .filter_map(|v| match serde_json::from_value::<ResolvedAddon>(v) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        tracing::warn!(error = %e, "an entry of /v1/addons doesn't read; skipped");
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(ResolvedSet {
            revision,
            addons,
            extra: doc,
        })
    }

    /// The first addon listed for `slot`.
    pub fn for_slot(&self, slot: SlotId) -> Option<&ResolvedAddon> {
        self.addons.iter().find(|a| a.slot == slot.as_str())
    }
}

impl ResolvedAddon {
    /// The slot, if this build knows it.
    pub fn slot_id(&self) -> Option<SlotId> {
        self.slot.parse().ok()
    }

    /// Whether it is meant for `platform` (absent or empty = all).
    pub fn runs_on(&self, platform: Platform) -> bool {
        runs_on(self.platforms.as_deref(), platform)
    }

    /// `name`, or the id when the cloud sent no name.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    /// The bundle's path on the cloud (`bundle`, or built from id and sha).
    pub fn bundle_path(&self) -> String {
        if self.bundle.starts_with("/v1/") {
            self.bundle.clone()
        } else {
            format!("/v1/addons/{}/bundle/{}", self.id, self.sha256)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_answer_leniently() {
        let body = br#"{
            "revision": "r_0123456789abcdef",
            "addons": [
                {"id": "example.sidebar", "slot": "agent-sidebar", "name": "Example", "version": "1.0.0",
                 "sha256": "ab", "size": 10, "entry": "index.html", "bridge": 1,
                 "platforms": ["linux"], "permissions": {"hosts": ["api.example.com"], "capabilities": ["fetch"]},
                 "settings": {"a": 1}, "bundle": "/v1/addons/example.sidebar/bundle/ab",
                 "updated_at": "2026-10-19T00:00:00Z", "future": 1},
                {"slot": "agent-sidebar"},
                {"id": "example.other", "slot": "left-rail", "version": "1.0.0", "sha256": "cd"}
            ],
            "extra": true
        }"#;
        let set = ResolvedSet::parse(body).unwrap();
        assert_eq!(set.revision, "r_0123456789abcdef");
        assert_eq!(set.addons.len(), 2, "the entry without id is dropped");
        let a = set.for_slot(SlotId::AgentSidebar).unwrap();
        assert_eq!(a.settings["a"], 1);
        assert!(a.extra.contains_key("future"));
        assert!(a.runs_on(Platform::Linux));
        assert!(!a.runs_on(Platform::Mac));
        assert_eq!(set.addons[1].slot_id(), None);
        assert_eq!(set.addons[1].bridge, 1);
        assert_eq!(set.addons[1].settings, empty_object());
        assert_eq!(
            set.addons[1].bundle_path(),
            "/v1/addons/example.other/bundle/cd"
        );
        assert!(set.extra.contains_key("extra"));
        assert!(ResolvedSet::parse(b"[]").is_err());
    }
}
