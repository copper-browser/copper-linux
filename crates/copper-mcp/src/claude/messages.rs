//! Anthropic's Messages API as the Claude-account lane calls it (Mac:
//! `Fork/Claude.swift`, `PromptCache.messages`): Claude Code's headers and
//! identity block, which an account token needs, the agent's chat-shaped
//! transcript translated at the boundary, and the prompt-cache marks.
//!
//! Bodies are written with sorted keys (serde_json's map), so the same
//! history is the same bytes every turn, which the cache marks need.

use serde_json::{Map, Value, json};

/// `POST` here.
pub const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
/// The first system block, exactly: an account token is Claude Code's.
pub const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
pub const USER_AGENT: &str = "claude-cli/2.1.283";
pub const BETAS: &str = "claude-code-20250219,oauth-2025-04-20";
/// Anthropic's ceiling on cache marks in one request.
const MARK_LIMIT: usize = 4;

/// Which prompt-cache marks a body gets (`PromptCache.Marks`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marks {
    None,
    /// The system prompt and the last tool: a one-shot ask.
    Prefix,
    /// Those, plus the last message and the previous request's last one:
    /// an agent turn whose conversation comes back next turn.
    Rolling,
}

/// Why a request to the Claude-account lane failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No usable sign-in: not signed in, or the refresh didn't work.
    Account(String),
    /// No answer within the timeout.
    TimedOut,
    /// The request never got an answer.
    Network(String),
    /// Anthropic answered with this status.
    Http { status: u16, text: String },
}

impl Failure {
    /// In words, as the Mac says it.
    pub fn text(&self) -> String {
        match self {
            Failure::Account(t) => t.clone(),
            Failure::TimedOut => "Claude: the request timed out".into(),
            Failure::Network(t) => format!("Claude: {t}"),
            Failure::Http { text, .. } => text.clone(),
        }
    }
}

/// A non-200 answer in words (`Claude.failure`).
pub fn failure(status: u16, data: &[u8]) -> Failure {
    let text = match status {
        401 => "Claude sign-in expired or was revoked — sign in again in Settings › Intelligence"
            .to_string(),
        429 => "Claude is rate-limiting this account right now — try again in a moment".into(),
        503 | 529 => "Claude is overloaded right now".into(),
        _ => {
            let said = serde_json::from_slice::<Value>(data)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string));
            format!(
                "Claude {status}: {}",
                said.unwrap_or_else(|| prefix(data, 300))
            )
        }
    };
    Failure::Http { status, text }
}

/// The first `n` bytes as text.
pub(crate) fn prefix(data: &[u8], n: usize) -> String {
    String::from_utf8_lossy(&data[..data.len().min(n)]).into_owned()
}

