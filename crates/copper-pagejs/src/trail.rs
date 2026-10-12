//! The cosmetic trail layer that shows the agent's moves (`window.__jevTrail`,
//! Fork/MCP/Trail.swift). Every call is fire-and-forget; only [`glide`] is
//! awaited, capped at 400 ms by the caller.

use serde_json::{Value, json};

use crate::call::Call;
use crate::script::Script;

/// The layer's one colour.
pub const ACCENT: &str = "#c8743c";

/// Outline the element about to be acted on and name the move over it.
/// `rect` is `{x,y,w,h}` in viewport CSS pixels; `operation` is CLICK /
/// TYPE_TEXT / SELECT / HOVER / DRAG.
pub fn target(x: f64, y: f64, w: f64, h: f64, label: &str, operation: &str) -> Call<Value> {
    fire(
        "target",
        json!({ "rect": { "x": x, "y": y, "w": w, "h": h }, "label": label, "operation": operation }),
    )
}

/// A numbered dot at the click point.
pub fn click(x: f64, y: f64) -> Call<Value> {
    fire("click", json!({ "x": x, "y": y }))
}

/// What was written, as a plate under the field.
pub fn typed(x: f64, y: f64, text: &str) -> Call<Value> {
    fire("typed", json!({ "x": x, "y": y, "text": text }))
}

/// A chevron at the edge the page moved towards.
pub fn scroll(delta: f64) -> Call<Value> {
    fire("scroll", json!({ "delta": delta }))
}

/// A pulse over every control the read gave Jev (at most 250 rects of `{x,y,w,h}`).
pub fn seen(rects: &[Value]) -> Call<Value> {
    let rects: Vec<&Value> = rects.iter().take(250).collect();
    fire("seen", json!({ "rects": rects }))
}

/// The pointer breathes while nothing happens.
pub fn wait() -> Call<Value> {
    fire("wait", json!({}))
}

fn fire(name: &'static str, args: Value) -> Call<Value> {
    Call::new(Script::TrailFire, json!({ "name": name, "args": args }))
}

/// Take the pointer to (x, y); resolves when it arrives.
pub fn glide(x: f64, y: f64) -> Call<Value> {
    Call::new(Script::TrailGlide, json!({ "a": { "x": x, "y": y } }))
}

/// The run is over: the layer and its listeners come off the page.
pub fn clear() -> Call<bool> {
    Call::new(Script::TrailClear, json!({}))
}
