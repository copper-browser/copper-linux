//! Agents: the MCP server in the window (Mac: `Fork/MCP/MCP.swift`,
//! `Backstage.swift`, `HandsUI.swift`, Settings › Agents).
//!
//! * [`host`] — `copper_mcp::Host` for this shell: tabs, pages, groups,
//!   crashes, vault / canvas / cloud through the other lanes' contracts.
//! * [`groups`], [`crashes`], [`heat`] — what `browser_groups`,
//!   `copper_crashes` and the perf probe read.
//! * [`cli`] — `copper --mcp-stdio` and `copper <verb>` (before any window).
//!
//! The app supplies the rest through [`AgentsApp`]: the backstage (a tab an
//! agent works in while nobody looks at it is held where the engine still
//! renders it), the marks (who is driving which tab) and the password
//! tools. GTK's are `copper-app`'s `agents::{backstage, mark}` and Settings
//! › Agents.
//!
//! The server runs while `agent.json` says `enabled`; the listener is
//! `copper_mcp::transport` (its own two-thread runtime), the tools run on
//! the UI main loop (`Mcp::serve`).

pub mod cli;
pub mod crashes;
pub mod groups;
pub mod heat;
pub mod host;

use crate::Platform;
use crate::main_loop::MainLoop;
use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_core::agent::AgentFile;
use copper_engine::{Engine as _, LocalFuture};
use copper_mcp::Mcp;
use copper_mcp::drive::DriveEvent;
use copper_mcp::transport::{self, TransportConfig};
use serde_json::{Map, Value};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, RwLock};

thread_local! {
    /// The running `Rc<Agents<P>>` ([`start`]; one platform per process).
    static AGENTS: RefCell<Option<Rc<dyn Any>>> = const { RefCell::new(None) };
}

/// The agents' state for this window (main thread only), once [`start`]ed
/// and until [`shutdown`].
pub fn get<P: Platform>() -> Option<Rc<Agents<P>>> {
    AGENTS.with(|a| a.borrow().clone()?.downcast::<Agents<P>>().ok())
}

/// What the agents need from the app beyond the shell (GTK: `copper-app`'s
/// `agents::{backstage, mark}` and its passwords lane).
pub trait AgentsApp<P: Platform>: 'static {
    // -- the backstage ---------------------------------------------------------
    /// Whether `tab`'s page is held backstage: rendered by the engine but not
    /// on the user's screen (Mac: the Backstage window).
    fn holds(&self, tab: &TabId) -> bool;
    /// Make `tab`'s page drivable for a tool call: shown, or held backstage
    /// when the user is looking at another tab. `true` when its visibility
    /// changed (the engine needs a moment to paint).
    fn prepare(&self, shell: &Shell<P>, tab: &TabId) -> bool;
    /// Let `tab`'s page go from the backstage.
    fn release(&self, shell: &Shell<P>, tab: &TabId);
    /// Let every held page go (the server stopped).
    fn release_all(&self, shell: &Shell<P>);

    // -- the marks ---------------------------------------------------------------
    /// Say what the agent is doing (Settings › Agents › Say what the agent
    /// does).
    fn announce(&self, text: &str);
    /// An agent session started, worked in or left a tab.
    fn drive(&self, event: &DriveEvent);
    /// Forget every mark (the server stopped).
    fn clear_marks(&self);

    // -- passwords ---------------------------------------------------------------
    /// `browser_sign_in` on `tab` (the app's credential stores).
    fn sign_in(
        &self,
        shell: Shell<P>,
        tab: TabId,
        args: Map<String, Value>,
    ) -> LocalFuture<Result<Value, String>>;
    /// `browser_autofill` on `tab`.
    fn autofill(
        &self,
        shell: Shell<P>,
        tab: TabId,
        args: Map<String, Value>,
    ) -> LocalFuture<Result<Value, String>>;
}

/// Load `agent.json`, and listen if it says so. Once, at startup (from the
/// app's lane registration).
pub fn start<P: Platform>(shell: &Shell<P>, app: Rc<dyn AgentsApp<P>>) -> Rc<Agents<P>> {
    let state = Agents::new(shell, app);
    AGENTS.with(|a| *a.borrow_mut() = Some(state.clone() as Rc<dyn Any>));
    if state.enabled() {
        state.start();
    }
    state
}

/// Quitting: close the port at once rather than at process exit.
pub fn shutdown<P: Platform>() {
    if let Some(a) = AGENTS.with(|a| a.borrow_mut().take())
        && let Ok(a) = a.downcast::<Agents<P>>()
    {
        a.server.borrow_mut().take();
    }
}

