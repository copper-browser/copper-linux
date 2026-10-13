//! Agents: the MCP server in the window (Mac: `Fork/MCP/MCP.swift`,
//! `Backstage.swift`, `HandsUI.swift`, Settings › Agents).
//!
//! * [`host`] — `copper_mcp::Host` for this shell: tabs, pages, groups,
//!   crashes, vault / canvas / cloud through the other lanes' contracts.
//! * [`backstage`] — the policy over `WebKitBackstage`: a tab an agent works
//!   in while nobody looks at it is held in the window's ghost host, handed
//!   to the stage when the user selects it, and let go five minutes after
//!   the agent's last call.
//! * [`mark`] — who is driving which tab: a small dot on the tab row, a
//!   capsule over the page with Stop.
//! * [`settings`] — Settings › Agents.
//! * [`cli`] — `copper --mcp-stdio` and `copper <verb>` (before GTK).
//!
//! The server runs while `agent.json` says `enabled`; the listener is
//! `copper_mcp::transport` (its own two-thread runtime), the tools run on
//! the GTK main loop (`Mcp::serve`).

pub mod backstage;
pub mod cli;
pub mod crashes;
pub mod groups;
pub mod heat;
pub mod host;
pub mod mark;
pub mod qa;
pub mod settings;

use crate::actions::{self, Command, SettingsPage};
use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_core::agent::AgentFile;
use copper_mcp::Mcp;
use copper_mcp::transport::{self, TransportConfig};
use gio::prelude::ApplicationExt as _;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

pub use mark::paint_badge;

thread_local! {
    static AGENTS: RefCell<Option<Rc<Agents>>> = const { RefCell::new(None) };
}

/// The agents' state for this window (main thread only).
pub fn agents() -> Option<Rc<Agents>> {
    AGENTS.with(|a| a.borrow().clone())
}

/// Called once from `actions::register_lanes`.
pub fn register(shell: &Shell) {
    let state = Agents::new(shell);
    AGENTS.with(|a| *a.borrow_mut() = Some(state.clone()));
    if state.enabled() {
        state.start();
    }
    actions::register_settings_page(
        shell,
        SettingsPage {
            id: "agents".into(),
            title: "Agents".into(),
            icon: "system-run-symbolic".into(),
            build: Rc::new(settings::page),
        },
    );
    actions::register_command(
        shell,
        Command::new(
            "agents-stop",
            "Stop Agents",
            "media-playback-stop-symbolic",
            |_| {
                if let Some(a) = agents() {
                    a.marks.stop_all();
                }
            },
        )
        .when(|_| agents().is_some_and(|a| a.marks.any_active())),
    );
    actions::register_command(
        shell,
        Command::new(
            "agents-toggle",
            "Let Agents Drive This Window",
            "system-run-symbolic",
            |s| {
                if let Some(a) = agents() {
                    let on = !a.enabled();
                    a.set_enabled(on);
                    s.toast(if on {
                        "Agents can drive this window"
                    } else {
                        "Agent server off"
                    });
                }
            },
        ),
    );
    // Quitting closes the port at once rather than at process exit.
    shell.app().connect_shutdown(|_| {
        if let Some(a) = AGENTS.with(|a| a.borrow_mut().take()) {
            a.server.borrow_mut().take();
        }
    });
    qa::start(shell);
}

/// A settings page following the state; false once its widgets are gone.
type Watcher = Box<dyn Fn(&Agents) -> bool>;

/// The server, its settings and what the window shows about it.
pub struct Agents {
    shell: WeakShell,
    file: RefCell<AgentFile>,
    /// What the listener checks; rotated in place.
    token: Arc<RwLock<String>>,
    server: RefCell<Option<transport::Server>>,
    /// Why the server is not listening, when it should be.
    trouble: RefCell<Option<String>>,
    mcp: Rc<Mcp<host::AppHost>>,
    pub(crate) marks: Rc<mark::Marks>,
    pub(crate) stage: Rc<backstage::Stage>,
    /// Settings pages showing this state; each says whether it is still alive.
    watchers: RefCell<Vec<Watcher>>,
}

