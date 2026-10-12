//! JSON-RPC over the transport (`MCP.handle`, `MCP.route`'s POST branch).
//!
//! The transport parses HTTP on its own thread and sends each POST body here
//! as a [`Job`]; [`Mcp::serve`] answers them on the host's thread, many in
//! flight at once, and keeps the backstage sweep going.

use std::future::Future;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::{Either, select};
use futures_util::stream::FuturesUnordered;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::agent_tabs::opens_behind;
use crate::catalogue;
use crate::drive::{self, DriveEvent, Outcome};
use crate::host::{Content, Host};
use crate::who::{self, Said, Who};
use crate::{CallCtx, Mcp};

/// The headers a message's meaning depends on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestCtx {
    /// `Mcp-Session-Id` the client sent, or the one minted for its `initialize`.
    pub session: Option<String>,
    /// `X-Copper-Agent`.
    pub agent: Option<String>,
}

/// What came in on one POST.
#[derive(Debug)]
pub enum Body {
    One(Map<String, Value>),
    Many(Vec<Value>),
}

/// What goes back: a JSON value (an object or an array), or nothing (202).
pub type Answer = Option<Value>;

/// One POST for the host thread.
#[derive(Debug)]
pub struct Job {
    pub body: Body,
    pub ctx: RequestCtx,
    pub reply: oneshot::Sender<Answer>,
}

/// The sending side the transport holds.
pub type JobSender = mpsc::UnboundedSender<Job>;
/// The receiving side [`Mcp::serve`] drains.
pub type JobReceiver = mpsc::UnboundedReceiver<Job>;

/// A fresh channel.
pub fn channel() -> (JobSender, JobReceiver) {
    mpsc::unbounded_channel()
}

/// The loopback-only control methods (the CLI's `link`, `intelligence`,
/// `claude`, `bitwarden` verbs). Never reachable through an agent link.
pub const CONTROLS: [&str; 4] = [
    "copper/link",
    "copper/intelligence",
    "copper/claude",
    "copper/bitwarden",
];

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

type Pending<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;

impl<H: Host> Mcp<H> {
    /// Answer jobs until the channel closes. Spawn on the host's loop:
    /// `glib::MainContext::default().spawn_local(mcp.clone().serve(jobs))`.
    pub async fn serve(self: Rc<Self>, mut jobs: JobReceiver) {
        self.running.set(true);
        let mut in_flight: FuturesUnordered<Pending<'_>> = FuturesUnordered::new();
        let mut tick: Pin<Box<dyn Future<Output = ()> + '_>> =
            Box::pin(self.host.sleep(Duration::from_secs(15)));
        loop {
            let next = pin!(jobs.recv());
            let busy = async {
                if in_flight.is_empty() {
                    std::future::pending::<()>().await;
                } else {
                    in_flight.next().await;
                }
            };
            let busy = pin!(busy);
            let either = select(next, select(busy, tick.as_mut())).await;
            match either {
                Either::Left((Some(job), _)) => {
                    let this = &self;
                    in_flight.push(Box::pin(async move {
                        let answer = this.answer(job.body, &job.ctx).await;
                        let _ = job.reply.send(answer);
                    }));
                }
                Either::Left((None, _)) => break,
                Either::Right((Either::Left(_), _)) => {}
                Either::Right((Either::Right(_), _)) => {
                    self.sweep();
                    tick = Box::pin(self.host.sleep(Duration::from_secs(15)));
                }
            }
        }
        while in_flight.next().await.is_some() {}
        self.running.set(false);
    }

