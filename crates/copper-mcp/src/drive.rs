//! Who has the wheel and what they are doing with it (Fork/MCP/Drive.swift,
//! the model only — the pane, the pill and the tab badges are the app's).
//!
//! The server tells the host what happens as [`DriveEvent`]s; the one thing
//! that travels back is Stop ([`crate::Mcp::stop`]): a Jev run checks it
//! before its next action, any other agent has its next calls refused for
//! [`REFUSAL`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::host::TabId;
use crate::who::Who;

/// How long a stopped agent's calls are turned away.
pub const REFUSAL: Duration = Duration::from_secs(30);

/// The words a refused call answers with.
pub const REFUSAL_TEXT: &str = "Stopped by the user in Copper: they took the browser back. Do not retry; tell them what you were doing and wait until they ask you to continue.";

/// A phase of a Jev cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseKind {
    Observe,
    Ask,
    Write,
    Act,
    Settle,
}

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Done,
    Blocked,
    Budget,
    Error,
    Stopped,
    Ended,
}

impl Status {
    pub fn parse(raw: &str) -> Status {
        match raw {
            "running" | "ready" => Status::Running,
            "done" => Status::Done,
            "blocked" => Status::Blocked,
            "budget" => Status::Budget,
            "stopped" => Status::Stopped,
            "ended" => Status::Ended,
            _ => Status::Error,
        }
    }
}

/// What came of a cycle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// CLICK / TYPE_TEXT / SELECT / SCROLL / WAIT / DONE / BLOCKED / READ / GO …
    pub operation: String,
    pub label: String,
    pub text: Option<String>,
    pub probability: f64,
    pub confidence: f64,
    pub page_changed: Option<bool>,
    pub stale: bool,
    pub candidates: Vec<String>,
    pub error: Option<String>,
    pub result: Option<String>,
}

/// One thing the driver pane should know.
#[derive(Clone, Debug, PartialEq)]
pub enum DriveEvent {
    /// A tool call is starting (not jev_run / jev_step, which own a run).
    CallBegan {
        ticket: u64,
        who: Who,
        tool: String,
        title: String,
        detail: Option<String>,
        tab: Option<TabId>,
        behind: bool,
    },
    /// It returned.
    CallEnded {
        ticket: u64,
        who: Who,
        tool: String,
        outcome: Outcome,
        tab: Option<TabId>,
    },
    /// The user's Stop turned a call away.
    CallRefused {
        who: Who,
        tool: String,
    },
    /// A Jev run began on a tab.
    RunBegan {
        who: Who,
        goal: String,
        tab: TabId,
    },
    /// A new read → ask → act → settle cycle.
    Cycle,
    Phase {
        id: u64,
        kind: PhaseKind,
        title: String,
    },
    PhaseClosed {
        id: u64,
        detail: Option<String>,
    },
    Outcome(Outcome),
    Page {
        url: String,
        title: String,
    },
    RunFinished {
        status: Status,
        note: String,
    },
}

/// The model's state.
#[derive(Debug, Default)]
pub struct Drive {
    refused: HashMap<String, Instant>,
    /// A live Jev run: whose and on which tab.
    pub(crate) jev: Option<(Who, TabId)>,
    pub(crate) stop_requested: bool,
    next: u64,
}

impl Drive {
    pub(crate) fn ticket(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Turn one session's calls away for [`REFUSAL`].
    pub fn refuse(&mut self, key: &str) {
        self.refused
            .insert(key.to_string(), Instant::now() + REFUSAL);
    }

    /// Let everyone back in.
    pub fn resume(&mut self) {
        self.refused.clear();
    }

    /// The refusal text when `key` is stopped.
    pub fn refusal(&mut self, key: &str) -> Option<&'static str> {
        let now = Instant::now();
        self.refused.retain(|_, until| *until > now);
        self.refused.contains_key(key).then_some(REFUSAL_TEXT)
    }

    /// Stop the hand `key`: the live Jev run if it is that one, else refuse.
    pub fn stop(&mut self, key: &str) {
        match &self.jev {
            Some((who, _)) if who.key == key => self.stop_requested = true,
            _ => self.refuse(key),
        }
    }
}

