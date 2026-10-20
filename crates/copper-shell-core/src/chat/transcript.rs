//! The conversation as the person reads it (`Agent.Item`, `Drive.Run`), and
//! as `copper.agent.state().items` hands it to an addon.
//!
//! Items are kept flat, as the Mac keeps them: a question, each tool step
//! and the model's words on the way to it, the answer, notes, outside
//! drivers. [`items_json`] folds a question's steps and narration into one
//! `activity` item (`AgentPane.blocks`) whose `detail` is a JSON string:
//!
//! ```json
//! {"started":"…Z","ended":"…Z","current":"Reading the page…",
//!  "steps":[{"tool":"browser_snapshot","title":"Read the page","ms":412,"ok":true,"running":false,
//!            "warning":"…","error":"…","aside":false,"detail":"url: … → …",
//!            "jev":{"cycles":[{"text":"Clicked “Search”","ms":812,"ok":true,"running":false}],
//!                   "earlier":0,"status":"Done — found it","failed":false}}]}
//! ```
//!
//! Its `text` is the one line the Mac shows: the step in flight while the
//! question runs, "Worked for 4 s · 3 steps" once it is answered.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use copper_core::IsoDate;
use copper_mcp::drive::{Outcome, Status};
use serde_json::{Map, Value, json};

use super::words;

/// What an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    User,
    Assistant,
    Tool,
    Note,
    Drive,
}

/// One thing in the transcript (`Agent.Item`).
#[derive(Clone, Debug)]
pub struct Item {
    /// A lowercase UUID.
    pub id: String,
    pub kind: Kind,
    /// The words; for a step, the arguments that matter and the answer's
    /// first line ([`words::summary`]).
    pub text: String,
    /// For a step: the tool's name.
    pub tool: String,
    pub ok: bool,
    /// For a step: how long it took.
    pub ms: f64,
    /// When it landed; for a step, when it started.
    pub at: SystemTime,
    /// The model's words on its way to a tool call: narration inside the
    /// activity, not the answer.
    pub aside: bool,
    /// A step still in flight, or an outside driver still at work.
    pub running: bool,
    /// A step as a person says it; an outside driver's name.
    pub title: String,
    /// The Jev run a step started, or an outside driver's run.
    pub run: Option<u64>,
    /// What went partly wrong in a step that still ran.
    pub warning: String,
    /// For an outside driver: whose hands (`Who::key`).
    pub who: String,
}

impl Item {
    pub fn new(kind: Kind, text: impl Into<String>) -> Item {
        Item {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            text: text.into(),
            tool: String::new(),
            ok: true,
            ms: 0.0,
            at: SystemTime::now(),
            aside: false,
            running: false,
            title: String::new(),
            run: None,
            warning: String::new(),
            who: String::new(),
        }
    }

    /// A note that says something went wrong.
    pub fn failure(text: impl Into<String>) -> Item {
        Item {
            ok: false,
            ..Item::new(Kind::Note, text)
        }
    }

    /// When a step ended (or the item landed).
    fn ended(&self) -> SystemTime {
        if self.kind == Kind::Tool {
            self.at + Duration::from_secs_f64(self.ms.max(0.0) / 1000.0)
        } else {
            self.at
        }
    }
}

/// One move of a Jev run (`Drive.Cycle`).
#[derive(Clone, Debug, Default)]
pub struct Cycle {
    /// The phases' titles, in order ("Reading the page", "Asking Jev").
    pub phases: Vec<String>,
    pub outcome: Option<Outcome>,
    pub started: Option<SystemTime>,
    pub ended: Option<SystemTime>,
}

/// A Jev run, or an outside driver's stretch of work (`Drive.Run`).
#[derive(Clone, Debug)]
pub struct Run {
    /// `Who::key` of the hands.
    pub who: String,
    pub jev: bool,
    pub goal: String,
    pub cycles: Vec<Cycle>,
    pub status: Status,
    pub note: String,
}

impl Run {
    pub fn new(who: &str, jev: bool, goal: &str) -> Run {
        Run {
            who: who.to_string(),
            jev,
            goal: goal.to_string(),
            cycles: Vec::new(),
            status: Status::Running,
            note: String::new(),
        }
    }

