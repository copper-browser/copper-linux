//! Model keys (Mac: `Fork/Intelligence.swift` `Intelligence.Keys`, written to
//! `intelligence.json`) and the Claude account credential (Mac:
//! `Fork/ClaudeAccount.swift`, `claude.json`). Both 0600; never log keys or
//! tokens.

use crate::date::SwiftDate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// `intelligence.json`. Lenient like the Mac: every field optional, defaults
/// via accessors.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntelligenceKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_key: Option<String>,
    #[serde(default, rename = "routerURL", skip_serializing_if = "Option::is_none")]
    pub router_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_model: Option<String>,
    /// `key` (API key lane) or `claude` (Claude account lane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// `haiku`, `sonnet` or `opus`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// tier → Anthropic model id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_models: Option<BTreeMap<String, String>>,
    /// tier → gateway model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router_models: Option<BTreeMap<String, String>>,
    /// The small model Jev asks to write field values; empty = router model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_model: Option<String>,
    /// Jev mode's Fast mode: the shipped strategy (on, the default) or the
    /// classic loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev_fast: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl IntelligenceKeys {
    /// Default Jev endpoint.
    pub const DEFAULT_JEV_ENDPOINT: &'static str = "https://api.typesafe.ai/v1/systemone";
    /// Default Jev model.
    pub const DEFAULT_JEV_MODEL: &'static str = "jev-latest";

    /// The Jev endpoint, defaulted.
    pub fn jev_endpoint(&self) -> &str {
        self.jev_endpoint
            .as_deref()
            .unwrap_or(Self::DEFAULT_JEV_ENDPOINT)
    }
    /// The Jev model, defaulted.
    pub fn jev_model(&self) -> &str {
        self.jev_model.as_deref().unwrap_or(Self::DEFAULT_JEV_MODEL)
    }
    /// The Jev key, if set and non-empty.
    pub fn jev_key(&self) -> Option<&str> {
        self.jev_key.as_deref().filter(|k| !k.is_empty())
    }
    /// The lane (`key` by default).
    pub fn lane(&self) -> &str {
        self.lane.as_deref().unwrap_or("key")
    }
    /// The tier (`sonnet` by default).
    pub fn tier(&self) -> &str {
        self.tier.as_deref().unwrap_or("sonnet")
    }
    /// Fast mode (on by default, as the Mac).
    pub fn jev_fast(&self) -> bool {
        self.jev_fast.unwrap_or(true)
    }
}

/// `claude.json` (`ClaudeAccount.Credential`). `expires` is a Swift date.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ClaudeCredential {
    pub access: String,
    pub refresh: String,
    pub expires: SwiftDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_mode_is_on_unless_the_file_says_off() {
        let fresh: IntelligenceKeys = serde_json::from_str("{}").unwrap();
        assert!(fresh.jev_fast());
        let off: IntelligenceKeys = serde_json::from_str(r#"{"jevFast":false}"#).unwrap();
        assert!(!off.jev_fast());
        assert!(off.extra.is_empty());
        assert_eq!(serde_json::to_string(&off).unwrap(), r#"{"jevFast":false}"#);
    }
}