fn arg_str<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn host_of(raw: &str) -> String {
    match copper_core::url_from_typed(raw) {
        Some(url) => url
            .host_str()
            .map(str::to_string)
            .unwrap_or_else(|| url.to_string()),
        None => raw.to_string(),
    }
}

/// Whether a field label sounds like a secret (`JevProgress.looksSecret`).
pub fn looks_secret(label: &str) -> bool {
    let l = label.to_lowercase();
    if [
        "password",
        "passcode",
        "passphrase",
        "passwd",
        "one-time",
        "verification code",
        "security code",
        "card number",
        "social security",
    ]
    .iter()
    .any(|w| l.contains(w))
    {
        return true;
    }
    l.split(|c: char| !c.is_alphanumeric()).any(|w| {
        [
            "otp", "totp", "2fa", "mfa", "cvv", "cvc", "csc", "pin", "ssn", "secret", "token",
        ]
        .contains(&w)
    })
}

/// The row's title and detail for a tool call (`Drive.words`).
pub fn words(tool: &str, args: &Map<String, Value>) -> (String, Option<String>) {
    let element = arg_str(args, "element")
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let reference = arg_str(args, "ref").or_else(|| arg_str(args, "selector"));
    let named = element
        .map(str::to_string)
        .or_else(|| reference.map(|r| format!("'{r}'")))
        .unwrap_or_default();
    let detail = arg_str(args, "reason")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let host = arg_str(args, "url").map(host_of).unwrap_or_default();
    let sentence = |verb: &str| {
        if named.is_empty() {
            verb.to_string()
        } else {
            format!("{verb} {named}")
        }
    };
    let flag = |k: &str| args.get(k).and_then(Value::as_bool) == Some(true);
    let title = match tool {
        "browser_snapshot" => if flag("interactive") {
            "Reading the page's controls"
        } else {
            "Reading the page"
        }
        .to_string(),
        "browser_click" => {
            let how = if flag("doubleClick") {
                "Double-clicking"
            } else if arg_str(args, "button") == Some("right") {
                "Right-clicking"
            } else {
                "Clicking"
            };
            sentence(how)
        }
        "browser_type" => {
            if named.is_empty() {
                "Typing".to_string()
            } else {
                format!(
                    "Typing into {named}{}",
                    if flag("submit") { ", then Enter" } else { "" }
                )
            }
        }
        "browser_fill_form" => {
            let n = args
                .get("fields")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("Filling {n} field{}", if n == 1 { "" } else { "s" })
        }
        "browser_press_key" => format!("Pressing {}", arg_str(args, "key").unwrap_or("a key")),
        "browser_hover" => sentence("Hovering"),
        "browser_select_option" => {
            let values = string_list(args.get("values")).join(", ");
            if named.is_empty() {
                format!("Choosing {values}")
            } else {
                format!("Choosing {values} in {named}")
            }
        }
        "browser_drag" => format!(
            "Dragging {} to {}",
            arg_str(args, "startElement")
                .or_else(|| arg_str(args, "startRef"))
                .unwrap_or(""),
            arg_str(args, "endElement")
                .or_else(|| arg_str(args, "endRef"))
                .unwrap_or("")
        ),
        "browser_navigate" => {
            if host.is_empty() {
                "Opening a page".into()
            } else {
                format!("Opening {host}")
            }
        }
        "browser_navigate_back" => "Going back".into(),
        "browser_navigate_forward" => "Going forward".into(),
        "browser_take_screenshot" => if flag("fullPage") {
            "Taking a full-page screenshot"
        } else {
            "Taking a screenshot"
        }
        .into(),
        "browser_evaluate" => "Running a script in the page".into(),
        "browser_wait_for" => {
            if let Some(text) = arg_str(args, "text").filter(|t| !t.is_empty()) {
                format!("Waiting for “{}”", prefix(text, 40))
            } else if let Some(gone) = arg_str(args, "textGone").filter(|t| !t.is_empty()) {
                format!("Waiting for “{}” to go", prefix(gone, 40))
            } else if let Some(t) = args.get("time").and_then(Value::as_f64) {
                if t.round() == t {
                    format!("Waiting {} s", t as i64)
                } else {
                    format!("Waiting {t:.1} s")
                }
            } else {
                "Waiting".into()
            }
        }
        "browser_scroll" => {
            if let Some(dy) = args.get("deltaY").and_then(Value::as_f64) {
                if dy < 0.0 {
                    "Scrolling up".into()
                } else {
                    "Scrolling down".into()
                }
            } else if let Some(r) = arg_str(args, "ref") {
                format!("Scrolling '{r}' into view")
            } else {
                "Scrolling down".into()
            }
        }
        "browser_get_text" => {
            if named.is_empty() {
                "Reading the page's text".into()
            } else {
                format!("Reading the text of {named}")
            }
        }
        "browser_console_messages" => "Reading the console".into(),
        "browser_find" => format!(
            "Finding “{}”",
            prefix(arg_str(args, "text").unwrap_or(""), 40)
        ),
        "browser_resize" => "Resizing the window".into(),
        "browser_close" => "Closing the tab".into(),
        "browser_tabs" => match arg_str(args, "action").unwrap_or("list") {
            "new" => {
                if host.is_empty() {
                    "Opening a new tab".into()
                } else {
                    format!("Opening a tab on {host}")
                }
            }
            "close" => "Closing a tab".into(),
            "select" => "Switching tabs".into(),
            _ => "Listing the tabs".into(),
        },
        "browser_groups" => format!("Tab groups: {}", arg_str(args, "action").unwrap_or("list")),
        "browser_sign_in" => if arg_str(args, "what") == Some("otp") {
            "Filling the one-time code from the vault"
        } else {
            "Signing in with a saved account"
        }
        .into(),
        "browser_autofill" => format!(
            "Filling a saved {}",
            arg_str(args, "kind").unwrap_or("item")
        ),
        "browser_perf_probe" => "Sampling the page's performance".into(),
        "browser_crashes" => "Reading Copper's crash history".into(),
        "jev_observe" => "Reading the page (Jev)".into(),
        "canvas_read" => "Reading the canvas".into(),
        "canvas_apply" => {
            let ops = args
                .get("ops")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if ops > 0 {
                format!(
                    "Changing the canvas · {ops} op{}",
                    if ops == 1 { "" } else { "s" }
                )
            } else {
                "Changing the canvas".into()
            }
        }
        "canvas_list" => "Listing canvases".into(),
        "canvas_open" => "Opening a canvas".into(),
        "canvas_create" => "Making a canvas".into(),
        "canvas_select" | "canvas_focus" => "Pointing at shapes on the canvas".into(),
        "canvas_invite" => "Inviting someone to a canvas".into(),
        "canvas_screenshot" => "Picturing the canvas".into(),
        "jev_extract" => "Extracting values (Jev)".into(),
        "cloud_history_delete" => "Deleting history on Copper Cloud".into(),
        _ => {
            let plain = tool.strip_prefix("browser_").unwrap_or(tool);
            capitalized(&plain.replace('_', " "))
        }
    };
    (title, detail)
}