    /// How the run stands, in one line (`Drive.Run.statusLine(busy: false)`).
    pub fn status_line(&self) -> String {
        let acted = self.cycles.iter().filter(|c| c.outcome.is_some()).count();
        let word = if self.jev { "action" } else { "call" };
        let count = format!("{acted} {word}{}", if acted == 1 { "" } else { "s" });
        let said = |head: &str| {
            if self.note.is_empty() {
                head.to_string()
            } else {
                format!("{head} — {}", self.note)
            }
        };
        match self.status {
            Status::Running if self.jev => format!("Running · {count}"),
            Status::Running => format!("Thinking · {count}"),
            Status::Done => said("Done"),
            Status::Blocked => said("Blocked"),
            Status::Budget => said("Out of steps"),
            Status::Error => said("Error"),
            Status::Ended => said("Let go"),
            Status::Stopped if self.note.is_empty() => "Stopped by you".into(),
            Status::Stopped => self.note.clone(),
        }
    }

    /// Whether it ended badly enough to say so in red.
    pub fn failed(&self) -> bool {
        matches!(self.status, Status::Error | Status::Blocked)
    }

    /// The phase in flight, for the step's running line.
    pub fn phase(&self) -> Option<&str> {
        self.cycles.last()?.phases.last().map(String::as_str)
    }
}

/// Swift's `capitalized` for an operation word: "TYPE" → "Type".
fn capitalized(word: &str) -> String {
    let lower = word.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A Jev cycle as a sentence (`DriveStep.line`, `jev: true`).
pub fn cycle_line(cycle: &Cycle) -> String {
    let Some(outcome) = &cycle.outcome else {
        return cycle
            .phases
            .last()
            .cloned()
            .unwrap_or_else(|| "Starting".into());
    };
    if let Some(error) = outcome.error.as_deref().filter(|e| !e.is_empty()) {
        let head = cycle
            .phases
            .first()
            .cloned()
            .unwrap_or_else(|| capitalized(&outcome.operation));
        let first = error.split('\n').find(|l| !l.is_empty()).unwrap_or(error);
        return format!("{head} — {first}");
    }
    match outcome.operation.as_str() {
        "DONE" => "Said it is done".into(),
        "BLOCKED" => "Said it is blocked".into(),
        "TYPE_TEXT" => {
            let text = outcome
                .text
                .as_deref()
                .map(|t| format!(" “{t}”"))
                .unwrap_or_default();
            format!("Typed{text} into {}", outcome.label)
        }
        op => {
            if outcome.stale {
                return "Page moved — reading it again".into();
            }
            let verb = match op {
                "CLICK" => "Clicked".to_string(),
                "SELECT" => "Chose".to_string(),
                "SCROLL" => "Scrolled".to_string(),
                "WAIT" => "Waited for".to_string(),
                "GO" => "Opened".to_string(),
                other => capitalized(other),
            };
            if outcome.label.is_empty() {
                verb
            } else {
                format!("{verb} {}", outcome.label)
            }
        }
    }
}

/// RFC 3339, whole seconds, UTC (the Mac's `AddonTime.text`).
pub fn rfc3339(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    IsoDate(secs).to_string()
}

fn seconds_between(a: SystemTime, b: SystemTime) -> f64 {
    b.duration_since(a).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// One question's activity while it is being folded (`AgentActivityGroup`).
struct Group {
    id: String,
    items: Vec<Item>,
    started: SystemTime,
    running: bool,
}

/// What [`items_json`] lays out, in order.
enum Block {
    Plain(Item),
    Activity(Group),
}

/// The transcript as `copper.agent.state().items`. `busy`: a question is
/// running; `status`: what it is doing ("Thinking…").
pub fn items_json(
    items: &[Item],
    runs: &HashMap<u64, Run>,
    busy: bool,
    status: &str,
) -> Vec<Value> {
    let mut out: Vec<Block> = Vec::new();
    let mut turn_start = items.first().map_or_else(SystemTime::now, |i| i.at);
    let mut current: Option<usize> = None;
    let mut last_user: Option<usize> = None;
    for item in items {
        match item.kind {
            Kind::Tool => fold(&mut out, &mut current, item, turn_start),
            Kind::Assistant if item.aside => fold(&mut out, &mut current, item, turn_start),
            Kind::User => {
                out.push(Block::Plain(item.clone()));
                turn_start = item.at;
                current = None;
                last_user = Some(out.len() - 1);
            }
            Kind::Assistant | Kind::Note | Kind::Drive => {
                current = None;
                out.push(Block::Plain(item.clone()));
            }
        }
    }
    // Only the newest question can still be running.
    if busy {
        let after_user = |i: usize| last_user.is_none_or(|u| i > u);
        let running = (0..out.len())
            .rev()
            .find(|&i| {
                after_user(i)
                    && matches!(&out[i], Block::Activity(g) if g.items.iter().any(|it| it.running))
            })
            .or(current.filter(|&i| after_user(i)));
        if let Some(i) = running
            && let Block::Activity(g) = &mut out[i]
        {
            g.running = true;
        }
    }
    out.into_iter()
        .map(|b| match b {
            Block::Plain(item) => plain_json(&item, runs),
            Block::Activity(g) => activity_json(&g, runs, status),
        })
        .collect()
}

fn fold(out: &mut Vec<Block>, current: &mut Option<usize>, item: &Item, started: SystemTime) {
    if let Some(i) = *current
        && let Some(Block::Activity(g)) = out.get_mut(i)
    {
        g.items.push(item.clone());
        return;
    }
    out.push(Block::Activity(Group {
        id: item.id.clone(),
        items: vec![item.clone()],
        started,
        running: false,
    }));
    *current = Some(out.len() - 1);
}

/// A question, an answer, a note or an outside driver (`AddonBridge.item`).
fn plain_json(item: &Item, runs: &HashMap<u64, Run>) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(item.id));
    out.insert("at".into(), json!(rfc3339(item.at)));
    let (role, text) = match item.kind {
        Kind::User => ("user", item.text.clone()),
        Kind::Assistant => ("assistant", item.text.clone()),
        Kind::Note => (
            if item.ok { "activity" } else { "error" },
            item.text.clone(),
        ),
        Kind::Drive => {
            let text = if item.title.is_empty() {
                &item.text
            } else {
                &item.title
            };
            if !item.title.is_empty() && !item.text.is_empty() {
                out.insert("detail".into(), json!(item.text));
            }
            ("drive", text.clone())
        }
        Kind::Tool => ("activity", item.title.clone()),
    };
    out.insert("role".into(), json!(role));
    out.insert("text".into(), json!(text));
    let live = item.running
        || (item.kind == Kind::Drive
            && item
                .run
                .and_then(|r| runs.get(&r))
                .is_some_and(|r| r.jev && r.status == Status::Running));
    if live {
        out.insert("busy".into(), json!(true));
    }
    Value::Object(out)
}