/// A settings page following the state; false once its widgets are gone.
type Watcher<P> = Box<dyn Fn(&Agents<P>) -> bool>;

/// The server, its settings and what the window shows about it.
pub struct Agents<P: Platform> {
    shell: WeakShell<P>,
    file: RefCell<AgentFile>,
    /// What the listener checks; rotated in place.
    token: Arc<RwLock<String>>,
    server: RefCell<Option<transport::Server>>,
    /// Why the server is not listening, when it should be.
    trouble: RefCell<Option<String>>,
    mcp: Rc<Mcp<host::AppHost<P>>>,
    app: Rc<dyn AgentsApp<P>>,
    /// Settings pages showing this state; each says whether it is still alive.
    watchers: RefCell<Vec<Watcher<P>>>,
    /// The console capture every page gets from document start while the
    /// server is on (`copper_pagejs::page::console_early`).
    console: Cell<Option<copper_engine::ScriptId>>,
}

impl<P: Platform> Agents<P> {
    fn new(shell: &Shell<P>, app: Rc<dyn AgentsApp<P>>) -> Rc<Agents<P>> {
        let mut file = match shell.store().load::<AgentFile>() {
            Ok(l) => l.value,
            Err(e) => {
                tracing::warn!(error = %e, "agent.json unreadable; starting from defaults");
                AgentFile::default()
            }
        };
        let minted = file.ensure_token();
        let token = Arc::new(RwLock::new(file.token().unwrap_or_default().to_string()));
        let host = host::AppHost::new(shell, app.clone());
        let mcp = Mcp::new(host, mcp_settings(&file));
        let agents = Rc::new(Agents {
            shell: shell.downgrade(),
            file: RefCell::new(file),
            token,
            server: RefCell::default(),
            trouble: RefCell::default(),
            mcp,
            app,
            watchers: RefCell::default(),
            console: Cell::new(None),
        });
        if minted {
            agents.save();
        }
        agents
    }

    /// What the app supplies ([`AgentsApp`]).
    pub fn app(&self) -> &Rc<dyn AgentsApp<P>> {
        &self.app
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

    pub fn mcp(&self) -> &Rc<Mcp<host::AppHost<P>>> {
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
        self.capture_console(true);
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
                P::Loop::spawn_local(self.mcp.clone().serve(rx));
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
        self.capture_console(false);
        self.server.borrow_mut().take();
        *self.trouble.borrow_mut() = None;
        if let Some(shell) = self.shell.upgrade() {
            self.app.release_all(&shell);
        }
        self.app.clear_marks();
        self.changed();
    }

    /// While agents may drive the window, every page keeps its console from
    /// the moment its document starts, so `browser_console_messages` also
    /// has what was logged while the page loaded (the Mac starts at the
    /// first page-tool call). Pages already open get it from their next load.
    fn capture_console(&self, on: bool) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        match (on, self.console.get()) {
            (true, None) => {
                let script = engine_script(&copper_pagejs::page::console_early());
                self.console
                    .set(Some(shell.engine().add_page_script(&script)));
            }
            (false, Some(id)) => {
                shell.engine().remove_page_script(id);
                self.console.set(None);
            }
            _ => {}
        }
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
    pub fn watch(&self, f: impl Fn(&Agents<P>) -> bool + 'static) {
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

/// A page script as the engine takes it.
fn engine_script(s: &copper_pagejs::UserScript) -> copper_engine::UserScript {
    use copper_engine::{Frames, InjectionTime, World};
    let mut u = copper_engine::UserScript::new(s.source.clone());
    u.injection_time = match s.injection {
        copper_pagejs::Injection::DocumentStart => InjectionTime::DocumentStart,
        copper_pagejs::Injection::DocumentEnd => InjectionTime::DocumentEnd,
    };
    u.frames = match s.frames {
        copper_pagejs::Frames::MainFrame => Frames::TopOnly,
        copper_pagejs::Frames::AllFrames => Frames::All,
    };
    u.world = match s.world {
        copper_pagejs::World::Page => World::Page,
        copper_pagejs::World::Isolated(n) => World::Isolated(n.to_string()),
    };
    u
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
    fn console_capture_runs_first_in_the_main_frame() {
        let script = engine_script(&copper_pagejs::page::console_early());
        assert_eq!(
            script.injection_time,
            copper_engine::InjectionTime::DocumentStart
        );
        assert_eq!(script.frames, copper_engine::Frames::TopOnly);
        assert_eq!(script.world, copper_engine::World::Page);
        assert!(script.source.contains("__copperLogs"));
    }

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
