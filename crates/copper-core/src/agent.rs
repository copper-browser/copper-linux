//! Agent-facing local files.
//!
//! * `agent.json` — [`AgentFile`]: the MCP server's port, bearer token and
//!   switches, plus agent links (Mac: `Fork/MCP/MCP.swift` `MCP.Config`,
//!   `Fork/MCP/Link.swift` `AgentLink.Config`). 0600. Never log `token`.
//! * `chat.json` — [`ChatConfig`]: the agent pane's settings
//!   (Mac: `Fork/Agent/Agent.swift` `Agent.Config`).
//! * `mcp.json` — [`McpServersFile`]: MCP servers the agent pane connects to,
//!   free-form like Claude Code's (Mac: `Fork/Agent/Servers.swift`).
//!
//! The Mac decoders are lenient (every field has a default), so fields are
//! optional here and accessor methods apply the Mac defaults; absent keys
//! stay absent on save.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The default MCP port.
pub const DEFAULT_MCP_PORT: u16 = 4123;

/// `agent.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentFile {
    /// The MCP server is on. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Default 4123.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Bearer token (64 hex chars). Empty or absent: mint one with
    /// [`AgentFile::ensure_token`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Say each tool call in the status line. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announces: Option<bool>,
    /// Jev tools on. Default false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<bool>,
    /// Agent links (bots that drive this browser remotely).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<AgentLink>>,
    /// The legacy single link, as an external installer may write it.
    #[serde(default, rename = "agentLink", skip_serializing_if = "Option::is_none")]
    pub agent_link: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AgentFile {
    /// Whether the server should run.
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(false)
    }
    /// The configured port (4123 by default). `SEARCH_MCP_PORT` overrides
    /// are the caller's business.
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_MCP_PORT)
    }
    /// The bearer token, if one is set.
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref().filter(|t| !t.is_empty())
    }
    /// Announce tool calls (default true).
    pub fn announces(&self) -> bool {
        self.announces.unwrap_or(true)
    }
    /// Jev tools (default false).
    pub fn jev(&self) -> bool {
        self.jev.unwrap_or(false)
    }
    /// Mint a token if there is none (as `MCP.init` does). True if minted —
    /// the caller should save the file (0600).
    pub fn ensure_token(&mut self) -> bool {
        if self.token().is_some() {
            return false;
        }
        self.token = Some(crate::ids::fresh_token());
        true
    }
    /// Replace the token.
    pub fn rotate_token(&mut self) {
        self.token = Some(crate::ids::fresh_token());
    }
    /// The MCP endpoint for a port.
    pub fn endpoint(port: u16) -> String {
        format!("http://127.0.0.1:{port}/mcp")
    }
}

/// One agent link (`AgentLink.Config`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLink {
    /// A UUID, or `legacy`.
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Never log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub announces: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// `chat.json` (`Agent.Config`).
///
/// A file without `version` is from before Settings › Agents: its `maxTurns`
/// of 24 means "the default", not a choice. [`ChatConfig::effective_max_turns`]
/// applies that rule.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatConfig {
    /// Empty means the router model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_context: Option<bool>,
    /// 2 in files written by current builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ChatConfig {
    /// Default tool-call rounds per question.
    pub const DEFAULT_MAX_TURNS: i64 = 60;
    /// What legacy files always held.
    pub const LEGACY_MAX_TURNS: i64 = 24;

    /// Rounds per question, clamped to 1…500, with the legacy rule applied.
    pub fn effective_max_turns(&self) -> i64 {
        match self.max_turns {
            Some(n) if !(self.version.is_none() && n == Self::LEGACY_MAX_TURNS) => n.clamp(1, 500),
            _ => Self::DEFAULT_MAX_TURNS,
        }
    }
    /// Put the page's text in front of each question (default true).
    pub fn page_context(&self) -> bool {
        self.page_context.unwrap_or(true)
    }
    /// Set every field as a current Mac build writes them (`version: 2`).
    pub fn normalize(&mut self) {
        let turns = self.effective_max_turns();
        self.model.get_or_insert_with(String::new);
        self.max_turns = Some(turns);
        self.page_context = Some(self.page_context());
        self.version = Some(2);
    }
}

/// `mcp.json`: `{"mcpServers": {name: {command,args,env | url,headers,type}}}`.
/// Read as free-form JSON (the Mac uses `JSONSerialization`); `servers` is
/// accepted as an alias of `mcpServers`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct McpServersFile {
    #[serde(
        default,
        rename = "mcpServers",
        skip_serializing_if = "Option::is_none"
    )]
    pub mcp_servers: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl McpServersFile {
    /// The server table, whichever key holds it.
    pub fn table(&self) -> Option<&Map<String, Value>> {
        self.mcp_servers.as_ref().or(self.servers.as_ref())
    }
    /// What is written when there is no file yet.
    pub const EXAMPLE: &'static str = "{\n  \"mcpServers\": {\n  }\n}\n";
}