/// The step in flight, in words; a Jev run under it says its own phase
/// (`AgentActivity.current`).
fn current_line(g: &Group, runs: &HashMap<u64, Run>, status: &str) -> String {
    let Some(step) = g
        .items
        .iter()
        .rev()
        .find(|i| i.kind == Kind::Tool && i.running)
    else {
        return if status.is_empty() {
            "Thinking…".into()
        } else {
            status.to_string()
        };
    };
    let title = if step.title.is_empty() {
        words::plain(&step.tool)
    } else {
        step.title.clone()
    };
    match step.run.and_then(|r| runs.get(&r)).and_then(Run::phase) {
        Some(phase) => format!("{title} · {phase}"),
        None => format!("{title}…"),
    }
}

/// The one line, the steps folded under it.
fn activity_json(g: &Group, runs: &HashMap<u64, Run>, status: &str) -> Value {
    let steps: Vec<&Item> = g.items.iter().filter(|i| i.kind == Kind::Tool).collect();
    let failures = steps.iter().filter(|s| !s.ok).count();
    let warnings = steps
        .iter()
        .filter(|s| s.ok && !s.warning.is_empty())
        .count();
    let ended = g.items.iter().map(Item::ended).max().unwrap_or(g.started);
    let mut detail = Map::new();
    detail.insert("started".into(), json!(rfc3339(g.started)));
    let text = if g.running {
        let line = current_line(g, runs, status);
        detail.insert("current".into(), json!(line));
        line
    } else {
        detail.insert("ended".into(), json!(rfc3339(ended)));
        let n = steps.len();
        let mut line = format!(
            "Worked for {} · {n} step{}",
            words::span(seconds_between(g.started, ended)),
            if n == 1 { "" } else { "s" }
        );
        if failures > 0 {
            line.push_str(&format!(" · {failures} failed"));
        } else if warnings > 0 {
            line.push_str(&format!(" · {warnings} with errors"));
        }
        line
    };
    let list: Vec<Value> = g.items.iter().map(|i| step_json(i, runs)).collect();
    detail.insert("steps".into(), Value::Array(list));
    let mut out = json!({
        "id": g.id,
        "role": "activity",
        "text": text,
        "at": rfc3339(g.started),
        "detail": Value::Object(detail).to_string(),
    });
    if g.running {
        out["busy"] = json!(true);
    }
    out
}