impl Agents {
    fn new(shell: &Shell) -> Rc<Agents> {
        let mut file = match shell.store().load::<AgentFile>() {
            Ok(l) => l.value,
            Err(e) => {
                tracing::warn!(error = %e, "agent.json unreadable; starting from defaults");
                AgentFile::default()
            }
        };
        let minted = file.ensure_token();
        let token = Arc::new(RwLock::new(file.token().unwrap_or_default().to_string()));
        let marks = mark::Marks::new(shell);
        let stage = backstage::Stage::new(shell);
        let host = host::AppHost::new(shell, stage.clone(), marks.clone());
        let mcp = Mcp::new(host, mcp_settings(&file));
        let agents = Rc::new(Agents {
            shell: shell.downgrade(),
            file: RefCell::new(file),
            token,
            server: RefCell::default(),
            trouble: RefCell::default(),
            mcp,
            marks,
            stage,
            watchers: RefCell::default(),
        });
        if minted {
            agents.save();
        }
        agents
    }

    pub fn enabled(&self) -> bool {
        self.file.borrow().enabled()
    }

    pub fn announces(&self) -> bool {
        self.file.borrow().announces()
    }

    pub fn jev(&self) -> bool {
        self.file.borrow().jev()
    }

    /// The port agent.json asks for.
    pub fn configured_port(&self) -> u16 {
        self.file.borrow().port()
    }

    /// The port this run listens on (`SEARCH_MCP_PORT` wins).
    pub fn port(&self) -> u16 {
        copper_mcp::config::port_override().unwrap_or_else(|| self.configured_port())
    }

    pub fn endpoint(&self) -> String {
        AgentFile::endpoint(self.port())
    }

    pub fn running(&self) -> bool {
        self.server.borrow().is_some()
    }

    pub fn trouble(&self) -> Option<String> {
        self.trouble.borrow().clone()
    }

    /// Never log this.
    pub fn token(&self) -> String {
        self.token.read().map(|t| t.clone()).unwrap_or_default()
    }

    /// Tool calls this session and the last tool's name.
    pub fn stats(&self) -> (u64, String) {
        let (calls, last, _) = self.mcp.stats();
        (calls, last)
    }

    pub fn mcp(&self) -> &Rc<Mcp<host::AppHost>> {
        &self.mcp
    }

    /// Settings › Agents › Let agents drive this window.
    pub fn set_enabled(&self, on: bool) {
        if self.enabled() == on && self.running() == on {
            return;
        }
        self.file.borrow_mut().enabled = Some(on);
        self.save();
        if on {
            self.start();
        } else {
            self.stop();
        }
    }

    pub fn set_announces(&self, on: bool) {
        self.file.borrow_mut().announces = Some(on);
        self.save();
        self.mcp.set_settings(mcp_settings(&self.file.borrow()));
        self.changed();
    }

    pub fn set_jev(&self, on: bool) {
        self.file.borrow_mut().jev = Some(on);
        self.save();
        self.mcp.set_settings(mcp_settings(&self.file.borrow()));
        self.changed();
    }

    /// A new port: listen there now if the server is on.
    pub fn set_port(&self, port: u16) {
        if port == 0 || port == self.configured_port() {
            return;
        }
        self.file.borrow_mut().port = Some(port);
        self.save();
        if self.enabled() {
            self.start();
        } else {
            self.changed();
        }
    }

    /// A new bearer token; every client with the old one stops working.
    pub fn rotate_token(&self) {
        let fresh = {
            let mut f = self.file.borrow_mut();
            f.rotate_token();
            f.token().unwrap_or_default().to_string()
        };
        if let Ok(mut t) = self.token.write() {
            *t = fresh;
        }
        self.save();
        self.changed();
    }

    /// Listen (again) on the configured port.
    pub fn start(&self) {
        self.server.borrow_mut().take();
        let port = self.port();
        let (tx, rx) = copper_mcp::server::channel();
        let config = TransportConfig {
            port,
            token: self.token.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            headless: headless(),
        };
        match transport::start(config, tx) {
            Ok(server) => {
                self.mcp.set_runtime(server.runtime());
                glib::spawn_future_local(self.mcp.clone().serve(rx));
                tracing::info!(port, "agent server listening");
                *self.server.borrow_mut() = Some(server);
                *self.trouble.borrow_mut() = None;
            }
            Err(e) => {
                let why = if e.kind() == std::io::ErrorKind::AddrInUse {
                    "another program is using it".to_string()
                } else {
                    e.to_string()
                };
                let text = format!("Couldn't listen on port {port}: {why}");
                tracing::warn!("{text}");
                *self.trouble.borrow_mut() = Some(text);
            }
        }
        self.changed();
    }