/// Foundation's `capitalized`: every word's first letter up, the rest down.
fn capitalized(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn string_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The chip: one word for what kind of move it was (`Drive.operation`).
pub fn operation(tool: &str, args: &Map<String, Value>) -> String {
    match tool {
        "browser_click" => "CLICK",
        "browser_type" | "browser_fill_form" => "TYPE",
        "browser_press_key" => "KEY",
        "browser_hover" => "HOVER",
        "browser_select_option" => "SELECT",
        "browser_drag" => "DRAG",
        "browser_navigate" | "browser_navigate_back" | "browser_navigate_forward" => "GO",
        "browser_take_screenshot" => "SHOT",
        "browser_evaluate" => "SCRIPT",
        "browser_wait_for" => "WAIT",
        "browser_scroll" => "SCROLL",
        "browser_snapshot"
        | "browser_get_text"
        | "browser_console_messages"
        | "browser_find"
        | "jev_observe"
        | "jev_extract"
        | "browser_crashes" => "READ",
        "browser_tabs" => match arg_str(args, "action").unwrap_or("list") {
            "new" | "close" | "select" => "TABS",
            _ => "READ",
        },
        "browser_close" => "TABS",
        "browser_sign_in" => "SIGN_IN",
        "browser_autofill" => "AUTOFILL",
        "browser_perf_probe" => "PROBE",
        "canvas_read" | "canvas_list" => "READ",
        t if t.starts_with("canvas_") => "CANVAS",
        "cloud_history_delete" => "CLOUD",
        _ => return tool.to_uppercase().chars().take(12).collect(),
    }
    .to_string()
}

/// Whether the call can move the page (`Drive.mayChange`).
pub fn may_change(tool: &str) -> bool {
    !matches!(
        tool,
        "browser_snapshot"
            | "browser_get_text"
            | "browser_console_messages"
            | "browser_find"
            | "browser_take_screenshot"
            | "browser_hover"
            | "browser_perf_probe"
            | "jev_observe"
            | "jev_extract"
            | "browser_resize"
            | "browser_groups"
            | "browser_wait_for"
            | "browser_crashes"
            | "cloud_history_delete"
    ) && !tool.starts_with("canvas_")
}

/// The outcome's label: the thing acted on (`Drive.target`).
pub fn target(tool: &str, args: &Map<String, Value>) -> String {
    if let Some(e) = arg_str(args, "element")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return e.to_string();
    }
    let s = |k: &str| arg_str(args, k).unwrap_or("").to_string();
    match tool {
        "browser_navigate" => arg_str(args, "url").map(host_of).unwrap_or_default(),
        "browser_press_key" => s("key"),
        "browser_find" | "browser_wait_for" => arg_str(args, "text")
            .or_else(|| arg_str(args, "textGone"))
            .unwrap_or("")
            .to_string(),
        "browser_drag" => format!("{} → {}", s("startRef"), s("endRef")),
        "browser_select_option" => string_list(args.get("values")).join(", "),
        "browser_autofill" => arg_str(args, "name")
            .or_else(|| arg_str(args, "kind"))
            .unwrap_or("")
            .to_string(),
        "browser_sign_in" => s("account"),
        _ => arg_str(args, "ref")
            .or_else(|| arg_str(args, "selector"))
            .unwrap_or("")
            .to_string(),
    }
}

