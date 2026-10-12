//! Contract used by the agents lane for `browser_sign_in` / `browser_autofill`.
//! The creds-ui lane fills these in; signatures are fixed.
use crate::shell::Shell;
use copper_core::TabId;
use serde_json::{Map, Value};

pub async fn sign_in(_shell: Shell, _tab: TabId, _args: Map<String, Value>) -> Result<Value, String> {
    Err("Signing in isn't available in this build yet".into())
}

pub async fn autofill(_shell: Shell, _tab: TabId, _args: Map<String, Value>) -> Result<Value, String> {
    Err("Autofill isn't available in this build yet".into())
}
