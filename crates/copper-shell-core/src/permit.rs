//! A site's camera and microphone answers, remembered per site (Mac
//! `Browser.swift` "the camera and the microphone", `forgetCaptureChoices`)
//! so a call you take every week asks once.
//!
//! Kept in settings as `capture.{host}|camera` and `capture.{host}|microphone`
//! (the Mac's prefix; one key per kind, so the two are remembered apart). A
//! private tab reads and writes none of them. The same keys as Windows'
//! `chrome/permit.rs`, which can switch to these once it bumps shell-core.

use crate::{Platform, Shell};
use copper_engine::PermissionKind;

/// The Mac's prefix for camera and microphone answers.
pub const CAPTURE: &str = "capture.";

/// The settings keys an answer about `kind` from `host` is kept under;
/// none for kinds that are asked every time.
pub fn capture_keys(kind: &PermissionKind, host: &str) -> Vec<String> {
    let one = |k: &str| format!("{CAPTURE}{host}|{k}");
    match kind {
        PermissionKind::Camera => vec![one("camera")],
        PermissionKind::Microphone => vec![one("microphone")],
        PermissionKind::CameraAndMicrophone => vec![one("camera"), one("microphone")],
        _ => Vec::new(),
    }
}

/// What was remembered for all of `keys`: `Some(true)` only if each was
/// allowed, `Some(false)` if any was refused, `None` to ask.
pub fn remembered(keys: &[String], get: impl Fn(&str) -> Option<bool>) -> Option<bool> {
    if keys.is_empty() {
        return None;
    }
    let mut all = true;
    for k in keys {
        match get(k) {
            Some(false) => return Some(false),
            Some(true) => {}
            None => all = false,
        }
    }
    all.then_some(true)
}

impl<P: Platform> Shell<P> {
    /// The remembered answer for `kind` from `host`, if every part of it
    /// was answered.
    pub fn capture_choice(&self, kind: &PermissionKind, host: &str) -> Option<bool> {
        let s = self.settings();
        remembered(&capture_keys(kind, host), |k| {
            s.get(k).and_then(|v| v.as_bool())
        })
    }

    /// Keep `allowed` for `kind` from `host`.
    pub fn keep_capture_choice(&self, kind: &PermissionKind, host: &str, allowed: bool) {
        let keys = capture_keys(kind, host);
        if keys.is_empty() {
            return;
        }
        self.update_settings(|s| {
            for k in &keys {
                s.set(k, allowed);
            }
        });
    }

    /// Whether any site's answer is remembered.
    pub fn has_capture_choices(&self) -> bool {
        self.settings()
            .values()
            .keys()
            .any(|k| k.starts_with(CAPTURE))
    }

    /// Forget every site's answer (Mac `forgetCaptureChoices`).
    pub fn forget_capture_choices(&self) {
        let keys: Vec<String> = self
            .settings()
            .values()
            .keys()
            .filter(|k| k.starts_with(CAPTURE))
            .cloned()
            .collect();
        if !keys.is_empty() {
            self.update_settings(|s| {
                for k in &keys {
                    s.remove(k);
                }
            });
        }
        self.toast("Camera and microphone choices forgotten");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_per_site_and_kind() {
        assert_eq!(
            capture_keys(&PermissionKind::CameraAndMicrophone, "a.b"),
            vec!["capture.a.b|camera", "capture.a.b|microphone"]
        );
        assert!(capture_keys(&PermissionKind::Geolocation, "a.b").is_empty());
        let keys = capture_keys(&PermissionKind::CameraAndMicrophone, "a.b");
        assert_eq!(remembered(&keys, |_| None), None);
        assert_eq!(
            remembered(&keys, |k| k.ends_with("camera").then_some(true)),
            None,
            "the microphone was never answered"
        );
        assert_eq!(remembered(&keys, |_| Some(true)), Some(true));
        assert_eq!(
            remembered(&keys, |k| Some(!k.ends_with("microphone"))),
            Some(false)
        );
        assert_eq!(remembered(&[], |_| Some(true)), None);
    }
}