/// At most this many of a Jev run's moves are listed; the rest are counted.
const SHOWN_CYCLES: usize = 8;

fn step_json(item: &Item, runs: &HashMap<u64, Run>) -> Value {
    if item.kind != Kind::Tool {
        return json!({
            "tool": "",
            "title": item.text,
            "ms": 0,
            "ok": true,
            "running": false,
            "aside": true,
        });
    }
    let mut out = Map::new();
    out.insert("tool".into(), json!(item.tool));
    let title = if item.title.is_empty() {
        words::plain(&item.tool)
    } else {
        item.title.clone()
    };
    out.insert("title".into(), json!(title));
    out.insert("ms".into(), json!(item.ms.round() as u64));
    out.insert("ok".into(), json!(item.ok));
    out.insert("running".into(), json!(item.running));
    if !item.warning.is_empty() {
        out.insert("warning".into(), json!(item.warning));
    } else if !item.ok
        && let Some(reason) = item
            .text
            .split('\n')
            .rfind(|l| !l.is_empty())
            .and_then(|l| l.strip_prefix("→ "))
    {
        out.insert("error".into(), json!(reason));
    }
    if !item.text.is_empty() {
        out.insert("detail".into(), json!(item.text));
    }
    if let Some(run) = item.run.and_then(|r| runs.get(&r)) {
        out.insert("jev".into(), run_json(run));
    }
    Value::Object(out)
}

