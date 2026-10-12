//! Where the server listens and with which token: `agent.json`, as the
//! bridge and the CLI read it (enabled, port, token), with the
//! `SEARCH_MCP_PORT` override the Mac honours.

use std::path::Path;

use copper_core::agent::{AgentFile, DEFAULT_MCP_PORT};
use copper_core::{DataFile, Store};

/// The MCP path on the loopback server.
pub const PATH: &str = "/mcp";

/// What a client needs to reach the running instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub enabled: bool,
    pub port: u16,
    /// Never log this.
    pub token: String,
}

impl Endpoint {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{PATH}", self.port)
    }
}

/// `SEARCH_MCP_PORT`: listen (or connect) here instead of agent.json's
/// port, without changing the file.
pub fn port_override() -> Option<u16> {
    std::env::var("SEARCH_MCP_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p > 0)
}

/// `agent.json` at `path`, or `None` when it is missing or unreadable.
pub fn read_agent_file(path: &Path) -> Option<AgentFile> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The endpoint from a store's `agent.json`, with the port override.
pub fn endpoint(store: &Store) -> Option<Endpoint> {
    let file = read_agent_file(&store.path(DataFile::Agent))?;
    Some(Endpoint {
        enabled: file.enabled(),
        port: port_override().unwrap_or_else(|| file.port()),
        token: file.token().unwrap_or("").to_string(),
    })
}

/// The default port.
pub const DEFAULT_PORT: u16 = DEFAULT_MCP_PORT;