    /// One POST body: a message or a batch.
    pub async fn answer(&self, body: Body, ctx: &RequestCtx) -> Answer {
        match body {
            Body::One(message) => {
                let method = message.get("method").and_then(Value::as_str).unwrap_or("");
                if CONTROLS.contains(&method) {
                    let params = message
                        .get("params")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_default();
                    let result = self.host.control(method, &params).await;
                    let id = message.get("id").cloned().unwrap_or(Value::Null);
                    return Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                }
                self.handle(&message, ctx, None).await
            }
            Body::Many(messages) => {
                let mut replies = Vec::new();
                for one in messages {
                    let Value::Object(one) = one else { continue };
                    if let Some(reply) = self.handle(&one, ctx, None).await {
                        replies.push(reply);
                    }
                }
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
        }
    }

    /// One JSON-RPC message in, one reply out — or none, for a notification.
    /// `given` is the caller when it is not a loopback client (the agent
    /// pane, a linked app); such callers never see `cloud_history_delete`.
    pub async fn handle(
        &self,
        message: &Map<String, Value>,
        ctx: &RequestCtx,
        given: Option<Who>,
    ) -> Answer {
        let id = message.get("id");
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let empty = Map::new();
        let params = message
            .get("params")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let reply =
            |result: Value| id.map(|id| json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        let loopback = given.is_none();

        match method {
            "initialize" => {
                if let Some(name) = params
                    .get("clientInfo")
                    .and_then(|c| c.get("name"))
                    .and_then(Value::as_str)
                {
                    let mut state = self.state.borrow_mut();
                    state.client_name = who::pretty(name);
                    if let Some(sid) = &ctx.session {
                        let first = Said {
                            name: name.to_string(),
                            ..Said::default()
                        };
                        let said = Said::of_request(params, ctx.agent.as_deref()).over(&first);
                        state.said.insert(sid.clone(), said);
                    }
                }
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = if catalogue::PROTOCOL_VERSIONS.contains(&asked) {
                    asked
                } else {
                    "2025-06-18"
                };
                let settings = self.settings.borrow().clone();
                reply(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": {}, "prompts": {} },
                    "serverInfo": { "name": "copper", "version": settings.version },
                    "instructions": catalogue::instructions(settings.jev),
                }))
            }
            "notifications/initialized"
            | "notifications/cancelled"
            | "notifications/roots/list_changed" => None,
            "ping" => reply(json!({})),
            "tools/list" => {
                let jev = self.settings.borrow().jev;
                reply(json!({ "tools": catalogue::catalogue(jev, loopback) }))
            }
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let arguments = params
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let offered = catalogue::tool(name).is_some()
                    && !catalogue::OMITTED.contains(&name)
                    && (loopback || name != catalogue::CLOUD_HISTORY);
                if !offered {
                    return reply(result(
                        vec![Content::Text(format!("Unknown tool {name}"))],
                        true,
                        None,
                    ));
                }
                let caller = given.unwrap_or_else(|| self.caller(params, ctx));
                let (content, is_error, summary) =
                    self.tools_call(name, &arguments, caller, loopback).await;
                reply(result(content, is_error, summary))
            }
            "resources/list" => reply(json!({ "resources": [] })),
            "resources/templates/list" => reply(json!({ "resourceTemplates": [] })),
            "prompts/list" => reply(json!({ "prompts": [] })),
            "logging/setLevel" => reply(json!({})),
            _ => id.map(|id| error(id, -32601, &format!("Method not found: {method}"))),
        }
    }

    /// Which session this is, from what it says about itself (Hands.swift).
    fn caller(&self, params: &Map<String, Value>, ctx: &RequestCtx) -> Who {
        let said_now = Said::of_request(params, ctx.agent.as_deref());
        let state = self.state.borrow();
        let known = ctx
            .session
            .as_ref()
            .and_then(|s| state.said.get(s))
            .cloned()
            .unwrap_or_default();
        who::from(
            &said_now.over(&known),
            ctx.session.as_deref(),
            &state.client_name,
        )
    }

    /// A tool call with the bookkeeping around it: the status line, Stop's
    /// refusal, the driver pane's ticket. Returns content, isError, summary.
    pub async fn tools_call(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
        caller: Who,
        loopback: bool,
    ) -> (Vec<Content>, bool, Option<String>) {
        {
            let mut state = self.state.borrow_mut();
            state.calls += 1;
            state.last_tool = name.to_string();
        }
        let mut ctx = CallCtx {
            who: caller.clone(),
            loopback,
            behind: opens_behind(name, arguments),
            ..CallCtx::default()
        };
        let landing = self.peek(arguments, &ctx);
        let on_screen = landing.as_ref().is_none_or(|t| t.selected);
        if on_screen && !ctx.behind && self.settings.borrow().announces {
            self.host.announce(&format!("Agent · {name}"));
        }
        let refusal = self.state.borrow_mut().drive.refusal(&caller.key);
        if let Some(text) = refusal {
            self.host.drive(&DriveEvent::CallRefused {
                who: caller.clone(),
                tool: name.to_string(),
            });
            return (vec![Content::text(text)], true, None);
        }
        let ticket = if name == "jev_run" || name == "jev_step" {
            None
        } else {
            let ticket = self.state.borrow_mut().drive.ticket();
            let (title, detail) = drive::words(name, arguments);
            self.host.drive(&DriveEvent::CallBegan {
                ticket,
                who: caller.clone(),
                tool: name.to_string(),
                title,
                detail,
                tab: landing.as_ref().map(|t| t.id.clone()),
                behind: ctx.behind,
            });
            Some(ticket)
        };
        let outcome = self.call_tool(name, arguments, &mut ctx).await;
        let (content, error) = match outcome {
            Ok(content) => (content, None),
            Err(f) => (vec![Content::Text(f.0.clone())], Some(f.0)),
        };
        if let Some(ticket) = ticket {
            let tab = self.peek(arguments, &ctx).map(|t| t.id);
            let summary = ctx.summary.clone().filter(|s| !s.trim().is_empty());
            let mut o = Outcome {
                operation: drive::operation(name, arguments),
                label: drive::target(name, arguments),
                text: drive::typed(name, arguments),
                error: error.clone(),
                ..Outcome::default()
            };
            if let Some(summary) = summary {
                o.result = Some(summary);
            }
            self.host.drive(&DriveEvent::CallEnded {
                ticket,
                who: caller,
                tool: name.to_string(),
                outcome: o,
                tab,
            });
        }
        (content, error.is_some(), ctx.summary)
    }
}

/// `{content, isError, _meta?}`.
fn result(content: Vec<Content>, is_error: bool, summary: Option<String>) -> Value {
    let mut out = json!({
        "content": content.iter().map(Content::json).collect::<Vec<_>>(),
        "isError": is_error,
    });
    if let Some(line) = summary {
        out["_meta"] = json!({ "summary": line });
    }
    out
}
