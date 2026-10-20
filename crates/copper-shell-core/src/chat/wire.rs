//! One turn on the router (`Agent.complete`, `Router.body`,
//! `PromptCache.chat`): an OpenAI-compatible `POST {router}/v1/chat/completions`
//! with the whole conversation, Copper's tools and prompt-cache marks.
//!
//! The router's quirks, handled as the Mac does: no `temperature` and no
//! forced tool (`tool_choice: "auto"` is the one every model behind a gateway
//! takes); a model that refuses the cache marks is asked again without them;
//! a model with a smaller output ceiling (a 400 naming `max_tokens`) is asked
//! again under the old ceiling; any other non-2xx answer is an error that
//! carries the start of its body.
//!
//! The request runs on a tokio runtime (the host's), so the UI thread that
//! awaits it never has to be one. Bodies are written with sorted keys
//! (serde_json's map), so the same history is the same bytes every turn —
//! what the cache marks need.

use std::time::Duration;

use copper_mcp::intelligence::{Keys, family};
use serde_json::{Map, Value, json};

use super::history::{Message, message};
use super::words;

/// Output tokens a reply may take: a big `canvas_apply` is some 10k tokens
/// of arguments.
pub const OUTPUT_LIMIT: u32 = 16_000;
/// The old ceiling, for a router model that refuses the new one.
pub const FALLBACK_LIMIT: u32 = 4_096;
/// Long enough for a 16k-token reply from a slow model.
pub const TIMEOUT: Duration = Duration::from_secs(300);
/// Anthropic's ceiling on cache marks in one request.
const MARK_LIMIT: usize = 4;

/// Why a turn failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// No answer within the timeout.
    TimedOut(Duration),
    /// Anything else, as the transcript says it.
    Text(String),
}

impl Failure {
    /// The failure as a sentence for the transcript (`Agent.words(for:)`).
    pub fn words(&self) -> String {
        match self {
            Failure::TimedOut(t) => words::timed_out(t.as_secs() / 60),
            Failure::Text(s) => s.clone(),
        }
    }
}

/// What one turn came back with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    /// The words, joined when the gateway sent parts.
    pub content: String,
    /// `tool_calls`, as sent.
    pub tool_calls: Vec<Value>,
    /// `finish_reason` (`length`: cut off at the output limit).
    pub finish: Option<String>,
    /// The model that answered, when the gateway says.
    pub model: Option<String>,
}

/// Where requests go: an HTTP client on a tokio runtime.
#[derive(Clone)]
pub struct Net {
    runtime: tokio::runtime::Handle,
    client: reqwest::Client,
    user_agent: String,
}

impl Net {
    pub fn new(runtime: tokio::runtime::Handle) -> Net {
        Net {
            runtime,
            client: reqwest::Client::new(),
            user_agent: format!("copper/{}", copper_core::version::app()),
        }
    }

    /// POST a JSON body and read the whole answer, on the runtime.
    pub async fn post(
        &self,
        url: String,
        bearer: String,
        body: Vec<u8>,
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>), Failure> {
        let request = self
            .client
            .post(url)
            .timeout(timeout)
            .header("Authorization", format!("Bearer {bearer}"))
            .header("Content-Type", "application/json")
            .header("User-Agent", self.user_agent.clone())
            .body(body);
        let task = self.runtime.spawn(async move {
            let failed = |e: reqwest::Error| {
                if e.is_timeout() {
                    Failure::TimedOut(timeout)
                } else {
                    Failure::Text(without_url(&e))
                }
            };
            let response = request.send().await.map_err(failed)?;
            let status = response.status().as_u16();
            let bytes = response.bytes().await.map_err(failed)?;
            Ok::<_, Failure>((status, bytes.to_vec()))
        });
        task.await
            .map_err(|e| Failure::Text(format!("The request did not finish: {e}")))?
    }
}

/// A transport error in words, without the address (reqwest's own text
/// names the URL, which a gateway may carry a key in), then its causes.
fn without_url(e: &reqwest::Error) -> String {
    let mut text = if e.is_connect() {
        "Couldn't reach the router"
    } else if e.is_body() || e.is_decode() {
        "The router's answer could not be read"
    } else {
        "The request to the router failed"
    }
    .to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        text = format!("{text}: {s}");
        source = s.source();
    }
    text
}

