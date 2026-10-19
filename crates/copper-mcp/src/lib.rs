//! Copper MCP server, tools, Jev client and stdio bridge.
//!
//! The pieces, and where they run:
//!
//! * [`transport`] — the loopback HTTP listener (`127.0.0.1:4123/mcp`, MCP's
//!   Streamable HTTP in its simplest shape: one POST, one JSON reply). It
//!   runs on its own thread with a tokio runtime, checks Origin and the
//!   bearer token, and hands every JSON-RPC message to the host thread as a
//!   [`server::Job`].
//! * [`Mcp`] — the JSON-RPC server and every tool. It lives on the host's
//!   UI thread (GTK's main loop): [`Mcp::serve`] is a `!Send` future the app
//!   spawns there; tools reach the browser only through [`host::Host`].
//! * [`bridge`] — `copper --mcp-stdio`: stdin/stdout JSON-RPC proxied to the
//!   running instance. [`cli`] — the `copper` command-line verbs;
//!   [`cloud_cli`] — `copper cloud pair|sso|status`, with no window.
//!
//! Formats follow macOS Copper (`Fork/MCP/*.swift`) byte for byte where an
//! agent can see them: tool names, descriptions and schemas come straight
//! from the captured Mac catalogue; result texts follow `Tools.swift`.

pub mod agent_tabs;
pub mod bridge;
pub mod catalogue;
pub mod cli;
pub mod client;
pub mod cloud_cli;
pub mod config;
pub mod drive;
pub mod host;
pub mod input;
pub mod intelligence;
pub mod server;
pub mod shot;
pub mod tools;
pub mod transport;
pub mod ultrafast;
pub mod who;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

pub use host::{Content, EnginePage, GroupInfo, Host, IntelligenceSource, Page, TabId, TabInfo};
pub use who::Who;

/// A tool's failure: the text the agent reads (`isError: true`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure(pub String);

impl Failure {
    pub fn new(text: impl Into<String>) -> Self {
        Failure(text.into())
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Failure {}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure(s)
    }
}

impl From<&str> for Failure {
    fn from(s: &str) -> Self {
        Failure(s.to_string())
    }
}

/// The server's switches (`MCP.Config` minus what the transport owns).
#[derive(Clone, Debug)]
pub struct Settings {
    /// Jev mode: offer and run the `jev_*` tools.
    pub jev: bool,
    /// Say each tool call in the status line.
    pub announces: bool,
    /// Copper's version, for `serverInfo` and the Jev user agent.
    pub version: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            jev: false,
            announces: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// One tool call's context: who is calling, the tab it named, and the
/// one-line summary it may leave for `_meta.summary`.
#[derive(Clone, Debug, Default)]
pub struct CallCtx {
    pub who: Who,
    /// The tab the call named with `tab` (not for `browser_tabs`).
    pub pinned: Option<TabId>,
    /// The call opens a tab behind the user's (`AgentTabs.behind`).
    pub behind: bool,
    /// `_meta.summary` (≤ 200 characters).
    pub summary: Option<String>,
    /// A loopback client (not an agent link).
    pub loopback: bool,
}

impl CallCtx {
    pub fn new(who: Who) -> Self {
        CallCtx {
            who,
            loopback: true,
            ..CallCtx::default()
        }
    }

    pub(crate) fn set_summary(&mut self, line: String) {
        self.summary = Some(if line.chars().count() > 200 {
            format!("{}…", line.chars().take(199).collect::<String>())
        } else {
            line
        });
    }
}

#[derive(Default)]
pub(crate) struct State {
    pub drive: drive::Drive,
    /// Each agent session's own tab, by `Who::key`.
    pub bound: HashMap<String, TabId>,
    /// Backstage holds: tab → the last time an agent asked for it.
    pub held: HashMap<TabId, Instant>,
    /// What each MCP session said at `initialize`.
    pub said: HashMap<String, who::Said>,
    /// The last client's name, as a person would say it.
    pub client_name: String,
    /// One Jev session per tab.
    pub sessions: HashMap<TabId, ultrafast::Session>,
    pub calls: u64,
    pub last_tool: String,
    pub jev_note: String,
}

/// The MCP server and its tools, bound to one [`Host`]. Lives on the host's
/// thread; share it as `Rc<Mcp<H>>`.
pub struct Mcp<H: Host> {
    pub host: H,
    pub(crate) settings: RefCell<Settings>,
    pub(crate) state: RefCell<State>,
    pub(crate) net: RefCell<Option<intelligence::Net>>,
    pub(crate) running: Cell<bool>,
}

impl<H: Host> Mcp<H> {
    pub fn new(host: H, settings: Settings) -> Rc<Self> {
        Rc::new(Mcp {
            host,
            settings: RefCell::new(settings),
            state: RefCell::new(State::default()),
            net: RefCell::new(None),
            running: Cell::new(false),
        })
    }

    /// Where Jev and router requests run (the transport's runtime, or any
    /// other tokio runtime). Without one, the `jev_*` tools say so.
    pub fn set_runtime(&self, runtime: tokio::runtime::Handle) {
        let version = self.settings.borrow().version.clone();
        *self.net.borrow_mut() = Some(intelligence::Net::new(runtime, &version));
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        *self.settings.borrow_mut() = settings;
    }

    /// The user pressed Stop on the hand `key`: a live Jev run stops before
    /// its next action; any other agent's calls are refused for 30 s.
    pub fn stop(&self, key: &str) {
        self.state.borrow_mut().drive.stop(key);
    }

    /// Let every stopped agent back in.
    pub fn resume(&self) {
        self.state.borrow_mut().drive.resume();
    }

    /// The tab an agent session holds, if any.
    pub fn own_tab(&self, key: &str) -> Option<TabId> {
        self.state.borrow().bound.get(key).cloned()
    }

    /// Calls served, and the last tool's name (Settings › Agents).
    pub fn stats(&self) -> (u64, String, String) {
        let s = self.state.borrow();
        (s.calls, s.last_tool.clone(), s.jev_note.clone())
    }
}