    /// Stop listening and let go of every tab agents held.
    pub fn stop(&self) {
        self.server.borrow_mut().take();
        *self.trouble.borrow_mut() = None;
        if let Some(shell) = self.shell.upgrade() {
            self.stage.release_all(&shell);
        }
        self.marks.clear();
        self.changed();
    }

    /// The tab an agent session keeps as its own.
    pub fn own_tab(&self, key: &str) -> Option<TabId> {
        self.mcp.own_tab(key)
    }

    /// Stop one agent session (Stop on its capsule).
    pub fn stop_hand(&self, key: &str) {
        self.mcp.stop(key);
    }

    /// What to paste into Claude Code's or phi's MCP settings.
    pub fn http_config(&self) -> String {
        http_config(&self.endpoint(), &self.token())
    }

    /// The same for a client that only speaks stdio.
    pub fn stdio_config(&self) -> String {
        let command = std::env::current_exe()
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "copper".into());
        stdio_config(&command)
    }

    /// Follow this state until `f` says its widgets are gone.
    pub fn watch(&self, f: impl Fn(&Agents) -> bool + 'static) {
        self.watchers.borrow_mut().push(Box::new(f));
    }

    fn changed(&self) {
        let watchers = std::mem::take(&mut *self.watchers.borrow_mut());
        let alive: Vec<_> = watchers.into_iter().filter(|w| w(self)).collect();
        let mut current = self.watchers.borrow_mut();
        let added = std::mem::take(&mut *current);
        *current = alive;
        current.extend(added);
    }

    fn save(&self) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        if let Err(e) = shell.store().save(&*self.file.borrow()) {
            tracing::warn!(error = %e, "could not write agent.json");
        }
    }
}

/// The HTTP client config (Mac `MCP.clientConfig`).
pub fn http_config(endpoint: &str, token: &str) -> String {
    let value = serde_json::json!({
        "mcpServers": {
            "copper": {
                "type": "http",
                "url": endpoint,
                "headers": { "Authorization": format!("Bearer {token}") },
            }
        }
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// The stdio client config (Mac `MCP.stdioConfig`): this binary with
/// `--mcp-stdio`.
pub fn stdio_config(command: &str) -> String {
    let value = serde_json::json!({
        "mcpServers": {
            "copper": {
                "type": "stdio",
                "command": command,
                "args": ["--mcp-stdio"],
            }
        }
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

fn mcp_settings(file: &AgentFile) -> copper_mcp::Settings {
    copper_mcp::Settings {
        jev: file.jev(),
        announces: file.announces(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// `SEARCH_HEADLESS=1` (the Mac's switch) or `COPPER_HEADLESS=1`: reported
/// by `/health`.
fn headless() -> bool {
    ["SEARCH_HEADLESS", "COPPER_HEADLESS"].iter().any(|k| {
        std::env::var(k)
            .map(|v| {
                matches!(
                    v.trim().to_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_configs_have_the_mac_shape() {
        let http: serde_json::Value =
            serde_json::from_str(&http_config("http://127.0.0.1:4123/mcp", "abc")).unwrap();
        assert_eq!(
            http,
            serde_json::json!({"mcpServers": {"copper": {
                "type": "http",
                "url": "http://127.0.0.1:4123/mcp",
                "headers": {"Authorization": "Bearer abc"}
            }}})
        );
        assert!(!http_config("http://127.0.0.1:4123/mcp", "abc").contains("\\/"));
        let stdio: serde_json::Value =
            serde_json::from_str(&stdio_config("/usr/bin/copper")).unwrap();
        assert_eq!(
            stdio,
            serde_json::json!({"mcpServers": {"copper": {
                "type": "stdio", "command": "/usr/bin/copper", "args": ["--mcp-stdio"]
            }}})
        );
    }
}
