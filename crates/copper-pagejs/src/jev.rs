//! Jev's fast page read and actions (`window.__jevFast`, Fork/MCP/Ultrafast.swift),
//! plus the element table `jev_observe` prints (`Ultrafast.table`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::call::Call;
use crate::script::Script;

/// An observed control's box, viewport CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JevRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// One action the page offers: `click`, `fill`, `select` on a node, or the
/// controls `scroll` / `wait`. Serialises back exactly as observed, for
/// [`resolve`] and [`settle`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JevAction {
    pub id: String,
    pub kind: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<JevRect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expanded: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JevScroll {
    pub y: f64,
    pub height: f64,
}

/// `window.__jevFast.observe()` (`Ultrafast.Observation`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub url: String,
    pub title: String,
    pub w: f64,
    pub h: f64,
    pub text: String,
    pub scroll: JevScroll,
    pub actions: Vec<JevAction>,
    pub marker: String,
    pub page_key: String,
    /// Node id (as a string) → guard JSON, or null when the node is gone/hidden.
    pub guards: BTreeMap<String, Option<String>>,
    #[serde(default)]
    pub omitted_actions: u64,
}

impl Observation {
    pub fn guard_value(&self, node: u64) -> Option<&str> {
        self.guards
            .get(&node.to_string())
            .and_then(|g| g.as_deref())
    }

    pub fn action(&self, id: &str) -> Option<&JevAction> {
        self.actions.iter().find(|a| a.id == id)
    }
}

/// A point to put input, viewport CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The page as Jev sees it (`None` before there is a body).
pub fn observe() -> Call<Option<Observation>> {
    Call::new(Script::JevObserve, json!({}))
}

/// Whole-page freshness marker.
pub fn marker() -> Call<Option<String>> {
    Call::new(Script::JevMarker, json!({}))
}

/// `(page_key, guard)` for one node: compare with the observation's.
pub fn check(node: u64) -> Call<(String, Option<String>)> {
    Call::new(Script::JevCheck, json!({ "node": node }))
}

/// Where to put the input for `action` now, or `None` when the target moved,
/// hid or got covered. A `select` is performed here in full.
pub fn resolve(action: &JevAction) -> Call<Option<Point>> {
    Call::new(Script::JevResolve, json!({ "action": action }))
}

pub fn dom_click(node: u64) -> Call<bool> {
    Call::new(Script::JevDomClick, json!({ "node": node }))
}

pub fn select_all(node: u64) -> Call<bool> {
    Call::new(Script::JevSelectAll, json!({ "node": node }))
}

pub fn set_value(node: u64, text: &str) -> Call<bool> {
    Call::new(Script::JevSetValue, json!({ "node": node, "text": text }))
}

/// Wait for the page to settle after `action` (the Mac drops `rect` first and
/// caps the wait at 1 s).
pub fn settle(action: &JevAction) -> Call<bool> {
    let mut plain = action.clone();
    plain.rect = None;
    Call::new(Script::JevSettle, json!({ "action": plain }))
}

pub fn scroll_by(delta: f64) -> Call<bool> {
    Call::new(Script::JevScrollBy, json!({ "delta": delta }))
}

/// The agent-only controls a page can offer besides its own elements.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Extras {
    pub sign_in: bool,
    pub autofill_card: bool,
    pub autofill_identity: bool,
}

#[derive(Debug)]
struct Element<'a> {
    index: usize,
    role: Option<&'a str>,
    label: &'a str,
    value: Option<&'a str>,
    checked: Option<&'a str>,
    expanded: Option<&'a str>,
    operations: Vec<&'static str>,
    options: Vec<&'a str>,
}

fn operation(kind: &str) -> Option<&'static str> {
    match kind {
        "click" => Some("CLICK"),
        "fill" => Some("TYPE_TEXT"),
        "select" => Some("SELECT"),
        _ => None,
    }
}

/// One element per observed node, in observation order (`Ultrafast.actionSpace`).
fn elements(actions: &[JevAction]) -> Vec<Element<'_>> {
    let mut out: Vec<Element<'_>> = Vec::new();
    let mut index_of: BTreeMap<u64, usize> = BTreeMap::new();
    for action in actions {
        let Some(op) = operation(&action.kind) else {
            continue;
        };
        let Some(node) = action.node else { continue };
        let position = *index_of.entry(node).or_insert_with(|| {
            let select = action.kind == "select";
            out.push(Element {
                index: out.len() + 1,
                role: action.role.as_deref(),
                label: action.label.split(" → ").next().unwrap_or_default(),
                value: if select {
                    Some(action.current_value.as_deref().unwrap_or(""))
                } else {
                    action.value.as_deref()
                },
                checked: action.checked.as_deref(),
                expanded: action.expanded.as_deref(),
                operations: Vec::new(),
                options: Vec::new(),
            });
            out.len() - 1
        });
        let element = &mut out[position];
        if !element.operations.contains(&op) {
            element.operations.push(op);
        }
        if action.kind == "select" {
            element.options.push(&action.label);
        }
    }
    out
}

/// Swift's `prefix(n)` on a String (by character).
fn prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// NSString `padding(toLength:withPad:" ",startingAt:0)`: pad or cut to `n`.
fn pad(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    while out.chars().count() < n {
        out.push(' ');
    }
    out
}

