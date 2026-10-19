//! `copper-addon://<id>/<path>` (contract §5.4): what the slot view's
//! scheme handler answers, the same on every platform. Files come only from
//! the folder of the addon currently in the slot; `/` is the manifest's
//! `entry`; anything else is a 404. HTML gets the CSP `<meta>`
//! ([`super::csp`]); every answer also lists the policy as a header for
//! platforms that can send headers.

use super::bundle::valid_path;
use super::csp::{CSP, inject_csp_bytes};
use super::state::ActiveAddon;
use std::fs;

/// The scheme name.
pub const SCHEME: &str = "copper-addon";

/// `copper-addon://<id>/<path>` (`path` without a leading `/`).
pub fn addon_url(id: &str, path: &str) -> String {
    format!("{SCHEME}://{id}/{}", path.trim_start_matches('/'))
}

/// `copper-addon://<id>/`, which serves the entry.
pub fn root_url(id: &str) -> String {
    format!("{SCHEME}://{id}/")
}

/// `(id, path)` of a `copper-addon:` URL; query and fragment are dropped,
/// the path has no leading `/` (empty = the entry).
pub fn parse_addon_url(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let (id, path) = match rest.split_once('/') {
        Some((id, path)) => (id, path),
        None => (rest, ""),
    };
    if id.is_empty() {
        return None;
    }
    Some((id.to_string(), path.to_string()))
}

/// Whether `url` belongs to addon `id` (`copper-addon://<id>/…`): the check
/// for "messages only from the slot's own origin" and for navigations that
/// stay inside the view.
pub fn is_addon_origin(url: &str, id: &str) -> bool {
    parse_addon_url(url).is_some_and(|(host, _)| host == id)
}

/// One answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemeResponse {
    pub status: u16,
    /// `Content-Type`.
    pub mime: &'static str,
    pub body: Vec<u8>,
    /// For platforms that can send headers: the CSP and `nosniff`.
    pub headers: Vec<(&'static str, String)>,
}

impl SchemeResponse {
    fn new(status: u16, mime: &'static str, body: Vec<u8>) -> Self {
        SchemeResponse {
            status,
            mime,
            body,
            headers: vec![
                ("Content-Security-Policy", CSP.to_string()),
                ("X-Content-Type-Options", "nosniff".to_string()),
                ("Cache-Control", "no-cache".to_string()),
            ],
        }
    }

    fn not_found() -> Self {
        SchemeResponse::new(404, "text/plain; charset=utf-8", b"Not found".to_vec())
    }
}

/// `Content-Type` by extension.
pub fn mime_for(path: &str) -> &'static str {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" | "md" => "text/plain; charset=utf-8",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Answer `uri` for the addon in the slot. Another addon's id, a path that
/// breaks the bundle path rule, a folder, a symlink (outside a developer
/// folder) or a missing file is a 404.
pub fn serve(active: &ActiveAddon, uri: &str) -> SchemeResponse {
    let Some((id, path)) = parse_addon_url(uri) else {
        return SchemeResponse::not_found();
    };
    if id != active.installed.id {
        return SchemeResponse::not_found();
    }
    let path = if path.is_empty() {
        active.manifest.entry.clone()
    } else {
        path
    };
    if !valid_path(&path) {
        return SchemeResponse::not_found();
    }
    let file = active.dir.join(&path);
    let Ok(meta) = fs::symlink_metadata(&file) else {
        return SchemeResponse::not_found();
    };
    let plain = meta.is_file() || (active.dev && file.is_file());
    if !plain {
        return SchemeResponse::not_found();
    }
    let Ok(body) = fs::read(&file) else {
        return SchemeResponse::not_found();
    };
    let mime = mime_for(&path);
    let body = if mime.starts_with("text/html") {
        inject_csp_bytes(&body)
    } else {
        body
    };
    SchemeResponse::new(200, mime, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::manifest::Manifest;
    use crate::addons::slot::SlotId;
    use crate::addons::state::Installed;
    use crate::addons::testkit;

    #[test]
    fn urls() {
        assert_eq!(
            parse_addon_url("copper-addon://example.sidebar/js/a.js?x=1#y"),
            Some(("example.sidebar".into(), "js/a.js".into()))
        );
        assert_eq!(
            parse_addon_url("copper-addon://example.sidebar"),
            Some(("example.sidebar".into(), String::new()))
        );
        assert_eq!(parse_addon_url("https://example.com/"), None);
        assert!(is_addon_origin("copper-addon://a.b/", "a.b"));
        assert!(!is_addon_origin("copper-addon://a.bc/", "a.b"));
        assert_eq!(addon_url("a.b", "/x.js"), "copper-addon://a.b/x.js");
        assert_eq!(root_url("a.b"), "copper-addon://a.b/");
    }

    #[test]
    fn serves_only_the_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("index.html"), b"<head></head><p>hi</p>").unwrap();
        std::fs::create_dir(tmp.path().join("js")).unwrap();
        std::fs::write(tmp.path().join("js/a.js"), b"1").unwrap();
        let manifest =
            Manifest::from_value(testkit::manifest_json("example.sidebar", "1.0.0")).unwrap();
        let active = ActiveAddon {
            slot: SlotId::AgentSidebar,
            installed: Installed {
                id: "example.sidebar".into(),
                ..Default::default()
            },
            manifest,
            dir: tmp.path().to_path_buf(),
            dev: false,
        };
        let root = serve(&active, "copper-addon://example.sidebar/");
        assert_eq!(root.status, 200);
        assert_eq!(root.mime, "text/html; charset=utf-8");
        assert!(
            String::from_utf8(root.body)
                .unwrap()
                .starts_with("<head><meta http-equiv=\"Content-Security-Policy\"")
        );
        let js = serve(&active, "copper-addon://example.sidebar/js/a.js");
        assert_eq!(
            (js.status, js.mime, js.body),
            (200, "text/javascript; charset=utf-8", b"1".to_vec())
        );
        for bad in [
            "copper-addon://other.addon/index.html",
            "copper-addon://example.sidebar/missing.js",
            "copper-addon://example.sidebar/js",
            "copper-addon://example.sidebar/../secret",
            "copper-addon://example.sidebar/js/%2e%2e/x",
            "https://example.sidebar/index.html",
        ] {
            assert_eq!(serve(&active, bad).status, 404, "{bad}");
        }
    }
}
