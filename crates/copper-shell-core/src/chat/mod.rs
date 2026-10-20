//! Copper's agent in the window (Mac: `Fork/Agent/Agent.swift`): a chat
//! with a model that has Copper's own tools bound in-process — the same
//! `copper_mcp` tools the MCP server offers, run without any HTTP round trip
//! and whether or not the loopback server is listening.
//!
//! The model is whatever the router serves (Settings › Intelligence, or
//! Copper Cloud's keys): an OpenAI-compatible chat completion with tool
//! calling, one turn per request ([`wire`]), the tools run here between
//! turns, until the model answers in words or the round budget is spent.
//! Nothing is configured out of the box: no router key, no agent.
//!
//! The toolkit-free part lives here so every platform shares it; the app
//! brings a [`ChatHost`] (the browser's is [`ShellChat`]) and shows the
//! conversation however it likes — on Windows and Linux, through the addon
//! bridge's `copper.agent.*` ([`Chat::state`], [`Chat::send`], [`Chat::stop`],
//! [`Chat::clear`], [`Chat::set_model`], and [`Chat::on_change`] for the
//! `agent` event).
//!
//! | Module | What | Mac |
//! |---|---|---|
//! | [`host`] | what the app supplies | — |
//! | [`shell`] | the browser's host: keys, `chat.json`, the page, `copper_mcp` | `Agent.execute`, `Tools.call` |
//! | [`wire`] | one turn on the router, prompt-cache marks, its quirks | `Agent.complete`, `Router.body`, `PromptCache` |
//! | [`history`] | the model's side of the conversation | `Agent.sealed`, `Agent.arguments` |
//! | [`transcript`] | the person's side, and `state().items` | `Agent.Item`, `AgentPane.blocks`, `AddonBridge.item` |
//! | [`words`] | every sentence it says | `Agent.system`, notes, `Agent.doing` |

pub mod history;
pub mod host;
pub mod shell;
pub mod transcript;
pub mod wire;
pub mod words;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::SystemTime;

use copper_mcp::drive::DriveEvent;
use copper_mcp::intelligence::{self, Effective, Tier};
use serde_json::{Map, Value, json};

use history::{Broken, Message, message};
use transcript::{Cycle, Item, Kind, Run};

pub use host::{ChatHost, PageInFront, ToolOutput};
pub use shell::ShellChat;

/// The `Who::key` of a Jev run this agent starts (`Agent.jevKey`): Jev on
/// behalf of the pane, whose own key is `pane`.
pub const JEV_KEY: &str = "jev:pane";
/// The pane's own `Who::key`.
pub const PANE_KEY: &str = "pane";
/// A tool result is cut to this many characters for the model.
const RESULT_LIMIT: usize = 60_000;
/// At most this many pictures go to the model after one turn's calls.
const PICTURE_LIMIT: usize = 3;
/// How much of the page's text goes in front of a question.
const PAGE_TEXT_LIMIT: usize = 3000;

/// Why [`Chat::send`] turned a question away (the Mac bridge's words).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    /// Only whitespace.
    Empty,
    /// A question is still running.
    Busy,
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SendError::Empty => "agent.send needs some text",
            SendError::Busy => "The agent is still answering; stop it first",
        })
    }
}

/// The agent in one window. A cheap handle: clones share the conversation.
pub struct Chat<H: ChatHost> {
    inner: Rc<Inner<H>>,
}

impl<H: ChatHost> Clone for Chat<H> {
    fn clone(&self) -> Self {
        Chat {
            inner: self.inner.clone(),
        }
    }
}

struct Inner<H> {
    host: H,
    net: RefCell<Option<wire::Net>>,
    state: RefCell<State>,
    listeners: RefCell<Vec<Rc<dyn Fn()>>>,
}

