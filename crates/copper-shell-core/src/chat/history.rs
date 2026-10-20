//! The model's side of the conversation, in the chat-completions wire shape
//! (`Agent.sealed`, `Agent.arguments`, `Agent.emptied`): every tool call
//! answered before anything else, broken calls kept as JSON a provider
//! reads again.

use serde_json::{Map, Value, json};

use super::words;

/// Cut off this many replies running and the question ends: the model is
/// not finding a smaller step.
pub const CUT_LIMIT: u32 = 3;

/// One wire message (`{"role": …, …}`).
pub type Message = Map<String, Value>;

/// A message from its JSON.
pub fn message(value: Value) -> Message {
    match value {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn role(m: &Message) -> &str {
    m.get("role").and_then(Value::as_str).unwrap_or("")
}

/// The history with every tool call answered, the shape every provider
/// insists on: an assistant turn's tool calls are each followed, before
/// anything else, by a tool result with the same id. A call cut off by Stop
/// gets a result saying so; a result with no call before it goes.
pub fn sealed(messages: &[Message]) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        i += 1;
        if role(m) == "tool" {
            continue;
        }
        out.push(m.clone());
        let calls = match m.get("tool_calls").and_then(Value::as_array) {
            Some(c) if role(m) == "assistant" && !c.is_empty() => c,
            _ => continue,
        };
        let ids: Vec<&str> = calls
            .iter()
            .map(|c| c.get("id").and_then(Value::as_str).unwrap_or(""))
            .collect();
        let mut answered: Vec<String> = Vec::new();
        while i < messages.len() && role(&messages[i]) == "tool" {
            let id = messages[i]
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if ids.contains(&id) && !answered.iter().any(|a| a == id) {
                out.push(messages[i].clone());
                answered.push(id.to_string());
            }
            i += 1;
        }
        for id in ids {
            if !answered.iter().any(|a| a == id) {
                out.push(message(json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": words::CANCELLED,
                })));
                answered.push(id.to_string());
            }
        }
    }
    out
}

/// Why a call could not be run as the model sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Broken {
    /// The reply hit the output limit while this call was being written.
    CutOff,
    /// The arguments were not a JSON object.
    Invalid(String),
}

impl Broken {
    /// The tool result the model reads instead of the call's answer.
    pub fn for_model(&self) -> String {
        match self {
            Broken::CutOff => "Not run: your reply reached the output-token limit while you were still writing this call, so its arguments were cut off and nothing was done. Split the work into several smaller calls instead — at most 40 ops per canvas_apply — and carry on from where things stand now.".into(),
            Broken::Invalid(why) => format!("Not run: the arguments were not a valid JSON object ({why}), so nothing was done. Send the call again with its arguments as one JSON object; if it was a big batch, split it into several smaller calls."),
        }
    }
}

/// A call's arguments as the object the tool takes. Empty means `{}`;
/// anything else that is not one JSON object is broken, with why.
pub fn arguments(call: &Value) -> Result<Map<String, Value>, Broken> {
    let function = &call["function"];
    if let Some(object) = function["arguments"].as_object() {
        return Ok(object.clone());
    }
    let raw = function["arguments"].as_str().unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(Broken::Invalid("not an object".into())),
        Err(_) => {
            let count = raw.chars().count();
            let near = if count > 60 {
                let tail: String = raw.chars().skip(count - 40).collect();
                format!("…{tail}")
            } else {
                raw.to_string()
            };
            Err(Broken::Invalid(format!(
                "{count} characters, ending “{near}”"
            )))
        }
    }
}

/// The call as the history keeps it once it was not run: the same id and
/// name, `{}` for arguments, so the next request is one any provider reads.
pub fn emptied(call: &Value) -> Value {
    let mut call = call.clone();
    if let Some(f) = call.get_mut("function").and_then(Value::as_object_mut) {
        f.insert("arguments".into(), Value::String("{}".into()));
    }
    call
}

/// A message's text: a string, or the text parts of a list.
pub fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect(),
        _ => String::new(),
    }
}

/// Whether the reply stopped because it ran out of output tokens.
pub fn cut_off(finish: Option<&str>) -> bool {
    matches!(finish, Some("length" | "max_tokens"))
}

/// A reply that ends by announcing what comes next ("Let me do that
/// now:") is a turn that meant to call a tool.
pub fn announces(text: &str) -> bool {
    text.ends_with(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> Value {
        json!({"id": id, "type": "function", "function": {"name": "x", "arguments": "{}"}})
    }

    #[test]
    fn sealing_answers_every_call_and_drops_orphans() {
        let history = vec![
            message(json!({"role": "user", "content": "hi"})),
            message(json!({"role": "tool", "tool_call_id": "zz", "content": "orphan"})),
            message(json!({"role": "assistant", "tool_calls": [call("a"), call("b")]})),
            message(json!({"role": "tool", "tool_call_id": "b", "content": "B"})),
            message(json!({"role": "tool", "tool_call_id": "b", "content": "again"})),
        ];
        let out = sealed(&history);
        let roles: Vec<&str> = out.iter().map(role).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "tool"]);
        assert_eq!(out[2]["content"], "B");
        assert_eq!(out[3]["tool_call_id"], "a");
        assert_eq!(out[3]["content"], words::CANCELLED);
        assert_eq!(sealed(&out), out, "sealing is idempotent");
    }

    #[test]
    fn arguments_are_one_object_or_broken() {
        let args = |raw: &str| arguments(&json!({"function": {"name": "x", "arguments": raw}}));
        assert_eq!(args("").unwrap(), Map::new());
        assert_eq!(args(r#"{"a":1}"#).unwrap()["a"], 1);
        assert_eq!(args("[1]"), Err(Broken::Invalid("not an object".into())));
        let long = format!("{{\"ops\":[{}", "1,".repeat(40));
        match args(&long) {
            Err(Broken::Invalid(why)) => {
                assert!(why.starts_with("88 characters, ending “…"), "{why}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            arguments(&json!({"function": {"arguments": {"k": true}}})).unwrap()["k"],
            true
        );
        let e = emptied(
            &json!({"id": "c", "function": {"name": "canvas_apply", "arguments": "{\"ops\":["}}),
        );
        assert_eq!(e["function"]["arguments"], "{}");
        assert_eq!(e["id"], "c");
    }

    #[test]
    fn small_readers() {
        assert_eq!(
            text_of(Some(&json!([{"text": "a"}, {"type": "x"}, {"text": "b"}]))),
            "ab"
        );
        assert!(cut_off(Some("length")) && cut_off(Some("max_tokens")) && !cut_off(Some("stop")));
        assert!(announces("Let me do that now:") && !announces("Done."));
    }
}
