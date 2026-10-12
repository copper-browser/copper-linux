//! MCP page tools (`browser_*`): the calls into `window.__copper` (Fork/MCP/Page.swift,
//! Tools.swift). Every call installs the helper first if this document lacks it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::call::Call;
use crate::script::Script;

/// What a page tool acts on: `ref:e12` (from a snapshot) or `css:…`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Locator {
    Ref(String),
    Css(String),
}

impl Locator {
    /// `Tools.locator`: `ref` wins over `selector`; empty strings don't count.
    pub fn from_args(reference: Option<&str>, selector: Option<&str>) -> Option<Locator> {
        match (reference, selector) {
            (Some(r), _) if !r.is_empty() => Some(Locator::Ref(r.to_string())),
            (_, Some(s)) if !s.is_empty() => Some(Locator::Css(s.to_string())),
            _ => None,
        }
    }

    /// The string the page script expects (`Tools.Locator.script`).
    pub fn script(&self) -> String {
        match self {
            Locator::Ref(r) => format!("ref:{r}"),
            Locator::Css(c) => format!("css:{c}"),
        }
    }
}

/// An element's box in viewport CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    /// The click point: the middle of the box.
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// CSS pixels to view points: page zoom is the only scale between them.
    pub fn scaled(&self, zoom: f64) -> Rect {
        Rect {
            x: self.x * zoom,
            y: self.y * zoom,
            width: self.width * zoom,
            height: self.height * zoom,
        }
    }
}

/// `window.__copper.rect`: the box, or why there is none.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Located {
    Found(Rect),
    Missing { error: String },
}

impl Located {
    /// The Mac's error text when there is no box (`Page.prepare`).
    pub fn into_result(self, target: &Locator) -> Result<Rect, String> {
        match self {
            Located::Found(rect) => Ok(rect),
            Located::Missing { error } => Err(format!(
                "{}: {error}. Take a fresh browser_snapshot — refs change when the page does.",
                target.script()
            )),
        }
    }
}

/// One captured console line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub level: String,
    pub text: String,
}

/// Parse the JSON string [`console`] returns.
pub fn parse_console(json: &str) -> Result<Vec<ConsoleEntry>, serde_json::Error> {
    serde_json::from_str(json)
}

/// `browser_snapshot`. `limit` defaults to 600 on the Mac.
pub fn snapshot(selector: Option<&str>, interactive: bool, limit: u32) -> Call<String> {
    Call::new(
        Script::Snapshot,
        json!({ "selector": selector.unwrap_or(""), "interactive": interactive, "limit": limit }),
    )
}

/// Scroll into view and measure (`Page.prepare`).
pub fn rect(target: &Locator) -> Call<Located> {
    Call::new(Script::Rect, json!({ "target": target.script() }))
}

/// Document height for a full-page screenshot (≤ 20000).
pub fn document_height() -> Call<f64> {
    Call::new(Script::DocumentHeight, json!({}))
}

/// DOM click. `button`: "left" | "right" | "middle".
pub fn click(target: &Locator, double: bool, button: &str) -> Call<String> {
    Call::new(
        Script::Click,
        json!({ "target": target.script(), "double": double, "button": button }),
    )
}

pub fn hover(target: &Locator) -> Call<String> {
    Call::new(Script::Hover, json!({ "target": target.script() }))
}

pub fn focus(target: &Locator) -> Call<String> {
    Call::new(Script::Focus, json!({ "target": target.script() }))
}

/// Native-setter typing: the value a framework notices, then input + change.
pub fn set_value(target: &Locator, text: &str) -> Call<String> {
    Call::new(
        Script::SetValue,
        json!({ "target": target.script(), "text": text }),
    )
}

pub fn submit(target: &Locator) -> Call<String> {
    Call::new(Script::Submit, json!({ "target": target.script() }))
}

/// One `browser_fill_form` field. `kind`: textbox | checkbox | radio | combobox | slider.
pub fn fill(reference: &str, kind: &str, value: &str) -> Call<String> {
    Call::new(
        Script::Fill,
        json!({ "target": Locator::Ref(reference.into()).script(), "kind": kind, "value": value }),
    )
}

pub fn key(key: &str) -> Call<String> {
    Call::new(Script::Key, json!({ "key": key }))
}

pub fn select(target: &Locator, values: &[String]) -> Call<String> {
    Call::new(
        Script::Select,
        json!({ "target": target.script(), "values": values }),
    )
}

pub fn drag(start_ref: &str, end_ref: &str) -> Call<String> {
    Call::new(
        Script::Drag,
        json!({
            "from": Locator::Ref(start_ref.into()).script(),
            "to": Locator::Ref(end_ref.into()).script(),
        }),
    )
}

/// `browser_wait_for`'s poll: whether the body's text includes `text`
/// (`None` before there is a body).
pub fn has_text(text: &str) -> Call<Option<bool>> {
    Call::new(Script::HasText, json!({ "text": text }))
}

/// `browser_scroll`: within an element (ref) or the window.
pub fn scroll(within: Option<&str>, dx: f64, dy: f64) -> Call<String> {
    let within = within
        .map(|r| Locator::Ref(r.into()).script())
        .unwrap_or_default();
    Call::new(
        Script::Scroll,
        json!({ "within": within, "dx": dx, "dy": dy }),
    )
}

/// `browser_get_text`; `None` reads the main content (or the body).
pub fn text(target: Option<&Locator>) -> Call<String> {
    Call::new(
        Script::Text,
        json!({ "target": target.map(Locator::script).unwrap_or_default() }),
    )
}

/// `browser_console_messages`: a JSON string (rendered as is); see [`parse_console`].
pub fn console() -> Call<String> {
    Call::new(Script::Console, json!({}))
}

/// `browser_find`.
pub fn find_text(needle: &str) -> Call<String> {
    Call::new(Script::FindText, json!({ "needle": needle }))
}

/// `browser_evaluate`: the agent's function, called with the element behind
/// `reference` (or with nothing). The function text is the agent's own code
/// and is necessarily spliced in, as on the Mac. Render the result with
/// [`crate::render::render`].
pub fn evaluate(function: &str, reference: Option<&str>) -> Call<Value> {
    match reference {
        Some(r) => Call::custom(
            Script::Element,
            json!({ "target": Locator::Ref(r.into()).script() }),
            function.to_string(),
        ),
        None => Call::custom(Script::Element, Value::Null, function.to_string()),
    }
}