#[derive(Default)]
struct State {
    items: Vec<Item>,
    busy: bool,
    /// "Thinking…", "Reading the page…", "Stopped".
    status: String,
    /// The model's side, in the wire shape.
    messages: Vec<Message>,
    /// Which question owns the transcript. Stop and Clear move it on, so a
    /// question still unwinding (a request in flight, a tool finishing) can
    /// never write into the next one.
    generation: u64,
    /// Jev runs and outside drivers' stretches of work, by id.
    runs: HashMap<u64, Run>,
    next_run: u64,
    /// The Jev run the drive events are about now.
    live_run: Option<u64>,
    /// This agent's own Jev run, while it runs.
    own_jev: Option<u64>,
}

impl State {
    fn new_run(&mut self, run: Run) -> u64 {
        self.next_run += 1;
        self.runs.insert(self.next_run, run);
        self.next_run
    }
}

impl<H: ChatHost> Chat<H> {
    pub fn new(host: H) -> Chat<H> {
        Chat {
            inner: Rc::new(Inner {
                host,
                net: RefCell::default(),
                state: RefCell::default(),
                listeners: RefCell::default(),
            }),
        }
    }

    /// What the app supplied.
    pub fn host(&self) -> &H {
        &self.inner.host
    }

    /// Call `f` after every change to the conversation or its state (the
    /// bridge's `agent` event; throttling is the caller's).
    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.inner.listeners.borrow_mut().push(Rc::new(f));
    }

    /// A question is running.
    pub fn busy(&self) -> bool {
        self.inner.state.borrow().busy
    }

    /// What the question is doing now ("Thinking…"), or how it ended.
    pub fn status(&self) -> String {
        self.inner.state.borrow().status.clone()
    }

    /// The transcript, flat, as the Mac keeps it.
    pub fn items(&self) -> Vec<Item> {
        self.inner.state.borrow().items.clone()
    }

    /// The model keys in force (this computer's, Copper Cloud's, the
    /// environment's).
    pub fn effective(&self) -> Effective {
        Effective::new(&self.inner.host.intelligence(), false)
    }

    /// A model can be asked: a router key and address exist.
    pub fn available(&self) -> bool {
        self.effective().model_ready()
    }

    /// `copper.agent.state()`: `{available, busy, models, model, items,
    /// pageContext, status}`.
    pub fn state(&self) -> Value {
        let eff = self.effective();
        let ready = eff.model_ready();
        let models: Vec<Value> = [Tier::Haiku, Tier::Sonnet, Tier::Opus]
            .into_iter()
            .map(|t| {
                json!({
                    "id": t.name(),
                    "name": tier_title(t),
                    "ready": ready,
                    "detail": tier_blurb(t),
                })
            })
            .collect();
        let page_context = self.inner.host.config().page_context();
        let s = self.inner.state.borrow();
        json!({
            "available": ready,
            "busy": s.busy,
            "models": models,
            "model": eff.keys.tier.name(),
            "items": transcript::items_json(&s.items, &s.runs, s.busy, &s.status),
            "pageContext": page_context,
            "status": s.status,
        })
    }

    /// `copper.agent.setModel(id)`: a tier, kept where the Mac keeps it.
    pub fn set_model(&self, id: &str) -> Result<(), String> {
        let tier = Tier::parse(id.trim())
            .ok_or_else(|| format!("No model \"{id}\"; agent.state() lists them"))?;
        self.inner.host.set_tier(tier);
        self.inner.changed();
        Ok(())
    }

    /// Ask a question (`Agent.ask`). Without a model the question stays in
    /// the transcript with the Mac's "Not set up yet…" under it.
    pub fn send(&self, text: &str) -> Result<(), SendError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(SendError::Empty);
        }
        if self.busy() {
            return Err(SendError::Busy);
        }
        if !self.available() {
            self.inner.edit(|s| {
                s.items.push(Item::new(Kind::User, text));
                s.items.push(Item::failure(words::NOT_SET_UP));
            });
            return Ok(());
        }
        let ticket = self.inner.edit(|s| {
            s.items.push(Item::new(Kind::User, text));
            s.busy = true;
            s.status = "Thinking…".into();
            s.generation += 1;
            s.generation
        });
        let inner = self.inner.clone();
        let text = text.to_string();
        self.inner.host.spawn(Box::pin(async move {
            inner.run(text, ticket).await;
            inner.edit(|s| {
                if s.generation == ticket {
                    s.messages = history::sealed(&s.messages);
                    s.busy = false;
                }
            });
        }));
        Ok(())
    }

    /// Stop the question in flight (`Agent.stop`): its unanswered calls are
    /// sealed, its running steps say "Stopped", a Jev run it started stops
    /// before its next move. Nothing happens while idle (the Mac bridge's
    /// rule: Stop on an idle agent would only leave a "Stopped" behind).
    pub fn stop(&self) {
        if !self.busy() {
            return;
        }
        let jev = self.inner.edit(|s| {
            s.generation += 1;
            s.messages = history::sealed(&s.messages);
            let now = SystemTime::now();
            for item in s
                .items
                .iter_mut()
                .filter(|i| i.running && i.kind == Kind::Tool)
            {
                item.running = false;
                item.ok = false;
                item.ms = millis_since(item.at, now);
                item.text = words::STEP_STOPPED.into();
            }
            s.busy = false;
            s.status = words::STOPPED.into();
            s.items.push(Item::new(Kind::Note, words::STOPPED));
            s.own_jev.is_some()
        });
        if jev {
            self.inner.host.stop_jev();
        }
    }

    /// A new chat (`Agent.clear`): the transcript and the model's history
    /// go; a question in flight is let go, and a Jev run it started stops.
    pub fn clear(&self) {
        let jev = self.inner.edit(|s| {
            s.generation += 1;
            s.busy = false;
            s.status.clear();
            s.items.clear();
            s.messages.clear();
            s.runs.clear();
            s.own_jev.take().is_some()
        });
        if jev {
            self.inner.host.stop_jev();
        }
    }

    /// What the tools' driver model says happened (every [`DriveEvent`]):
    /// a Jev run this agent started nests under the step that asked for it;
    /// anyone else's hands become a `drive` item.
    pub fn drive(&self, event: &DriveEvent) {
        let changed = {
            let mut s = self.inner.state.borrow_mut();
            drive_event(&mut s, event)
        };
        if changed {
            self.inner.changed();
        }
    }
}

