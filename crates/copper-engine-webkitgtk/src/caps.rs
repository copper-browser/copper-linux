//! What this WebKitGTK build has of [`copper_engine::Capability`], and turning
//! each of them on for a page.
//!
//! WebKit lists a runtime feature only when the build has the code behind it
//! (`webkit_settings_get_all_features`), so a feature's absence means the
//! build lacks it: Ubuntu 24.04's 2.52 has no `PeerConnection`,
//! `EncryptedMediaAPI` or `WebAuthentication` at all, and setting
//! `enable-webrtc` there changes nothing. The settings are still set on every
//! page, so a build that has them gets them.

use std::collections::HashMap;

use copper_engine::Capabilities;
use webkit6::{Feature, FeatureStatus, Settings};

/// Feature identifiers (WebKit's `UnifiedWebPreferences.yaml` names without
/// `Enabled`) that exist only when the build has the capability.
const WEBRTC: &str = "PeerConnection";
const MEDIA_STREAM: &str = "MediaDevices";
const ENCRYPTED_MEDIA: &str = "EncryptedMediaAPI";
const WEB_AUTHN: &str = "WebAuthentication";

thread_local! {
    /// The features above that this build lists, by identifier.
    static FEATURES: HashMap<String, Feature> = listed();
}

fn listed() -> HashMap<String, Feature> {
    let mut out = HashMap::new();
    let Some(list) = Settings::all_features() else {
        return out;
    };
    for i in 0..list.length() {
        let Some(feature) = list.get(i) else { continue };
        let Some(id) = feature.identifier() else {
            continue;
        };
        if [WEBRTC, MEDIA_STREAM, ENCRYPTED_MEDIA, WEB_AUTHN].contains(&id.as_str()) {
            out.insert(id.to_string(), feature);
        }
    }
    out
}

/// Ready for use: on by default, or marked for embedders / shipped. A
/// feature that is only testable or internal in this build does not count.
fn usable(feature: &Feature) -> bool {
    feature.is_default_value()
        || matches!(
            feature.status(),
            FeatureStatus::Embedder
                | FeatureStatus::Stable
                | FeatureStatus::Mature
                | FeatureStatus::Preview
        )
}

/// What the running build has (main thread).
pub(crate) fn capabilities() -> Capabilities {
    FEATURES.with(|f| {
        let has = |id: &str| f.get(id).is_some_and(usable);
        Capabilities {
            webrtc: has(WEBRTC),
            media_stream: has(MEDIA_STREAM),
            encrypted_media: has(ENCRYPTED_MEDIA),
            web_authn: has(WEB_AUTHN),
        }
    })
}

/// Turn on, in a page's settings, WebRTC, camera and microphone, encrypted
/// media and — when the build has it ready — Web Authentication. Every site
/// still asks before it uses a camera or a microphone
/// (`PageEvent::PermissionRequest`).
pub(crate) fn enable(settings: &Settings) {
    settings.set_enable_webrtc(true);
    settings.set_enable_media_stream(true);
    settings.set_enable_encrypted_media(true);
    FEATURES.with(|f| {
        if let Some(feature) = f.get(WEB_AUTHN).filter(|w| usable(w)) {
            settings.set_feature_enabled(feature, true);
        }
    });
}
