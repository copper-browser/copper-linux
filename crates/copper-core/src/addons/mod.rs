//! Addons: small web bundles from Copper Cloud that take over a slot of the
//! window (v1: the agent sidebar). Toolkit-free and shared by every desktop
//! build (Linux now, Windows next); the Mac carries the same rules in Swift.
//! The binding spec is the addons contract (`CONTRACT.md` §2–§6, §8).
//!
//! | Module | What |
//! |---|---|
//! | [`slot`] | [`SlotId`], the closed list of slots |
//! | [`manifest`] | `manifest.json` ([`Manifest`]), validation, [`Capability`], host rules ([`HostRule`], [`HostAllow`]) |
//! | [`bundle`] | the served bundle format: [`verify_bundle`], atomic [`Bundle::write_to`] |
//! | [`resolved`] | what `GET /v1/addons` answers ([`ResolvedSet`], [`ResolvedAddon`]) |
//! | [`state`] | `addons/state.json` ([`AddonState`]), the folder ([`AddonStore`]), the developer override |
//! | [`plan`] | applying an answer ([`plan()`](plan::plan), [`install_pending`], [`active_for`]) |
//! | [`csp`] | the CSP `<meta>` every addon page gets |
//! | [`scheme`] | `copper-addon://<id>/<path>` answers |
//! | [`bridge`] | `window.copper` calls: [`dispatch`], [`BridgeHost`], [`Reply::to_js`] |
//!
//! The JS half of the bridge is [`BRIDGE_JS`]: the canonical shim, the same
//! bytes on every platform.

pub mod bridge;
pub mod bundle;
pub mod csp;
pub mod manifest;
pub mod plan;
pub mod resolved;
pub mod scheme;
pub mod slot;
pub mod state;
#[doc(hidden)]
pub mod testkit;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

pub use bridge::{
    BridgeError, BridgeHost, BridgeRequest, Context, ErrorCode, FetchRequest, FetchResponse,
    Fetcher, Identity, PageInfo, Reply, dispatch, emit_js, handle_message,
};
pub use bundle::{Bundle, BundleError, verify_bundle};
pub use csp::inject_csp;
pub use manifest::{Capability, HostAllow, HostRule, Manifest, ManifestError, Permissions};
pub use plan::{Change, Plan, active_for, install_pending, plan};
pub use resolved::{ResolvedAddon, ResolvedSet};
pub use slot::SlotId;
pub use state::{ActiveAddon, AddonState, AddonStore, Installed};

/// The bridge version this build speaks. An addon whose manifest asks for a
/// higher one is not run.
pub const BRIDGE_VERSION: u32 = 1;

/// The bridge shim (`window.copper`), injected at document start, main frame
/// only, into the slot's own view before any addon script.
pub const BRIDGE_JS: &str = include_str!("bridge.js");

/// A Copper platform, as manifests and `copper.info()` spell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Mac,
    Linux,
    Windows,
}

impl Platform {
    /// The platform this build runs on (anything not Mac or Windows counts
    /// as Linux).
    pub fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// `"mac"`, `"linux"`, `"windows"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Mac => "mac",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "mac" => Ok(Platform::Mac),
            "linux" => Ok(Platform::Linux),
            "windows" => Ok(Platform::Windows),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_js_is_the_canonical_shim() {
        assert!(BRIDGE_JS.starts_with("// Copper addon bridge, v1. CANONICAL COPY"));
        assert!(BRIDGE_JS.contains("var BRIDGE = 1;"));
        assert_eq!(BRIDGE_VERSION, 1);
    }

    #[test]
    fn platforms() {
        assert_eq!("windows".parse(), Ok(Platform::Windows));
        assert!("beos".parse::<Platform>().is_err());
        assert_eq!(serde_json::to_string(&Platform::Mac).unwrap(), "\"mac\"");
        #[cfg(target_os = "linux")]
        assert_eq!(Platform::current(), Platform::Linux);
    }
}
