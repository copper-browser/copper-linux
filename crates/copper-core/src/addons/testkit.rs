//! Fixtures for tests and self-tests (here and in the platforms): a valid
//! manifest, a served-format bundle built from files, and the `/v1/addons`
//! entry that matches it. Not part of the stable API.

use super::bundle::sha256_hex;
use super::resolved::ResolvedAddon;
use base64::Engine as _;
use serde_json::{Value, json};

/// A valid `copper-addon/1` manifest for the agent sidebar: entry
/// `index.html`, every capability, host `api.example.com`, one setting
/// (`apiUrl`).
pub fn manifest_json(id: &str, version: &str) -> Value {
    json!({
        "schema": "copper-addon/1",
        "id": id,
        "name": "Example sidebar",
        "version": version,
        "slot": "agent-sidebar",
        "entry": "index.html",
        "description": "A test addon.",
        "author": "Example",
        "bridge": 1,
        "permissions": {
            "hosts": ["api.example.com"],
            "capabilities": ["identity", "storage", "fetch", "oauth", "open-url", "page", "agent"]
        },
        "settings": {
            "apiUrl": {"type": "string", "default": "https://api.example.com/v1", "title": "API"}
        }
    })
}

/// Served-format bytes (contract §3) for `manifest` and `files`: files
/// sorted by path, compact JSON — what the cloud serves.
pub fn served_bundle(manifest: &Value, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut sorted: Vec<&(&str, &[u8])> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let files: Vec<Value> = sorted
        .into_iter()
        .map(|(path, data)| {
            json!({
                "path": path,
                "size": data.len(),
                "sha256": sha256_hex(data),
                "data": base64::engine::general_purpose::STANDARD.encode(data),
            })
        })
        .collect();
    serde_json::to_vec(&json!({
        "format": super::bundle::FORMAT,
        "manifest": manifest,
        "files": files,
    }))
    .expect("JSON")
}

/// The `/v1/addons` entry for `manifest` served as `bytes` (settings at
/// their defaults).
pub fn resolved_for(manifest: &Value, bytes: &[u8]) -> ResolvedAddon {
    let sha = sha256_hex(bytes);
    let id = manifest["id"].as_str().unwrap_or_default().to_string();
    let settings: serde_json::Map<String, Value> = manifest["settings"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, s)| s.get("default").map(|d| (k.clone(), d.clone())))
                .collect()
        })
        .unwrap_or_default();
    ResolvedAddon {
        id: id.clone(),
        slot: manifest["slot"].as_str().unwrap_or_default().to_string(),
        name: manifest["name"].as_str().unwrap_or_default().to_string(),
        version: manifest["version"].as_str().unwrap_or_default().to_string(),
        sha256: sha.clone(),
        size: bytes.len() as u64,
        entry: manifest["entry"].as_str().unwrap_or_default().to_string(),
        bridge: manifest["bridge"].as_u64().unwrap_or(1) as u32,
        platforms: manifest["platforms"].as_array().map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        }),
        permissions: serde_json::from_value(manifest["permissions"].clone()).unwrap_or_default(),
        settings: Value::Object(settings),
        bundle: format!("/v1/addons/{id}/bundle/{sha}"),
        updated_at: Some("2026-10-19T00:00:00Z".into()),
        extra: Default::default(),
    }
}

/// A `/v1/addons` body listing `addons` at `revision`.
pub fn resolved_body(revision: &str, addons: &[ResolvedAddon]) -> Vec<u8> {
    serde_json::to_vec(&json!({"revision": revision, "addons": addons})).expect("JSON")
}