/// The system blocks: the identity first and untouched, then `system`.
pub fn system(text: &str) -> Vec<Value> {
    let mut blocks = vec![json!({ "type": "text", "text": IDENTITY })];
    if !text.is_empty() {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    blocks
}

/// A Messages body (`Claude.body`). No `temperature`, no `thinking` and no
/// forced tool: Opus 5.5 refuses all three, and `auto` is what every
/// Claude model takes.
pub fn body(
    model: &str,
    mut system: Vec<Value>,
    mut messages: Vec<Value>,
    mut tools: Vec<Value>,
    max_tokens: u32,
    marks: Marks,
) -> Value {
    mark(&mut system, &mut messages, &mut tools, marks);
    let mut out = json!({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": messages,
    });
    if !tools.is_empty() {
        out["tools"] = Value::Array(tools);
        out["tool_choice"] = json!({ "type": "auto" });
    }
    out
}

/// The marks (`PromptCache.messages`): the last system block after the
/// identity, the last tool, and for `Rolling` the user message before the
/// last assistant reply and the last message.
fn mark(system: &mut [Value], messages: &mut [Value], tools: &mut [Value], marks: Marks) {
    if marks == Marks::None {
        return;
    }
    let mut used = 0;
    if system.len() > 1
        && let Some(last) = system.last_mut().and_then(Value::as_object_mut)
        && markable(last)
    {
        last.insert("cache_control".into(), ephemeral());
        used += 1;
    }
    if let Some(last) = tools.last_mut().and_then(Value::as_object_mut) {
        last.insert("cache_control".into(), ephemeral());
        used += 1;
    }
    if marks != Marks::Rolling {
        return;
    }
    for m in messages.iter_mut() {
        if role(m) == "user" {
            *m = as_parts(m);
        }
    }
    let roles: Vec<&str> = messages.iter().map(role).collect();
    let mut at = Vec::new();
    if let Some(reply) = roles.iter().rposition(|r| *r == "assistant")
        && reply > 0
        && roles[reply - 1] == "user"
    {
        at.push(reply - 1);
    }
    if roles.last() == Some(&"user") {
        at.push(roles.len() - 1);
    }
    for i in at {
        if used >= MARK_LIMIT {
            break;
        }
        if let Some(m) = marked(&messages[i]) {
            messages[i] = m;
            used += 1;
        }
    }
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

fn role(m: &Value) -> &str {
    m["role"].as_str().unwrap_or("")
}

/// A message whose content is a non-empty string, as one text block.
fn as_parts(m: &Value) -> Value {
    let mut m = m.clone();
    if let Some(text) = m["content"].as_str().filter(|t| !t.is_empty()) {
        m["content"] = json!([{ "type": "text", "text": text }]);
    }
    m
}

/// A message with its last block marked; `None` when nothing can carry it.
fn marked(m: &Value) -> Option<Value> {
    let mut m = as_parts(m);
    let last = m["content"].as_array_mut()?.last_mut()?.as_object_mut()?;
    if !markable(last) {
        return None;
    }
    last.insert("cache_control".into(), ephemeral());
    Some(m)
}

fn markable(block: &Map<String, Value>) -> bool {
    match block.get("type").and_then(Value::as_str) {
        Some("thinking" | "redacted_thinking") | None => false,
        Some("text") => !block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .is_empty(),
        Some(_) => true,
    }
}

// MARK: - the chat shape ⇄ the Messages shape

/// The agent's chat-completions transcript as Messages (`Claude.messages`):
/// the system message dropped (it goes in `system`), an assistant turn as
/// the blocks it came back with (`_blocks`, thinking and its signature
/// included) or else its text and tool calls as `tool_use`, tool results as
/// a user turn's `tool_result`, turns of one role merged, and a user turn
/// first.
pub fn messages_from_chat(chat: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in chat {
        match role(m) {
            "user" => {
                let content = match &m["content"] {
                    Value::String(s) => Value::String(s.clone()),
                    other => Value::Array(user_blocks(other)),
                };
                append(json!({ "role": "user", "content": content }), &mut out);
            }
            "assistant" => {
                let kept = sanitised(&m["_blocks"]);
                if !kept.is_empty() {
                    append(json!({ "role": "assistant", "content": kept }), &mut out);
                    continue;
                }
                let mut blocks = Vec::new();
                let text = joined_text(&m["content"]);
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for call in m["tool_calls"].as_array().into_iter().flatten() {
                    let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
                    let input =
                        serde_json::from_str::<Map<String, Value>>(arguments).unwrap_or_default();
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call["id"].as_str().unwrap_or(""),
                        "name": call["function"]["name"].as_str().unwrap_or(""),
                        "input": input,
                    }));
                }
                if !blocks.is_empty() {
                    append(json!({ "role": "assistant", "content": blocks }), &mut out);
                }
            }
            "tool" => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": m["tool_call_id"].as_str().unwrap_or(""),
                    "content": m["content"].as_str().unwrap_or(""),
                });
                append(json!({ "role": "user", "content": [block] }), &mut out);
            }
            _ => {}
        }
    }
    if out.first().map(role) != Some("user") {
        out.insert(0, json!({ "role": "user", "content": "(continued)" }));
    }
    out
}

/// The chat's function tools as Messages tools (`Claude.tools`).
pub fn tools_from_chat(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|t| t.get("function"))
        .map(|f| {
            let schema = match f.get("parameters") {
                Some(p @ Value::Object(_)) => p.clone(),
                _ => json!({ "type": "object", "properties": {} }),
            };
            json!({
                "name": f["name"].as_str().unwrap_or(""),
                "description": f["description"].as_str().unwrap_or(""),
                "input_schema": schema,
            })
        })
        .collect()
}

/// A Messages answer as the chat's assistant message (`Claude.chatMessage`):
/// `content`, `tool_calls`, the blocks themselves for the next request
/// (`_blocks`), why it stopped (`_stop`: `max_tokens` is a cut-off) and who
/// answered (`_model`).
pub fn chat_message(payload: &Value) -> Map<String, Value> {
    let blocks = sanitised(&payload["content"]);
    let mut calls = Vec::new();
    for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
        calls.push(json!({
            "id": b["id"],
            "type": "function",
            "function": { "name": b["name"], "arguments": b["input"].to_string() },
        }));
    }
    let mut out = Map::new();
    out.insert("role".into(), json!("assistant"));
    out.insert(
        "content".into(),
        json!(joined_text(&Value::Array(blocks.clone()))),
    );
    out.insert("_blocks".into(), Value::Array(blocks));
    if !calls.is_empty() {
        out.insert("tool_calls".into(), Value::Array(calls));
    }
    if let Some(stop) = payload["stop_reason"].as_str() {
        out.insert("_stop".into(), json!(stop));
    }
    if let Some(model) = payload["model"].as_str() {
        out.insert("_model".into(), json!(model));
    }
    out
}