/// `Haiku`.
pub fn tier_title(t: Tier) -> &'static str {
    match t {
        Tier::Haiku => "Haiku",
        Tier::Sonnet => "Sonnet",
        Tier::Opus => "Opus",
    }
}

/// `Quick` (`Intelligence.Tier.blurb`).
pub fn tier_blurb(t: Tier) -> &'static str {
    match t {
        Tier::Haiku => "Quick",
        Tier::Sonnet => "The balance",
        Tier::Opus => "Thinks hardest",
    }
}

fn millis_since(at: SystemTime, now: SystemTime) -> f64 {
    now.duration_since(at)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// One driver event into the state; whether anything changed.
fn drive_event(s: &mut State, event: &DriveEvent) -> bool {
    let now = SystemTime::now();
    match event {
        DriveEvent::RunBegan { who, goal, .. } => {
            let id = s.new_run(Run::new(&who.key, true, goal));
            s.live_run = Some(id);
            if who.key == JEV_KEY {
                s.own_jev = Some(id);
                if let Some(step) = s
                    .items
                    .iter_mut()
                    .rev()
                    .find(|i| i.kind == Kind::Tool && i.running && i.run.is_none())
                {
                    step.run = Some(id);
                    return true;
                }
            }
            let mut card = Item::new(Kind::Drive, goal.clone());
            card.title = if who.agent.is_empty() {
                "Jev".into()
            } else {
                who.agent.clone()
            };
            card.who = who.key.clone();
            card.run = Some(id);
            s.items.push(card);
            true
        }
        DriveEvent::Cycle => live_run(s).is_some_and(|run| {
            // A new move ends the last one (`Drive.cycle`), outcome or not.
            if let Some(last) = run.cycles.last_mut()
                && last.ended.is_none()
            {
                last.ended = Some(now);
            }
            run.cycles.push(Cycle {
                started: Some(now),
                ..Cycle::default()
            });
            true
        }),
        DriveEvent::Phase { title, .. } => live_run(s).is_some_and(|run| {
            if run.cycles.last().is_none_or(|c| c.ended.is_some()) {
                run.cycles.push(Cycle {
                    started: Some(now),
                    ..Cycle::default()
                });
            }
            if let Some(c) = run.cycles.last_mut() {
                c.phases.push(title.clone());
            }
            true
        }),
        DriveEvent::Outcome(outcome) => live_run(s).is_some_and(|run| {
            if run.cycles.last().is_none_or(|c| c.ended.is_some()) {
                run.cycles.push(Cycle {
                    started: Some(now),
                    ..Cycle::default()
                });
            }
            if let Some(c) = run.cycles.last_mut() {
                c.outcome = Some(outcome.clone());
                c.ended = Some(now);
            }
            true
        }),
        DriveEvent::RunFinished { status, note } => {
            let Some(id) = s.live_run.take() else {
                return false;
            };
            if s.own_jev == Some(id) {
                s.own_jev = None;
            }
            let Some(run) = s.runs.get_mut(&id) else {
                return false;
            };
            run.status = *status;
            run.note = note.clone();
            for c in run.cycles.iter_mut().filter(|c| c.ended.is_none()) {
                c.ended = Some(now);
            }
            true
        }
        DriveEvent::CallBegan { who, title, .. } if who.key != PANE_KEY && who.key != JEV_KEY => {
            let id = outside_card(s, who);
            if let Some(run) = s.runs.get_mut(&id) {
                run.cycles.push(Cycle {
                    phases: vec![title.clone()],
                    started: Some(now),
                    ..Cycle::default()
                });
            }
            true
        }
        DriveEvent::CallEnded { who, outcome, .. } if who.key != PANE_KEY && who.key != JEV_KEY => {
            let Some(card) = s
                .items
                .iter_mut()
                .rev()
                .find(|i| i.kind == Kind::Drive && i.who == who.key)
            else {
                return false;
            };
            card.running = false;
            if let Some(c) = card
                .run
                .and_then(|r| s.runs.get_mut(&r))
                .and_then(|run| run.cycles.iter_mut().rev().find(|c| c.ended.is_none()))
            {
                c.outcome = Some(outcome.clone());
                c.ended = Some(now);
            }
            true
        }
        _ => false,
    }
}

fn live_run(s: &mut State) -> Option<&mut Run> {
    let id = s.live_run?;
    s.runs.get_mut(&id)
}

/// The card an outside driver's call lands on: its latest since the last
/// question, or a new one. Returns the card's run.
fn outside_card(s: &mut State, who: &copper_mcp::Who) -> u64 {
    let last_user = s.items.iter().rposition(|i| i.kind == Kind::User);
    let found = s
        .items
        .iter()
        .enumerate()
        .rev()
        .take_while(|(i, _)| last_user.is_none_or(|u| *i > u))
        .find(|(_, it)| it.kind == Kind::Drive && it.who == who.key && it.run.is_some())
        .map(|(i, _)| i);
    if let Some(i) = found
        && let Some(run) = s.items[i].run
        && s.runs.get(&run).is_some_and(|r| !r.jev)
    {
        s.items[i].running = true;
        return run;
    }
    let id = s.new_run(Run::new(&who.key, false, &who.thread));
    let mut card = Item::new(Kind::Drive, who.thread.clone());
    card.title = if who.agent.is_empty() {
        "An agent".into()
    } else {
        who.agent.clone()
    };
    card.who = who.key.clone();
    card.run = Some(id);
    card.running = true;
    s.items.push(card);
    id
}

impl<H: ChatHost> Inner<H> {
    /// Change the state, then tell the listeners (outside the borrow: they
    /// read the state).
    fn edit<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let out = f(&mut self.state.borrow_mut());
        self.changed();
        out
    }

    fn changed(&self) {
        let listeners: Vec<Rc<dyn Fn()>> = self.listeners.borrow().clone();
        for f in listeners {
            f();
        }
    }

    fn live(&self, ticket: u64) -> bool {
        self.state.borrow().generation == ticket
    }

    fn net(&self) -> wire::Net {
        self.net
            .borrow_mut()
            .get_or_insert_with(|| wire::Net::new(self.host.runtime()))
            .clone()
    }

    /// The question as it goes to the model: the page in front of it when
    /// Settings › Agents says so.
    async fn question(&self, text: &str) -> String {
        if !self.host.config().page_context() {
            return text.to_string();
        }
        let Some(page) = self.host.page().await else {
            return text.to_string();
        };
        let mut context = format!("Current page: {}\nTitle: {}", page.url, page.title);
        if let Some((id, name)) = &page.canvas {
            context.push_str(&format!(
                "\nThis tab is the canvas “{name}” (id {id}): use canvas_read and canvas_apply on it."
            ));
        } else if let Some(slice) = page.text.as_deref().filter(|t| !t.is_empty()) {
            context.push_str("\nVisible text (first 3000 chars):\n");
            context.extend(slice.chars().take(PAGE_TEXT_LIMIT));
        }
        format!("<page>\n{context}\n</page>\n\n{text}")
    }

    /// One question, to its answer, a failure, Stop or the round budget
    /// (`Agent.run`).
    async fn run(&self, text: String, ticket: u64) {
        {
            let mut s = self.state.borrow_mut();
            s.messages = history::sealed(&s.messages);
        }
        let source = self.host.intelligence();
        let eff = Effective::new(&source, false);
        let content = self.question(&text).await;
        if !self.live(ticket) {
            return;
        }
        let asked = self.edit(|s| {
            s.messages
                .push(message(json!({ "role": "user", "content": content })));
            s.messages.len() - 1
        });
        let jev = self.host.jev_mode() && eff.jev_ready();
        // Read once per question, so every round sends the same tool bytes.
        let tools = model_tools(&self.host.catalogue(jev));
        // Read once: a cloud refresh mid-run does not move the goalposts.
        let (turns, cloud) = budget(
            self.host.config().effective_max_turns(),
            source.cloud.as_ref().and_then(|c| c.agent_max_turns()),
        );
        let model = intelligence::model(&eff.keys, None);
        let net = self.net();
        let mut cutoffs = 0;
        let mut nudged = false;
        for turn in 0..turns {
            if !self.live(ticket) {
                return;
            }
            self.edit(|s| {
                s.status = if turn == 0 {
                    "Thinking…".into()
                } else {
                    format!("Thinking… ({})", turn + 1)
                }
            });
            let history = history::sealed(&self.state.borrow().messages);
            let reply = wire::complete(&net, &eff.keys, &model, &history, &tools).await;
            if !self.live(ticket) {
                return;
            }
            let reply = match reply {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(target: "copper::agent", "the model's turn failed: {}", e.words());
                    self.edit(|s| {
                        s.items.push(Item::failure(e.words()));
                        // On the first turn the model never saw the question,
                        // so it goes; later, the tool results stay.
                        if turn == 0 && s.messages.len() > asked {
                            s.messages.truncate(asked);
                        }
                        s.messages = history::sealed(&s.messages);
                        s.status.clear();
                    });
                    return;
                }
            };
            match self.turn(reply, &mut cutoffs, &mut nudged, ticket).await {
                Turn::Again => {}
                Turn::Done => return,
            }
        }
        self.edit(|s| {
            s.items.push(Item::failure(words::stop_note(turns, cloud)));
            s.status.clear();
        });
    }

    /// One reply: the answer, a nudge, or the tool calls it asked for.
    async fn turn(
        &self,
        reply: wire::Reply,
        cutoffs: &mut u32,
        nudged: &mut bool,
        ticket: u64,
    ) -> Turn {
        let cut = history::cut_off(reply.finish.as_deref());
        let content = reply.content;
        let said = content.trim().to_string();
        let mut calls = reply.tool_calls;
        // A call with no id gets one here, where the history can keep it.
        for call in calls.iter_mut() {
            if call["id"].as_str().unwrap_or("").is_empty()
                && let Some(c) = call.as_object_mut()
            {
                c.insert("id".into(), json!(format!("call_{}", uuid::Uuid::new_v4())));
            }
        }
        // Every call's arguments are read before any runs; one that is not
        // a JSON object is answered as broken, never run with `{}`.
        let mut arguments: Vec<Result<Map<String, Value>, Broken>> =
            calls.iter().map(history::arguments).collect();
        if cut && let Some(last) = arguments.last_mut() {
            *last = Err(Broken::CutOff);
        }
        for (call, args) in calls.iter_mut().zip(&arguments) {
            if args.is_err() {
                *call = history::emptied(call);
            }
        }
        self.edit(|s| {
            let mut assistant = json!({ "role": "assistant" });
            if !content.is_empty() {
                assistant["content"] = json!(content);
            }
            if !calls.is_empty() {
                assistant["tool_calls"] = Value::Array(calls.clone());
            }
            // An empty assistant turn is no history worth keeping.
            if !said.is_empty() || !calls.is_empty() {
                s.messages.push(message(assistant));
            }
            if !said.is_empty() {
                let mut item = Item::new(Kind::Assistant, content.clone());
                item.aside = !calls.is_empty();
                s.items.push(item);
            }
        });

        if calls.is_empty() {
            if cut {
                *cutoffs += 1;
                if *cutoffs >= history::CUT_LIMIT {
                    self.finish(Item::failure(words::CUT_GIVE_UP));
                    return Turn::Done;
                }
                self.edit(|s| {
                    // Half an answer, with more to come: narration.
                    if !said.is_empty()
                        && let Some(last) = s.items.last_mut()
                    {
                        last.aside = true;
                    }
                    s.items.push(Item::failure(words::CUT_NOTE));
                    s.messages.push(message(
                        json!({ "role": "user", "content": words::CUT_NUDGE }),
                    ));
                });
                return Turn::Again;
            }
            if said.is_empty() {
                self.finish(Item::failure(words::EMPTY_REPLY));
                return Turn::Done;
            }
            // "Let me do that now:" and then nothing: told once.
            if !*nudged && history::announces(&said) {
                *nudged = true;
                self.edit(|s| {
                    if let Some(last) = s.items.last_mut() {
                        last.aside = true;
                    }
                    s.messages.push(message(
                        json!({ "role": "user", "content": words::ACT_NUDGE }),
                    ));
                });
                return Turn::Again;
            }
            self.edit(|s| s.status.clear());
            return Turn::Done;
        }
        if cut {
            *cutoffs += 1;
            let name = calls
                .last()
                .and_then(|c| c["function"]["name"].as_str())
                .unwrap_or("tool")
                .to_string();
            self.edit(|s| {
                s.items.push(Item::failure(words::cut_call_note(
                    wire::OUTPUT_LIMIT,
                    &name,
                )))
            });
        } else {
            *cutoffs = 0;
        }

        let mut pictures: Vec<(Vec<u8>, String)> = Vec::new();
        for (call, args) in calls.iter().zip(arguments) {
            if !self.live(ticket) {
                return Turn::Done;
            }
            let id = call["id"].as_str().unwrap_or("").to_string();
            let name = call["function"]["name"].as_str().unwrap_or("").to_string();
            let args = match args {
                Ok(args) => args,
                Err(broken) => {
                    self.edit(|s| {
                        let text = if broken == Broken::CutOff {
                            words::STEP_CUT
                        } else {
                            words::STEP_INVALID
                        };
                        let mut step = Item::new(Kind::Tool, text);
                        step.tool = name.clone();
                        step.ok = false;
                        step.title = words::doing(&name, &Map::new());
                        s.items.push(step);
                        s.messages.push(tool_result(&id, broken.for_model()));
                    });
                    continue;
                }
            };
            let title = words::doing(&name, &args);
            let step_id = self.edit(|s| {
                s.status = format!("{title}…");
                let mut step = Item::new(Kind::Tool, "");
                step.tool = name.clone();
                step.running = true;
                step.title = title.clone();
                let id = step.id.clone();
                s.items.push(step);
                id
            });
            let out = self.host.call(&name, &args).await;
            if !self.live(ticket) {
                return Turn::Done;
            }
            // A canvas_apply that returns has still not necessarily done
            // what was asked: every op turned away is an error the user sees.
            let mut warning = None;
            let mut failed = out.is_error;
            if name == "canvas_apply"
                && let Some((applied, errors)) = words::apply_errors(&out.text)
                && !errors.is_empty()
            {
                warning = Some(format!(
                    "{} of {} not applied — {}",
                    errors.len(),
                    applied + errors.len(),
                    errors[0]
                ));
                failed = failed || applied == 0;
            }
            self.edit(|s| {
                let now = SystemTime::now();
                // The transcript may have grown under the step.
                if let Some(step) = s.items.iter_mut().find(|i| i.id == step_id) {
                    step.running = false;
                    step.ok = !failed;
                    step.ms = millis_since(step.at, now);
                    step.text = words::summary(&args, &out.text);
                    if let Some(line) = out.line.as_deref().filter(|l| !l.is_empty()) {
                        step.title = line.to_string();
                    }
                    if let Some(w) = &warning {
                        step.warning = w.clone();
                    } else if failed {
                        step.warning = words::first_line(&out.text);
                    }
                }
                let text: String = out.text.chars().take(RESULT_LIMIT).collect();
                s.messages.push(tool_result(&id, text));
            });
            pictures.extend(out.pictures);
        }
        if !pictures.is_empty() {
            let n = pictures.len();
            let plural = if n == 1 { "" } else { "s" };
            let mut parts = vec![json!({
                "type": "text",
                "text": format!("Screenshot{plural} from the tool call{plural} above:"),
            })];
            for (data, mime) in pictures.iter().take(PICTURE_LIMIT) {
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(data);
                parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": format!("data:{mime};base64,{b64}") },
                }));
            }
            self.edit(|s| {
                s.messages
                    .push(message(json!({ "role": "user", "content": parts })))
            });
        }
        if *cutoffs >= history::CUT_LIMIT {
            self.finish(Item::failure(words::CUT_GIVE_UP));
            return Turn::Done;
        }
        Turn::Again
    }

    /// The question ends with `note`.
    fn finish(&self, note: Item) {
        self.edit(|s| {
            s.items.push(note);
            s.status.clear();
        });
    }
}

/// What a reply leaves the loop to do.
enum Turn {
    /// Ask the model again.
    Again,
    /// The question is over.
    Done,
}

fn tool_result(id: &str, content: String) -> Message {
    message(json!({ "role": "tool", "tool_call_id": id, "content": content }))
}

/// The tools in the chat shape (`Agent.modelTools`).
pub fn model_tools(catalogue: &[Value]) -> Vec<Value> {
    catalogue
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name")?.as_str()?;
            Some(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": tool.get("description").cloned().unwrap_or(json!("")),
                    "parameters": tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or(json!({ "type": "object", "properties": {} })),
                },
            }))
        })
        .collect()
}

/// Tool-call rounds per question in force, and whether Copper Cloud set
/// them (`Agent.budget`).
pub fn budget(local: i64, cloud: Option<i64>) -> (i64, bool) {
    match cloud {
        Some(n) if (1..=500).contains(&n) => (n, true),
        _ => (local.clamp(1, 500), false),
    }
}