/// What was typed, for the row — masked when the field sounds secret.
pub fn typed(tool: &str, args: &Map<String, Value>) -> Option<String> {
    if tool != "browser_type" {
        return None;
    }
    let text = arg_str(args, "text").filter(|t| !t.is_empty())?;
    let label = format!(
        "{} {}",
        arg_str(args, "element").unwrap_or(""),
        arg_str(args, "ref").unwrap_or("")
    );
    Some(if looks_secret(&label) {
        "•••".into()
    } else {
        text.into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn words_for_calls() {
        assert_eq!(
            words(
                "browser_click",
                &m(json!({"element": "Search", "reason": "go"}))
            ),
            ("Clicking Search".to_string(), Some("go".to_string()))
        );
        assert_eq!(
            words("browser_navigate", &m(json!({"url": "example.com/x"}))).0,
            "Opening example.com"
        );
        assert_eq!(
            words("browser_wait_for", &m(json!({"time": 2}))).0,
            "Waiting 2 s"
        );
        assert_eq!(
            words("browser_mystery_tool", &m(json!({}))).0,
            "Mystery Tool"
        );
    }

    #[test]
    fn secrets_masked() {
        assert!(looks_secret("Password"));
        assert!(looks_secret("PIN code"));
        assert!(!looks_secret("Shipping address"));
        assert_eq!(
            typed(
                "browser_type",
                &m(json!({"element": "PIN", "text": "1234"}))
            ),
            Some("•••".into())
        );
    }

    #[test]
    fn refusal_window() {
        let mut d = Drive::default();
        assert!(d.refusal("a").is_none());
        d.stop("a");
        assert_eq!(d.refusal("a"), Some(REFUSAL_TEXT));
        d.resume();
        assert!(d.refusal("a").is_none());
    }
}
