//! Contract used by the agents lane for `canvas_*` tools. Signature fixed.
use crate::shell::Shell;
use serde_json::{Map, Value};

/// `tool` is the full tool name (e.g. `canvas_read`). Returns JSON content items
/// as the Mac does (a JSON value per content item; text items as strings).
pub async fn call(_shell: Shell, _tool: String, _args: Map<String, Value>) -> Result<Vec<Value>, String> {
    Err("Canvas isn't available in this build yet".into())
}
