//! MCP page tools (`browser_*`): the calls into `window.__copper` (Fork/MCP/Page.swift,
//! Tools.swift). Every call installs the helper first if this document lacks it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::call::{Call, UserScript};
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
    Missing {
        error: String,
        /// There, but hidden, off the page or covered (asked with `hit`).
        #[serde(default)]
        blocked: bool,
        /// What covers it, when something does.
        #[serde(default)]
        cover: Option<String>,
    },
}

impl Located {
    /// Hidden, off the page or covered: worth another look a beat later
    /// (a fading toast, a closing sheet).
    pub fn blocked(&self) -> bool {
        matches!(self, Located::Missing { blocked: true, .. })
    }

    /// The Mac's error text when there is no box (`Page.prepare`): what is
    /// wrong, then what to do about it.
    pub fn into_result(self, target: &Locator) -> Result<Rect, String> {
        match self {
            Located::Found(rect) => Ok(rect),
            Located::Missing {
                error,
                blocked,
                cover,
            } => {
                let next = if cover.is_some() {
                    "Close or dismiss what covers it first (find its button with browser_snapshot), then try again."
                } else if blocked {
                    "Open or scroll to whatever reveals it first, or pick another ref from a fresh browser_snapshot."
                } else {
                    "Take a fresh browser_snapshot — refs change when the page does."
                };
                Err(format!("{}: {error}. {next}", target.script()))
            }
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

/// Scroll into view and measure (`Page.prepare`). With `hit` (anything
/// about to be clicked or typed into), the point at its middle must be the
/// element's own, or it is [`Located::Missing`] saying what is in the way.
pub fn rect(target: &Locator, hit: bool) -> Call<Located> {
    Call::new(
        Script::Rect,
        json!({ "target": target.script(), "hit": hit }),
    )
}

/// Document height for a full-page screenshot (≤ 20000).
pub fn document_height() -> Call<f64> {
    Call::new(Script::DocumentHeight, json!({}))
}

/// Where the page is scrolled to and how big the document is, in CSS
/// pixels (the document at least the viewport's size).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    /// `scrollX`, `scrollY`.
    pub x: f64,
    pub y: f64,
    /// The document's scrollable size.
    pub width: f64,
    pub height: f64,
}

/// [`Metrics`] for a screenshot of an element or a region.
pub fn metrics() -> Call<Metrics> {
    Call::new(Script::Metrics, json!({}))
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

/// One file for [`drop`]: its name, MIME type and bytes (base64).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DropFile {
    pub name: String,
    #[serde(rename = "type")]
    pub mime: String,
    pub data: String,
}

/// `browser_file_upload` with `ref`: `files` dropped on the element, as a
/// file manager drops them. Answers `{accepted}` or `{error}`.
pub fn drop(reference: &str, files: &[DropFile]) -> Call<Value> {
    Call::new(
        Script::Drop,
        json!({ "target": Locator::Ref(reference.into()).script(), "files": files }),
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

/// The console capture as a document-start user script (page world, main
/// frame): install it on every page agents may read, and
/// [`console`] includes what was logged before the first page-tool call.
/// The entries and their text are the helper's own.
pub fn console_early() -> UserScript {
    UserScript::new(Script::ConsoleEarly, json!({}))
}

/// After an input: how long (ms) until the last short timer the page started
/// in the last `ms` fires — a navigation it sets off is worth waiting for.
/// 0 on a page without [`console_early`].
pub fn due(ms: f64) -> Call<f64> {
    Call::new(Script::Due, json!({ "ms": ms }))
}

/// `browser_find`.
pub fn find_text(needle: &str) -> Call<String> {
    Call::new(Script::FindText, json!({ "needle": needle }))
}

/// `browser_evaluate`: the agent's function — called with the element behind
/// `reference`, or with nothing — or a plain expression. A Promise is
/// awaited; the answer is a string as it is, anything else as JSON text. The
/// function text is the agent's own code and is necessarily spliced in, as
/// on the Mac.
pub fn evaluate(function: &str, reference: Option<&str>) -> Call<Value> {
    let target = reference.map(|r| Locator::Ref(r.into()).script());
    Call::custom(
        Script::Element,
        json!({ "target": target.unwrap_or_default() }),
        function.to_string(),
    )
}