/// The table an agent reads (`Ultrafast.table`): `[3] combobox   Where to? · London`.
pub fn table(obs: &Observation, limit: usize, extras: Extras) -> String {
    let elements = elements(&obs.actions);
    let mut lines: Vec<String> = Vec::new();
    for element in elements.iter().take(limit) {
        let label = element.label.replace('\n', " ");
        let mut line = format!(
            "[{}] {} {}",
            element.index,
            pad(element.role.unwrap_or(""), 10),
            prefix(&label, 90)
        );
        if let Some(value) = element.value.filter(|v| !v.is_empty()) {
            line += &format!(" · {}", prefix(value, 60));
        }
        if let Some(checked) = element.checked {
            line += &format!(" · checked={checked}");
        }
        if let Some(expanded) = element.expanded {
            line += &format!(" · expanded={expanded}");
        }
        if !element.options.is_empty() {
            let options: Vec<&str> = element
                .options
                .iter()
                .take(12)
                .map(|label| label.rsplit(" → ").next().unwrap_or_default())
                .collect();
            line += &format!(" · options: {}", options.join(" | "));
        }
        if element.operations.len() > 1 {
            let ops: Vec<String> = element
                .operations
                .iter()
                .map(|op| {
                    if *op == "TYPE_TEXT" {
                        "type".to_string()
                    } else {
                        op.to_lowercase()
                    }
                })
                .collect();
            line += &format!(" ({})", ops.join("/"));
        }
        lines.push(line);
    }
    if elements.len() > limit {
        lines.push(format!("… {} more", elements.len() - limit));
    }
    if obs.omitted_actions > 0 {
        lines.push(format!(
            "… {} beyond the 250-candidate cap",
            obs.omitted_actions
        ));
    }
    if extras.sign_in {
        lines.push("[SIGN_IN] Sign in with the saved account for this site (fills and submits the password for you)".into());
    }
    if extras.autofill_card {
        lines.push("[AUTOFILL_CARD] Fill the saved card — number, expiry, code — for you".into());
    }
    if extras.autofill_identity {
        lines.push(
            "[AUTOFILL_IDENTITY] Fill your saved name, address, email and phone for you".into(),
        );
    }
    if lines.is_empty() {
        "(no interactive elements in view)".into()
    } else {
        lines.join("\n")
    }
}

/// The read as a trace fragment (`Ultrafast.read`): `38 controls · Google Flights`.
pub fn read(obs: &Observation) -> String {
    let count = elements(&obs.actions).len();
    let mut out = format!("{count} control{}", if count == 1 { "" } else { "s" });
    let name = if obs.title.is_empty() {
        host(&obs.url).unwrap_or_default()
    } else {
        &obs.title
    };
    if !name.is_empty() {
        out += &format!(" · {}", prefix(name, 40));
    }
    out
}

/// `URL(string:)?.host` for the URLs a page reports.
fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then_some(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(id: &str, node: u64, kind: &str, role: &str, label: &str) -> JevAction {
        JevAction {
            id: id.into(),
            kind: kind.into(),
            label: label.into(),
            node: Some(node),
            role: Some(role.into()),
            rect: None,
            value: Some(String::new()),
            current_value: None,
            checked: None,
            selected: None,
            expanded: None,
            delta: None,
        }
    }

    fn obs(actions: Vec<JevAction>) -> Observation {
        Observation {
            url: "https://example.test/page/1".into(),
            title: "Title 1".into(),
            w: 1024.0,
            h: 768.0,
            text: String::new(),
            scroll: JevScroll {
                y: 0.0,
                height: 768.0,
            },
            actions,
            marker: String::new(),
            page_key: String::new(),
            guards: BTreeMap::new(),
            omitted_actions: 0,
        }
    }

    #[test]
    fn matches_the_mac_fixture() {
        // fixtures/mac/mcp-examples/jev_observe.json: 43 buttons, limit 10.
        let actions = (1..=43)
            .map(|i| {
                action(
                    &format!("e{i}"),
                    i,
                    "click",
                    "button",
                    &format!("Control {i}"),
                )
            })
            .collect();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/mac/mcp-examples/jev_observe.json"
        ))
        .unwrap();
        let text = fixture["result"]["content"][0]["text"].as_str().unwrap();
        let elements = text.split("### Elements\n").nth(1).unwrap();
        assert_eq!(table(&obs(actions), 10, Extras::default()), elements);
    }

    #[test]
    fn fill_select_and_extras() {
        let mut fill = action("e1", 1, "fill", "textbox", "Where to?");
        fill.value = Some("London".into());
        let mut open = action("e2", 1, "click", "textbox", "Open Where to?");
        open.value = Some("London".into());
        let mut a = action("e3", 2, "select", "combobox", "Class → Business");
        a.current_value = Some("Economy".into());
        a.value = Some("b".into());
        let mut b = action("e4", 2, "select", "combobox", "Class → First");
        b.current_value = Some("Economy".into());
        b.value = Some("f".into());
        let mut check = action("e5", 3, "click", "checkbox", "Direct only");
        check.checked = Some("false".into());
        let mut menu = action("e6", 4, "click", "menuitemradio", "A very long role");
        menu.expanded = Some("true".into());
        let o = obs(vec![fill, open, a, b, check, menu]);
        let extras = Extras {
            sign_in: true,
            autofill_card: false,
            autofill_identity: true,
        };
        assert_eq!(
            table(&o, 80, extras),
            "[1] textbox    Where to? · London (type/click)\n\
             [2] combobox   Class · Economy · options: Business | First\n\
             [3] checkbox   Direct only · checked=false\n\
             [4] menuitemra A very long role · expanded=true\n\
             [SIGN_IN] Sign in with the saved account for this site (fills and submits the password for you)\n\
             [AUTOFILL_IDENTITY] Fill your saved name, address, email and phone for you"
        );
        assert_eq!(read(&o), "4 controls · Title 1");
        assert_eq!(
            table(&obs(vec![]), 80, Extras::default()),
            "(no interactive elements in view)"
        );
    }

    #[test]
    fn hosts() {
        assert_eq!(
            host("https://user@www.example.test:8443/a?b"),
            Some("www.example.test")
        );
        assert_eq!(host("about:blank"), None);
    }
}
