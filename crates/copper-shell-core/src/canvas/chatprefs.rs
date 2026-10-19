//! What chat remembers per canvas and person (Mac: `CanvasChat.Saved` in
//! `CanvasChat.swift`, the same `canvas/chat.json`): whether the panel is
//! open, and the read mark (`readId`, `readAt`) the page reports with
//! `chatRead` — so unread counts survive a reload and a restart, and every
//! tab on a canvas shows the panel the same way.
//!
//! `{prefs: {<account>: {<canvas>: {open?, readId?, readAt?}}}, members: {…}}`;
//! `members` is the Mac's cache and is kept as it is.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pref {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<bool>,
    #[serde(default, rename = "readId", skip_serializing_if = "Option::is_none")]
    pub read_id: Option<String>,
    #[serde(default, rename = "readAt", skip_serializing_if = "Option::is_none")]
    pub read_at: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default)]
    pub prefs: BTreeMap<String, BTreeMap<String, Pref>>,
    /// The Mac's members cache; carried through untouched.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub members: Value,
    /// Anything else the Mac writes there.
    #[serde(flatten)]
    pub rest: BTreeMap<String, Value>,
}

thread_local! {
    static SAVED: RefCell<Option<Saved>> = const { RefCell::new(None) };
}

fn path() -> Option<PathBuf> {
    super::data_dir().map(|d| d.join("chat.json"))
}

fn with<R>(f: impl FnOnce(&mut Saved) -> R) -> R {
    SAVED.with(|cell| {
        let mut slot = cell.borrow_mut();
        let saved = slot.get_or_insert_with(|| {
            path()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        });
        f(saved)
    })
}

/// The pref of `canvas` for `account`.
pub fn get(account: &str, canvas: &str) -> Pref {
    with(|s| {
        s.prefs
            .get(account)
            .and_then(|m| m.get(canvas))
            .cloned()
            .unwrap_or_default()
    })
}

/// Change the pref of `canvas` for `account`; written at once when it changed.
pub fn update(account: &str, canvas: &str, f: impl FnOnce(&mut Pref)) {
    let changed = with(|s| {
        let pref = s
            .prefs
            .entry(account.to_string())
            .or_default()
            .entry(canvas.to_string())
            .or_default();
        let before = pref.clone();
        f(pref);
        *pref != before
    });
    if changed {
        save();
    }
}

fn save() {
    let Some(p) = path() else { return };
    let Ok(bytes) = with(|s| serde_json::to_vec(s)) else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = copper_core::store::write_atomic(&p, &bytes, true, false) {
        tracing::debug!(target: "copper_app::canvas", error = %e, "chat.json");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_the_mac_shape() {
        let mac = r#"{"prefs":{"u1":{"c1":{"open":true,"readId":"m9","readAt":1760000000000}}},"members":{"c1":[{"id":"u2","name":"Ann","email":"a@x"}]},"askedToNotify":true}"#;
        let s: Saved = serde_json::from_str(mac).unwrap();
        assert_eq!(s.prefs["u1"]["c1"].read_id.as_deref(), Some("m9"));
        assert_eq!(s.prefs["u1"]["c1"].open, Some(true));
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["askedToNotify"], Value::Bool(true));
        assert_eq!(back["members"]["c1"][0]["name"], "Ann");
        assert_eq!(back["prefs"]["u1"]["c1"]["readAt"], 1760000000000.0);
        let empty: Saved = serde_json::from_str("{}").unwrap();
        assert_eq!(serde_json::to_string(&empty).unwrap(), r#"{"prefs":{}}"#);
    }
}
