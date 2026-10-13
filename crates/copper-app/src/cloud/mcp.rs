//! `cloud_history_delete` for the agents lane (Mac: `CloudHistoryTool`). The
//! signature is a fixed cross-lane contract.
//!
//! Deletes the account's history on Copper Cloud — every device's rows — by
//! time, site, page or device, or all of it; history here is untouched. The
//! agents lane lists the tool only to loopback clients (as the Mac does).

use crate::shell::Shell;
use copper_cloud::sync::delete;
use serde_json::{Map, Value};

/// Returns the tool's text result: `{deleted, matched, looked, scanned,
/// requests, complete, what}` as compact JSON with sorted keys.
pub async fn history_delete(_shell: Shell, args: Map<String, Value>) -> Result<String, String> {
    let cloud = super::get().ok_or("Copper Cloud isn't linked")?;
    if !cloud.client.is_linked() {
        return Err("Copper Cloud isn't linked — connect at Settings › Cloud".into());
    }
    if !cloud.client.is_signed_in() {
        return Err("Not signed in to Copper Cloud — sign in at Settings › Cloud".into());
    }
    let me = cloud.client.account().map(|a| a.device_id);
    let selector = delete::selector_from(
        &Value::Object(args),
        copper_core::date::now_unix(),
        me.as_deref(),
    )
    .map_err(|f| f.message)?;
    let what = delete::words(&selector);
    let sync = cloud.sync.clone();
    let outcome = super::spawn(async move { sync.delete_history(&selector).await })
        .await
        .map_err(|_| "Stopped — nothing was deleted".to_string())?
        .map_err(|f| f.message)?;
    let mut json = outcome.json();
    if let Value::Object(map) = &mut json {
        map.insert("what".into(), Value::String(what));
    }
    Ok(sorted(&json))
}

/// JSON with object keys sorted (the Mac's `.sortedKeys`).
fn sorted(value: &Value) -> String {
    fn walk(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), walk(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(walk).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&walk(value)).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_sorted() {
        let v = serde_json::json!({"b": 1, "a": {"d": 2, "c": 3}});
        assert_eq!(sorted(&v), r#"{"a":{"c":3,"d":2},"b":1}"#);
    }
}
