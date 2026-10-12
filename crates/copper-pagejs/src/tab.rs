//! Tab plumbing: reading mode (Reader.swift), scroll/swipe/image relays,
//! favicons, smart zoom, find's selection, readiness, localStorage import.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::call::{Call, UserScript};
use crate::script::Script;

/// Reading mode's outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReaderOutcome {
    /// The page now shows its article (`#office-reader`).
    Read,
    /// Nothing on the page reads like an article; the page is untouched.
    None,
}

/// Replace the page with its article (`Reader.script`); reload to leave.
pub fn reader() -> Call<ReaderOutcome> {
    Call::new(Script::ReaderArticle, json!({}))
}

/// Scroll position to `officeScroll` (`{y, max}`): document end, main frame.
pub fn scroll_relay() -> UserScript {
    UserScript::new(Script::ScrollRelay, json!({}))
}

/// `overscroll-behavior-y: none`: document start, main frame.
pub fn swipe_calm() -> UserScript {
    UserScript::new(Script::SwipeCalm, json!({}))
}

/// Whether a sideways wheel is the page's (`officeScroll` `{side}`): all frames.
pub fn swipe_watch() -> UserScript {
    UserScript::new(Script::SwipeWatch, json!({}))
}

/// Right-click on an image to `officeImages` (`{src}`): all frames.
pub fn image_relay() -> UserScript {
    UserScript::new(Script::ImageRelay, json!({}))
}

/// A `<link rel=…icon…>` the page declares (`Favicons.probe`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IconLink {
    pub href: String,
    pub rel: String,
    pub sizes: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub media: String,
}

pub fn favicons() -> Call<Vec<IconLink>> {
    Call::new(Script::Favicons, json!({}))
}

/// Closing find: drop the highlighted selection.
pub fn clear_selection() -> Call<bool> {
    Call::new(Script::ClearSelection, json!({}))
}

/// `document.readyState`.
pub fn ready_state() -> Call<String> {
    Call::new(Script::ReadyState, json!({}))
}

pub fn scroll_to(x: f64, y: f64) -> Call<bool> {
    Call::new(Script::ScrollTo, json!({ "x": x, "y": y }))
}

/// Where a smart zoom lands: the new scale and the scroll offset.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Zoom {
    pub scale: f64,
    pub x: f64,
    pub y: f64,
}

/// A double-tap at view point (x, y) at `scale` in a view `width` wide: a JSON
/// string (see [`parse_zoom`]) or `None` when nothing is under the point.
pub fn smart_zoom(x: f64, y: f64, scale: f64, width: f64) -> Call<Option<String>> {
    Call::new(
        Script::SmartZoom,
        json!({ "x": x, "y": y, "scale": scale, "width": width }),
    )
}

pub fn parse_zoom(json: &str) -> Result<Zoom, serde_json::Error> {
    serde_json::from_str(json)
}

/// `location.origin`.
pub fn origin() -> Call<String> {
    Call::new(Script::Origin, json!({}))
}

/// Put localStorage items; answers `"key: ErrorName"` for each that failed.
pub fn storage_put(items: &BTreeMap<String, String>) -> Call<Vec<String>> {
    Call::new(Script::StoragePut, json!({ "items": items }))
}
