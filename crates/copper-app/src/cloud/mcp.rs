//! Contract used by the agents lane for `cloud_history_delete`. Signature fixed.
use crate::shell::Shell;
use serde_json::{Map, Value};

/// Returns the tool's text result.
pub async fn history_delete(_shell: Shell, _args: Map<String, Value>) -> Result<String, String> {
    Err("Copper Cloud isn't linked".into())
}
