//! What the agent in the window needs from the app around it: a place to
//! run on, the model keys, `chat.json`, the page in front, and Copper's own
//! tools run in-process. [`super::shell::ShellChat`] is the browser's; the
//! unit tests bring their own.

use copper_core::agent::ChatConfig;
use copper_engine::LocalFuture;
use copper_mcp::IntelligenceSource;
use copper_mcp::intelligence::Tier;
use serde_json::{Map, Value};

/// The tab in front of a question (Settings › Agents › Page in front of
/// every question).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageInFront {
    /// Its address (`about:blank` when it has none).
    pub url: String,
    pub title: String,
    /// A canvas tab: `(id, name)`. It is read with `canvas_read`, not as text.
    pub canvas: Option<(String, String)>,
    /// The page's visible text, when it could be read.
    pub text: Option<String>,
}

/// What one tool call came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolOutput {
    /// The text parts, joined by newlines; a picture reads `[screenshot attached]`.
    pub text: String,
    /// Pictures the call returned, `(bytes, mime)`: they go to the model as
    /// images in a user message, the one shape a tool result can't carry.
    pub pictures: Vec<(Vec<u8>, String)>,
    /// The call failed.
    pub is_error: bool,
    /// The tool's own one line for the step's row ("220 operations applied").
    pub line: Option<String>,
}

/// The app side of the agent in the window. Everything is called on the UI
/// thread; the futures are polled there too.
pub trait ChatHost: 'static {
    /// Run `task` on the UI thread, in a later turn of its loop.
    fn spawn(&self, task: LocalFuture<()>);
    /// The tokio runtime the router requests run on.
    fn runtime(&self) -> tokio::runtime::Handle;
    /// `intelligence.json` and Copper Cloud's keys, as they are now.
    fn intelligence(&self) -> IntelligenceSource;
    /// Choose the model, where the Mac keeps it (`intelligence.json` `tier`).
    fn set_tier(&self, tier: Tier);
    /// `chat.json` as it is now: read again for every question, so a switch
    /// flipped elsewhere (the sidebar, Settings) counts at once.
    fn config(&self) -> ChatConfig;
    /// Jev mode (Settings › Agents) is on: `jev_*` are offered when a Jev
    /// key is there too.
    fn jev_mode(&self) -> bool;
    /// The tools Copper's MCP server offers (`tools/list` entries).
    fn catalogue(&self, jev: bool) -> Vec<Value>;
    /// The tab in front, read for the question (`None`: none, or blank).
    fn page(&self) -> LocalFuture<Option<PageInFront>>;
    /// Run one of Copper's tools as the agent in the window.
    fn call(&self, name: &str, args: &Map<String, Value>) -> LocalFuture<ToolOutput>;
    /// Stop the Jev run this agent started, before its next move.
    fn stop_jev(&self);
}