/// The blocks a request may carry back, each with only its own fields
/// (`Claude.sanitised`): text, tool use, thinking with its signature,
/// redacted thinking.
pub fn sanitised(content: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for b in content.as_array().into_iter().flatten() {
        let text = |k: &str| b[k].as_str().map(str::to_string);
        let kept = match b["type"].as_str() {
            Some("text") => text("text").map(|t| json!({ "type": "text", "text": t })),
            Some("tool_use") => match (text("id"), text("name")) {
                (Some(id), Some(name)) => {
                    let input = match &b["input"] {
                        Value::Object(o) => Value::Object(o.clone()),
                        _ => json!({}),
                    };
                    Some(json!({ "type": "tool_use", "id": id, "name": name, "input": input }))
                }
                _ => None,
            },
            Some("thinking") => match (text("thinking"), text("signature")) {
                (Some(thinking), Some(signature)) => Some(
                    json!({ "type": "thinking", "thinking": thinking, "signature": signature }),
                ),
                _ => None,
            },
            Some("redacted_thinking") => {
                text("data").map(|d| json!({ "type": "redacted_thinking", "data": d }))
            }
            _ => None,
        };
        out.extend(kept);
    }
    out
}

/// The text blocks of `content` joined (a string is itself).
pub fn joined_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

/// A user turn's parts: text, and pictures as base64 image blocks (a
/// picture that isn't a data URL goes as its address).
fn user_blocks(content: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for part in content.as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("text") => {
                out.push(json!({ "type": "text", "text": part["text"].as_str().unwrap_or("") }))
            }
            Some("image_url") => {
                let Some(url) = part["image_url"]["url"].as_str() else {
                    continue;
                };
                out.push(match data_url(url) {
                    Some((media_type, data)) => json!({
                        "type": "image",
                        "source": { "type": "base64", "media_type": media_type, "data": data },
                    }),
                    None => json!({ "type": "text", "text": url }),
                });
            }
            _ => {}
        }
    }
    if out.is_empty() {
        out.push(empty_block());
    }
    out
}

/// `data:<media type>;base64,<data>`.
fn data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let (media_type, encoding) = head.split_once(';')?;
    encoding
        .eq_ignore_ascii_case("base64")
        .then_some((media_type, data))
}

fn empty_block() -> Value {
    json!({ "type": "text", "text": "(empty)" })
}

fn content_blocks(content: &Value) -> Vec<Value> {
    match content {
        Value::String(s) => vec![json!({ "type": "text", "text": s })],
        Value::Array(blocks) if !blocks.is_empty() => blocks.clone(),
        _ => vec![empty_block()],
    }
}

