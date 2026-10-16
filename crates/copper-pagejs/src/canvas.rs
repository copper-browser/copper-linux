//! The host side of the canvas page bridge (Fork/Canvas/CanvasHost.swift,
//! CanvasFrames.swift, CanvasChat*.swift, CanvasLean.swift): calls into
//! `window.copperCanvas` and the two user scripts the board uses. The canvas
//! page itself (`canvas.html`) is a bundled resource, not page JS.

use serde::Serialize;
use serde_json::{Value, json};

use crate::call::{Call, UserScript};
use crate::script::Script;

/// Live web frames on a board report where they are to `copperFrame`, from
/// their own isolated world (`FRAME_WORLD`): document start, all frames.
pub fn frame_reporter() -> UserScript {
    UserScript::new(Script::CanvasFrameReporter, json!({}))
}

/// The lean board's typing relay (`officeForms` `focus`): document start, main frame.
pub fn typing() -> UserScript {
    UserScript::new(Script::CanvasTyping, json!({}))
}

/// Hand a Yjs update (base64) to the page.
pub fn apply_update(update_b64: &str) -> Call<Value> {
    Call::new(Script::CanvasApplyUpdate, json!({ "u": update_b64 }))
}

/// Start the page: `init(options)`, replay `updates`, theme, title, ticket.
pub fn ready(
    options: &impl Serialize,
    updates: &[String],
    theme: &str,
    title: &str,
    ticket: &str,
) -> Call<bool> {
    Call::new(
        Script::CanvasReady,
        json!({ "options": options, "updates": updates, "theme": theme, "title": title, "ticket": ticket }),
    )
}

pub fn theme(theme: &str) -> Call<Value> {
    Call::new(Script::CanvasTheme, json!({ "t": theme }))
}

/// Whether the page's bridge is up (`adoptIfLive`).
pub fn is_live() -> Call<bool> {
    Call::new(Script::CanvasIsLive, json!({}))
}

/// `canvas_apply`: `input` is the JSON text `{"ops": […], "as": …}`; answers JSON text.
pub fn apply(input: &str) -> Call<String> {
    Call::new(Script::CanvasApply, json!({ "input": input }))
}

/// `canvas_read`: answers JSON text.
pub fn read(full: bool) -> Call<String> {
    read_with(&json!({ "full": full }))
}

/// `canvas_read` with the paging / narrowing options (`full`, `overview`,
/// `frame`, `region`, `ids`, `types`, `query`, `limit`, `cursor`), handed to
/// `copperCanvas.read` as one JSON string. A page that predates an option
/// ignores it. Answers JSON text.
pub fn read_with(opts: &Value) -> Call<String> {
    Call::new(Script::CanvasRead, json!({ "opts": opts.to_string() }))
}

/// A live frame's poster (`copperCanvas.framePoster(name, dataUrl | null)`):
/// `data_url` is a `data:image/jpeg;base64,…` picture of the frame, or `None`
/// when it could not be taken. Answers whether the page has `framePoster`.
pub fn frame_poster(name: &str, data_url: Option<&str>) -> Call<bool> {
    Call::new(
        Script::CanvasFramePoster,
        json!({ "name": name, "url": data_url }),
    )
}

pub fn select(ids: &[String]) -> Call<Value> {
    Call::new(Script::CanvasSelect, json!({ "ids": ids }))
}

pub fn zoom_to(ids: &[String]) -> Call<Value> {
    Call::new(Script::CanvasZoomTo, json!({ "ids": ids }))
}

/// Who is on the board: JSON text of an array.
pub fn presence() -> Call<String> {
    Call::new(Script::CanvasPresence, json!({}))
}

/// The document state for a checkpoint (`null` when the page can't export).
pub fn export_state() -> Call<Value> {
    Call::new(Script::CanvasExportState, json!({}))
}

pub fn ws_closed() -> Call<Value> {
    Call::new(Script::CanvasWsClosed, json!({}))
}

pub fn ws_open() -> Call<Value> {
    Call::new(Script::CanvasWsOpen, json!({}))
}

/// One relay frame (base64) for the page's socket.
pub fn ws_message(frame_b64: &str) -> Call<Value> {
    Call::new(Script::CanvasWsMessage, json!({ "f": frame_b64 }))
}

pub fn set_share(share: &impl Serialize) -> Call<Value> {
    Call::new(Script::CanvasSetShare, json!({ "s": share }))
}

pub fn set_status(status: &impl Serialize) -> Call<Value> {
    Call::new(Script::CanvasSetStatus, json!({ "s": status }))
}

/// Retitle the board; answers whether the page took it.
pub fn rename(title: &str) -> Call<bool> {
    Call::new(Script::CanvasRename, json!({ "t": title }))
}

pub fn frame_event(event: &impl Serialize) -> Call<bool> {
    Call::new(Script::CanvasFrameEvent, json!({ "e": event }))
}

/// Whether this host reports its live frames' loads (`{reports: true}`
/// once the frame reporter is installed): the page then takes a frame
/// that never reports as one that didn't load.
pub fn frame_host(reports: &impl Serialize) -> Call<Value> {
    Call::new(
        Script::CanvasFrameHost,
        json!({ "f": { "reports": reports } }),
    )
}

pub fn set_chat(state: &impl Serialize) -> Call<Value> {
    Call::new(Script::CanvasSetChat, json!({ "s": state }))
}

pub fn show_chat(open: bool) -> Call<Value> {
    Call::new(Script::CanvasShowChat, json!({ "o": open }))
}

/// Open the chat at one message.
pub fn reveal_chat(message_id: &str) -> Call<Value> {
    Call::new(
        Script::CanvasRevealChat,
        json!({ "t": { "id": message_id } }),
    )
}

/// `canvas_chat`: `input` is the JSON text `{"text": …, "mentions": […]}`;
/// answers JSON text `{ok, error?}`.
pub fn chat_send(input: &str) -> Call<Value> {
    Call::new(Script::CanvasChatSend, json!({ "input": input }))
}