fn run_json(run: &Run) -> Value {
    let shown = run.cycles.len().min(SHOWN_CYCLES);
    let earlier = run.cycles.len() - shown;
    let cycles: Vec<Value> = run.cycles[earlier..]
        .iter()
        .map(|c| {
            let ms = match (c.started, c.ended) {
                (Some(a), Some(b)) => (seconds_between(a, b) * 1000.0).round() as u64,
                _ => 0,
            };
            json!({
                "text": cycle_line(c),
                "ms": ms,
                "ok": c.outcome.as_ref().and_then(|o| o.error.as_deref()).is_none_or(str::is_empty),
                "running": c.ended.is_none(),
            })
        })
        .collect();
    json!({
        "cycles": cycles,
        "earlier": earlier,
        "status": run.status_line(),
        "failed": run.failed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(op: &str, label: &str) -> Outcome {
        Outcome {
            operation: op.into(),
            label: label.into(),
            ..Outcome::default()
        }
    }

    fn cycle(phases: &[&str], o: Option<Outcome>) -> Cycle {
        Cycle {
            phases: phases.iter().map(|s| s.to_string()).collect(),
            outcome: o,
            ..Cycle::default()
        }
    }

    #[test]
    fn cycles_in_words() {
        assert_eq!(cycle_line(&cycle(&[], None)), "Starting");
        assert_eq!(
            cycle_line(&cycle(&["Reading the page"], None)),
            "Reading the page"
        );
        assert_eq!(
            cycle_line(&cycle(&[], Some(outcome("CLICK", "“Search”")))),
            "Clicked “Search”"
        );
        assert_eq!(
            cycle_line(&cycle(&[], Some(outcome("DONE", "")))),
            "Said it is done"
        );
        assert_eq!(cycle_line(&cycle(&[], Some(outcome("KEY", "")))), "Key");
        let mut typed = outcome("TYPE_TEXT", "Search");
        typed.text = Some("Copper".into());
        assert_eq!(
            cycle_line(&cycle(&[], Some(typed))),
            "Typed “Copper” into Search"
        );
        let mut failed = outcome("CLICK", "x");
        failed.error = Some("gone\nmore".into());
        assert_eq!(
            cycle_line(&cycle(&["Clicking x"], Some(failed))),
            "Clicking x — gone"
        );
    }

    #[test]
    fn run_lines() {
        let mut run = Run::new("jev:pane", true, "g");
        run.cycles.push(cycle(&[], Some(outcome("CLICK", ""))));
        assert_eq!(run.status_line(), "Running · 1 action");
        run.status = Status::Done;
        run.note = "found it".into();
        assert_eq!(run.status_line(), "Done — found it");
        run.status = Status::Stopped;
        run.note.clear();
        assert_eq!(run.status_line(), "Stopped by you");
        run.status = Status::Blocked;
        assert!(run.failed());
    }

    #[test]
    fn a_question_folds_into_one_activity() {
        let t0 = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut user = Item::new(Kind::User, "q");
        user.at = t0;
        let mut aside = Item::new(Kind::Assistant, "Looking.");
        aside.aside = true;
        aside.at = t0;
        let mut a = Item::new(Kind::Tool, "url: x\n→ fine");
        a.tool = "browser_snapshot".into();
        a.title = "Read the page".into();
        a.at = t0 + Duration::from_secs(1);
        a.ms = 412.4;
        let mut b = Item::new(Kind::Tool, "→ no such ref");
        b.tool = "browser_click".into();
        b.ok = false;
        b.at = t0 + Duration::from_secs(2);
        b.ms = 2000.0;
        let mut answer = Item::new(Kind::Assistant, "**Example Domain**");
        answer.at = t0 + Duration::from_secs(5);
        let items = vec![user, aside, a.clone(), b, answer];
        let out = items_json(&items, &HashMap::new(), false, "");
        let roles: Vec<&str> = out.iter().map(|v| v["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "activity", "assistant"]);
        assert_eq!(out[1]["text"], "Worked for 4 s · 2 steps · 1 failed");
        assert_eq!(out[1]["id"], items[1].id);
        assert!(out[1].get("busy").is_none());
        let detail: Value = serde_json::from_str(out[1]["detail"].as_str().unwrap()).unwrap();
        assert_eq!(detail["started"], "2027-01-15T08:00:00Z");
        assert_eq!(detail["ended"], "2027-01-15T08:00:04Z");
        let steps = detail["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["aside"], true);
        assert_eq!(steps[0]["title"], "Looking.");
        assert_eq!(steps[1]["tool"], "browser_snapshot");
        assert_eq!(steps[1]["title"], "Read the page");
        assert_eq!(steps[1]["ms"], 412);
        assert_eq!(steps[1]["ok"], true);
        assert_eq!(steps[2]["error"], "no such ref");
        assert_eq!(steps[2]["title"], "browser click");
        assert_eq!(out[2]["text"], "**Example Domain**");
    }

    #[test]
    fn a_running_question_says_its_step() {
        let user = Item::new(Kind::User, "q");
        let mut step = Item::new(Kind::Tool, "");
        step.tool = "jev_run".into();
        step.title = "Jev: find it".into();
        step.running = true;
        step.run = Some(1);
        let mut runs = HashMap::new();
        let mut run = Run::new("jev:pane", true, "find it");
        run.cycles.push(cycle(&["Reading the page"], None));
        runs.insert(1, run);
        let out = items_json(&[user.clone(), step], &runs, true, "Jev: find it…");
        assert_eq!(out[1]["busy"], true);
        assert_eq!(out[1]["text"], "Jev: find it · Reading the page");
        let detail: Value = serde_json::from_str(out[1]["detail"].as_str().unwrap()).unwrap();
        assert!(detail.get("ended").is_none());
        assert_eq!(detail["current"], "Jev: find it · Reading the page");
        let jev = &detail["steps"][0]["jev"];
        assert_eq!(jev["cycles"][0]["text"], "Reading the page");
        assert_eq!(jev["cycles"][0]["running"], true);
        assert_eq!(jev["status"], "Running · 0 actions");
        assert_eq!(jev["earlier"], 0);
        // Asked, nothing back yet: no activity at all.
        let out = items_json(&[user], &runs, true, "Thinking…");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn notes_and_drivers() {
        let stopped = Item::new(Kind::Note, "Stopped");
        let failed = Item::failure("router 500: x");
        let mut drive = Item::new(Kind::Drive, "Download Copper");
        drive.title = "Claude Code".into();
        drive.running = true;
        let out = items_json(&[stopped, failed, drive], &HashMap::new(), false, "");
        assert_eq!(out[0]["role"], "activity");
        assert!(out[0].get("detail").is_none());
        assert_eq!(out[1]["role"], "error");
        assert_eq!(out[2]["role"], "drive");
        assert_eq!(out[2]["text"], "Claude Code");
        assert_eq!(out[2]["detail"], "Download Copper");
        assert_eq!(out[2]["busy"], true);
    }
}