/// Add a turn, merged into the last one when it has the same role.
fn append(m: Value, out: &mut Vec<Value>) {
    match out.last_mut() {
        Some(last) if role(last) == role(&m) => {
            let mut blocks = content_blocks(&last["content"]);
            blocks.extend(content_blocks(&m["content"]));
            last["content"] = Value::Array(blocks);
        }
        _ => out.push(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(v: &Value) -> usize {
        v.to_string().matches("cache_control").count()
    }

    #[test]
    fn the_transcript_crosses_over() {
        let chat = vec![
            json!({"role": "system", "content": "dropped"}),
            json!({"role": "user", "content": "seed"}),
            json!({"role": "assistant", "content": "", "tool_calls": [
                {"id": "call-1", "type": "function", "function": {"name": "lookup", "arguments": "{\"q\":\"copper\"}"}},
                {"id": "call-2", "type": "function", "function": {"name": "broken", "arguments": "not json"}},
            ]}),
            json!({"role": "tool", "tool_call_id": "call-1", "content": "first"}),
            json!({"role": "tool", "tool_call_id": "call-2", "content": "second"}),
            json!({"role": "user", "content": [
                {"type": "text", "text": "Screenshot:"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,abc"}},
                {"type": "image_url", "image_url": {"url": "https://example.com/i.png"}},
            ]}),
        ];
        let m = messages_from_chat(&chat);
        assert_eq!(m.len(), 3, "{m:?}");
        assert_eq!(m[0], json!({"role": "user", "content": "seed"}));
        let uses = m[1]["content"].as_array().unwrap();
        assert_eq!(uses[0]["type"], "tool_use");
        assert_eq!(uses[0]["input"]["q"], "copper");
        assert_eq!(uses[1]["input"], json!({}), "arguments that aren't JSON");
        let user = m[2]["content"].as_array().unwrap();
        assert_eq!(user.len(), 5, "two results and three parts, merged");
        assert_eq!(user[0]["type"], "tool_result");
        assert_eq!(user[1]["tool_use_id"], "call-2");
        assert_eq!(user[3]["source"]["media_type"], "image/png");
        assert_eq!(user[3]["source"]["data"], "abc");
        assert_eq!(user[4]["text"], "https://example.com/i.png");

        let started = messages_from_chat(&[json!({"role": "assistant", "content": "hi"})]);
        assert_eq!(started[0]["content"], "(continued)");

        let tools = tools_from_chat(&[
            json!({"type": "function", "function": {"name": "lookup", "description": "Find it"}}),
        ]);
        assert_eq!(tools[0]["name"], "lookup");
        assert_eq!(tools[0]["input_schema"]["type"], "object");
    }

    #[test]
    fn answers_come_back_in_the_chat_shape() {
        let m = chat_message(&json!({
            "model": "claude-sonnet-5",
            "stop_reason": "max_tokens",
            "content": [
                {"type": "thinking", "thinking": "hm", "signature": "s"},
                {"type": "text", "text": "done"},
                {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"q": "x"}},
            ],
        }));
        assert_eq!(m["content"], "done");
        assert_eq!(m["tool_calls"][0]["id"], "toolu_1");
        assert_eq!(m["tool_calls"][0]["function"]["arguments"], "{\"q\":\"x\"}");
        assert_eq!(m["_stop"], "max_tokens");
        assert_eq!(m["_model"], "claude-sonnet-5");
        // The next request carries the thinking back, signature and all.
        let back = messages_from_chat(&[json!({"role": "user", "content": "q"}), Value::Object(m)]);
        let blocks = back[1]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[0],
            json!({"type": "thinking", "thinking": "hm", "signature": "s"})
        );
        assert_eq!(blocks[2]["input"]["q"], "x");
    }

    #[test]
    fn bodies_carry_the_identity_and_the_marks() {
        let one = body(
            "claude-haiku-4-5",
            system("Reply with JSON only."),
            vec![json!({"role": "user", "content": "hi"})],
            vec![],
            4096,
            Marks::Prefix,
        );
        assert_eq!(one["system"][0], json!({"type": "text", "text": IDENTITY}));
        assert_eq!(one["system"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(count(&one), 1, "an ask marks only its instructions");
        assert_eq!(one["messages"][0]["content"], "hi");
        assert!(one.get("tool_choice").is_none() && one.get("temperature").is_none());
        assert_eq!(
            count(&body("m", system(""), vec![], vec![], 1, Marks::Prefix)),
            0
        );

        let chat = vec![
            json!({"role": "user", "content": "q"}),
            json!({"role": "assistant", "tool_calls": [{"id": "a", "function": {"name": "x", "arguments": "{}"}}]}),
            json!({"role": "tool", "tool_call_id": "a", "content": "r"}),
        ];
        let tools = tools_from_chat(&[
            json!({"function": {"name": "a"}}),
            json!({"function": {"name": "b"}}),
        ]);
        let turn = body(
            "claude-opus-5-5",
            system("sys"),
            messages_from_chat(&chat),
            tools,
            16_000,
            Marks::Rolling,
        );
        assert_eq!(count(&turn), 4, "{turn}");
        assert!(turn["tools"][0].get("cache_control").is_none());
        assert_eq!(turn["tools"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            turn["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(
            turn["messages"][2]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(turn["tool_choice"], json!({"type": "auto"}));
    }

    #[test]
    fn failures_in_words() {
        assert!(
            failure(401, b"")
                .text()
                .starts_with("Claude sign-in expired")
        );
        assert!(failure(429, b"").text().contains("rate-limiting"));
        assert_eq!(failure(529, b"").text(), "Claude is overloaded right now");
        assert_eq!(
            failure(400, br#"{"error":{"message":"bad model"}}"#).text(),
            "Claude 400: bad model"
        );
        assert_eq!(failure(500, b"oops").text(), "Claude 500: oops");
    }
}
