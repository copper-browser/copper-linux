//! A site's camera and microphone answers, remembered per site (Mac
//! `Browser.swift` "the camera and the microphone", `forgetCaptureChoices`)
//! so a call you take every week asks once.
//!
//! Kept in settings as `capture.{host}|camera` and `capture.{host}|microphone`
//! (the Mac's prefix; one key per kind, so the two are remembered apart). A
//! private tab reads and writes none of them. The same keys as Windows'
//! `chrome/permit.rs`, which can switch to these once it bumps shell-core.

use std::cell::Cell;

use crate::{Platform, Shell};
use copper_engine::{GeoPosition, PageView, PermissionKind};

/// Sites an agent granted geolocation (`browser_permissions`; the Mac's key).
pub const GEO_SITES: &str = "permissions.geolocation";

thread_local! {
    /// The position granted pages are told: one for every site, from the
    /// last grant that named one (the Mac's; not kept across launches).
    static POSITION: Cell<Option<GeoPosition>> = const { Cell::new(None) };
}

/// The camera / microphone answers' host for a permissions site
/// (`host[:port]`): the shell keys them by host, no port, no `www.`.
pub fn capture_host(site: &str) -> String {
    let host = match site.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => site,
    };
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// A page's site as `browser_permissions` names it: host, and the port when
/// the address gives one.
pub fn site_of_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str().filter(|h| !h.is_empty())?;
    Some(copper_mcp::page_tools::site_of(host, u.port()))
}

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

    /// Whether `site` (`host[:port]`) may use `kind` (`geolocation`,
    /// `camera`, `microphone`) without asking (`Permissions.state`).
    pub fn site_granted(&self, kind: &str, site: &str) -> bool {
        match kind {
            "geolocation" => self.geo_sites().iter().any(|s| s == site),
            "camera" => {
                self.capture_choice(&PermissionKind::Camera, &capture_host(site)) == Some(true)
            }
            "microphone" => {
                self.capture_choice(&PermissionKind::Microphone, &capture_host(site)) == Some(true)
            }
            _ => false,
        }
    }

    /// Allow `kind` for `site`, or forget it so the site is asked again.
    pub fn set_site_granted(&self, kind: &str, site: &str, allowed: bool) {
        match kind {
            "geolocation" => {
                let mut sites = self.geo_sites();
                sites.retain(|s| s != site);
                if allowed {
                    sites.push(site.to_string());
                }
                sites.sort();
                self.update_settings(|s| {
                    if sites.is_empty() {
                        s.remove(GEO_SITES);
                    } else {
                        s.set(GEO_SITES, serde_json::json!(sites));
                    }
                });
            }
            "camera" | "microphone" => {
                let key = format!("{CAPTURE}{}|{kind}", capture_host(site));
                self.update_settings(|s| {
                    if allowed {
                        s.set(&key, true);
                    } else {
                        s.remove(&key);
                    }
                });
            }
            _ => {}
        }
    }

    fn geo_sites(&self) -> Vec<String> {
        self.settings()
            .get(GEO_SITES)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether a page at `url` is granted `kind` before anyone is asked: a
    /// site given geolocation, or allowed the camera / microphone (not in a
    /// private tab, which keeps no answers).
    pub fn grants(&self, kind: &PermissionKind, url: Option<&str>, private: bool) -> bool {
        let Some(site) = url.and_then(site_of_url) else {
            return false;
        };
        match kind {
            PermissionKind::Geolocation => self.site_granted("geolocation", &site),
            PermissionKind::Camera
            | PermissionKind::Microphone
            | PermissionKind::CameraAndMicrophone => {
                !private && self.capture_choice(kind, &capture_host(&site)) == Some(true)
            }
            _ => false,
        }
    }

    /// The position granted pages are told.
    pub fn position(&self) -> Option<GeoPosition> {
        POSITION.with(Cell::get)
    }

    /// Tell every page `position` from now on.
    pub fn set_position(&self, position: Option<GeoPosition>) {
        POSITION.with(|p| p.set(position));
        let pages: Vec<_> = self
            .pages_map()
            .borrow()
            .values()
            .map(|s| s.page.clone())
            .collect();
        for page in pages {
            page.set_geolocation(position);
        }
    }

    /// Hand a new page the grants and the position (`adopt_page`).
    pub(crate) fn dress_permissions(&self, page: &crate::PageOf<P>, private: bool) {
        let weak = self.downgrade();
        page.set_permission_handler(Some(Box::new(move |kind, url| {
            weak.upgrade().is_some_and(|s| s.grants(kind, url, private))
        })));
        if let Some(position) = self.position() {
            page.set_geolocation(Some(position));
        }
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

    #[test]
    fn permission_sites() {
        assert_eq!(capture_host("www.example.com:8080"), "example.com");
        assert_eq!(capture_host("example.com"), "example.com");
        assert_eq!(
            site_of_url("https://WWW.Example.com:8443/x").as_deref(),
            Some("www.example.com:8443")
        );
        assert_eq!(
            site_of_url("https://example.com/").as_deref(),
            Some("example.com")
        );
        assert_eq!(site_of_url("about:blank"), None);
    }
}
