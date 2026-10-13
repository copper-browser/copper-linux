//! Web platform features an engine build may lack (`Engine::capabilities`).
//!
//! WebKitGTK builds differ by distribution: Ubuntu 24.04's 2.52 has no
//! WebRTC, no Encrypted Media Extensions and no Web Authentication compiled
//! in, while others do. The engine turns every one of these on where it can;
//! this says which ones the running build actually has, so the app can tell
//! the user why a call or a passkey will not work instead of letting the
//! site fail on its own.

use serde::{Deserialize, Serialize};

/// One web platform feature sites depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// `RTCPeerConnection`.
    WebRtc,
    /// `navigator.mediaDevices.getUserMedia`: camera and microphone.
    MediaStream,
    /// Encrypted Media Extensions (`navigator.requestMediaKeySystemAccess`).
    EncryptedMedia,
    /// Web Authentication (`PublicKeyCredential`).
    WebAuthn,
}

impl Capability {
    /// Every capability, in the order the app lists them.
    pub const ALL: [Capability; 4] = [
        Capability::WebRtc,
        Capability::MediaStream,
        Capability::EncryptedMedia,
        Capability::WebAuthn,
    ];

    /// Its usual name.
    pub fn name(self) -> &'static str {
        match self {
            Capability::WebRtc => "WebRTC",
            Capability::MediaStream => "Camera and microphone",
            Capability::EncryptedMedia => "Encrypted media",
            Capability::WebAuthn => "Passkeys and security keys",
        }
    }

    /// What a user loses without it, in a few words.
    pub fn needed_for(self) -> &'static str {
        match self {
            Capability::WebRtc => "video and voice calls (Meet, Teams, Slack huddles, Discord)",
            Capability::MediaStream => "sites that use your camera or microphone",
            Capability::EncryptedMedia => {
                "protected video and music (Widevine is never available on WebKitGTK)"
            }
            Capability::WebAuthn => "signing in with a passkey or a security key",
        }
    }
}

/// What the running engine build can do, of [`Capability::ALL`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub webrtc: bool,
    pub media_stream: bool,
    pub encrypted_media: bool,
    pub web_authn: bool,
}

impl Capabilities {
    /// Whether the build has `capability`.
    pub fn has(&self, capability: Capability) -> bool {
        match capability {
            Capability::WebRtc => self.webrtc,
            Capability::MediaStream => self.media_stream,
            Capability::EncryptedMedia => self.encrypted_media,
            Capability::WebAuthn => self.web_authn,
        }
    }

    /// The capabilities this build lacks, in [`Capability::ALL`] order.
    pub fn unavailable(&self) -> Vec<Capability> {
        Capability::ALL
            .into_iter()
            .filter(|c| !self.has(*c))
            .collect()
    }

    /// One line per missing capability ("WebRTC — video and voice calls (…)"),
    /// or an empty string when nothing is missing.
    pub fn summary(&self) -> String {
        self.unavailable()
            .into_iter()
            .map(|c| format!("{} — {}", c.name(), c.needed_for()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_in_order() {
        let caps = Capabilities {
            media_stream: true,
            ..Capabilities::default()
        };
        assert_eq!(
            caps.unavailable(),
            [
                Capability::WebRtc,
                Capability::EncryptedMedia,
                Capability::WebAuthn
            ]
        );
        assert!(caps.has(Capability::MediaStream));
        assert!(caps.summary().starts_with("WebRTC — video and voice calls"));
        let all = Capabilities {
            webrtc: true,
            media_stream: true,
            encrypted_media: true,
            web_authn: true,
        };
        assert!(all.unavailable().is_empty() && all.summary().is_empty());
        let json = serde_json::to_string(&caps).unwrap();
        assert_eq!(
            json,
            r#"{"webrtc":false,"media_stream":true,"encrypted_media":false,"web_authn":false}"#
        );
    }
}