/// The chat-completions endpoint for a router address.
pub fn endpoint(router_url: &str) -> Option<String> {
    let url = url::Url::parse(router_url.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some(format!(
        "{}/v1/chat/completions",
        url.as_str().trim_end_matches('/')
    ))
}

/// One chat-completions body (`Router.body` with `.rolling` marks for a
/// Claude model): the system prompt first, then the conversation.
pub fn body(
    model: &str,
    max_tokens: u32,
    system: &str,
    history: &[Message],
    tools: &[Value],
    cached: bool,
) -> Value {
    let mut messages: Vec<Message> = Vec::with_capacity(history.len() + 1);
    messages.push(message(json!({ "role": "system", "content": system })));
    messages.extend(history.iter().cloned());
    let mut tools = tools.to_vec();
    if cached && family(model).is_some() {
        mark(&mut messages, &mut tools);
    }
    let mut out = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    if !tools.is_empty() {
        out["tools"] = Value::Array(tools);
        out["tool_choice"] = Value::String("auto".into());
    }
    out
}

/// The prompt-cache marks on the gateway's chat shape (`PromptCache.chat`,
/// `.rolling`): the system prompt, the last tool, the message just before
/// the last assistant reply and the last message.
fn mark(messages: &mut [Message], tools: &mut [Value]) {
    let mut used = 0;
    if let Some(i) = messages.iter().position(|m| role(m) == "system")
        && let Some(marked) = marked(&messages[i])
    {
        messages[i] = marked;
        used += 1;
    }
    if let Some(last) = tools.last_mut().and_then(Value::as_object_mut) {
        last.insert("cache_control".into(), json!({ "type": "ephemeral" }));
        used += 1;
    }
    for m in messages.iter_mut() {
        if matches!(role(m), "user" | "tool") {
            *m = as_parts(m);
        }
    }
    let roles: Vec<String> = messages.iter().map(|m| role(m).to_string()).collect();
    for i in rolling_indices(&roles) {
        if used >= MARK_LIMIT {
            break;
        }
        if let Some(marked) = marked(&messages[i]) {
            messages[i] = marked;
            used += 1;
        }
    }
}

fn role(m: &Message) -> &str {
    m.get("role").and_then(Value::as_str).unwrap_or("")
}

/// The previous request's last message and this one's, by role.
fn rolling_indices(roles: &[String]) -> Vec<usize> {
    let takes = |r: &str| matches!(r, "user" | "tool");
    let Some(last) = roles.len().checked_sub(1) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(reply) = roles.iter().rposition(|r| r == "assistant")
        && reply > 0
        && reply - 1 != last
        && takes(&roles[reply - 1])
    {
        out.push(reply - 1);
    }
    if takes(&roles[last]) {
        out.push(last);
    }
    out
}

/// A message whose content is a non-empty string, as one text part.
fn as_parts(m: &Message) -> Message {
    let mut m = m.clone();
    if let Some(Value::String(text)) = m.get("content")
        && !text.is_empty()
    {
        let parts = json!([{ "type": "text", "text": text }]);
        m.insert("content".into(), parts);
    }
    m
}

/// A message with its last content part marked; `None` when nothing can
/// carry a mark.
fn marked(m: &Message) -> Option<Message> {
    let mut m = as_parts(m);
    let parts = m.get_mut("content")?.as_array_mut()?;
    let last = parts.last_mut()?.as_object_mut()?;
    if !markable(last) {
        return None;
    }
    last.insert("cache_control".into(), json!({ "type": "ephemeral" }));
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

/// Whether an error answer says the marks were the problem.
fn cache_refused(data: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&data[..data.len().min(4000)]).to_lowercase();
    text.contains("prompt caching") || text.contains("cache_control") || text.contains("cachepoint")
}

/// One turn: the conversation, the tools, the router's answer.
pub async fn complete(
    net: &Net,
    keys: &Keys,
    model: &str,
    history: &[Message],
    tools: &[Value],
) -> Result<Reply, Failure> {
    let url =
        endpoint(&keys.router_url).ok_or_else(|| Failure::Text("Bad router address".into()))?;
    let mut limit = OUTPUT_LIMIT;
    let mut cached = true;
    loop {
        let sent = body(model, limit, words::SYSTEM, history, tools, cached);
        let bytes = serde_json::to_vec(&sent).unwrap_or_default();
        let (status, data) = net
            .post(url.clone(), keys.router_key.clone(), bytes, TIMEOUT)
            .await?;
        if status != 200 && cached && cache_refused(&data) {
            cached = false;
            continue;
        }
        if status == 400
            && limit > FALLBACK_LIMIT
            && String::from_utf8_lossy(&data[..data.len().min(2000)]).contains("max_tokens")
        {
            limit = FALLBACK_LIMIT;
            continue;
        }
        if status != 200 {
            return Err(Failure::Text(format!(
                "router {status}: {}",
                String::from_utf8_lossy(&data[..data.len().min(300)])
            )));
        }
        return parse(&data);
    }
}

/// The first choice of a chat-completions answer.
pub fn parse(data: &[u8]) -> Result<Reply, Failure> {
    let none = || Failure::Text("router answered with no choices".into());
    let payload: Value = serde_json::from_slice(data).map_err(|_| none())?;
    let choice = payload["choices"].get(0).ok_or_else(none)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(none)?;
    Ok(Reply {
        content: super::history::text_of(message.get("content")),
        tool_calls: message
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        finish: choice["finish_reason"].as_str().map(str::to_string),
        model: payload["model"].as_str().map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> Vec<Message> {
        vec![
            message(json!({"role": "user", "content": "q"})),
            message(
                json!({"role": "assistant", "tool_calls": [{"id": "a", "type": "function", "function": {"name": "x", "arguments": "{}"}}]}),
            ),
            message(json!({"role": "tool", "tool_call_id": "a", "content": "r"})),
        ]
    }

    #[test]
    fn claude_models_get_rolling_marks() {
        let tools = vec![
            json!({"type": "function", "function": {"name": "a"}}),
            json!({"type": "function", "function": {"name": "b"}}),
        ];
        let b = body("sonnet", 16_000, "sys", &history(), &tools, true);
        let text = serde_json::to_string(&b).unwrap();
        assert_eq!(text.matches("cache_control").count(), 4, "{text}");
        assert_eq!(
            b["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(b["tools"][0].get("cache_control").is_none());
        assert_eq!(b["tools"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            b["messages"][1]["content"][0]["cache_control"]["type"], "ephemeral",
            "the message before the last reply"
        );
        assert_eq!(
            b["messages"][3]["content"][0]["cache_control"]["type"], "ephemeral",
            "the last message"
        );
        assert_eq!(b["tool_choice"], "auto");
        assert!(b.get("temperature").is_none());
        assert_eq!(b["max_tokens"], 16_000);
    }

    #[test]
    fn other_models_and_retries_get_none() {
        for (model, cached) in [("gpt-4o-mini", true), ("sonnet", false)] {
            let b = body(model, 4096, "sys", &history(), &[], cached);
            let text = serde_json::to_string(&b).unwrap();
            assert!(!text.contains("cache_control"), "{model}");
            assert_eq!(
                b["messages"][1]["content"], "q",
                "plain strings stay strings"
            );
            assert!(b.get("tools").is_none() && b.get("tool_choice").is_none());
        }
    }

    #[test]
    fn endpoints_and_answers() {
        assert_eq!(
            endpoint("https://llm.example.com/").as_deref(),
            Some("https://llm.example.com/v1/chat/completions")
        );
        assert_eq!(endpoint("not a url"), None);
        let r = parse(br#"{"model":"m","choices":[{"finish_reason":"length","message":{"content":[{"type":"text","text":"a"},{"text":"b"}],"tool_calls":[{"id":"1"}]}}]}"#).unwrap();
        assert_eq!(r.content, "ab");
        assert_eq!(r.finish.as_deref(), Some("length"));
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.model.as_deref(), Some("m"));
        assert_eq!(
            parse(b"{}"),
            Err(Failure::Text("router answered with no choices".into()))
        );
        assert_eq!(
            Failure::TimedOut(TIMEOUT).words(),
            "The model took more than 5 minutes to answer, so the request was given up. Try a smaller piece of the job."
        );
    }
}
