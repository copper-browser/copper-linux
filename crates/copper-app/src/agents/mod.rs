//! Agents in the GTK window (Mac: `Backstage.swift`, `HandsUI.swift`,
//! Settings › Agents). The MCP server, its tools' view of the shell and the
//! command-line client are copper-shell-core's
//! [`copper_shell_core::agents`] (re-exported here); this is what GTK adds:
//!
//! * [`backstage`] — the policy over `WebKitBackstage`: a tab an agent works
//!   in while nobody looks at it is held in the window's ghost host, handed
//!   to the stage when the user selects it, and let go five minutes after
//!   the agent's last call.
//! * [`mark`] — who is driving which tab: a small dot on the tab row, a
//!   capsule over the page with Stop.
//! * [`settings`] — Settings › Agents.

pub mod backstage;
pub mod mark;
pub mod qa;
pub mod settings;

pub use copper_shell_core::agents::{AgentsApp, cli, crashes, groups, heat, host};
pub use mark::paint_badge;

use crate::actions::{self, Command, SettingsPage};
use crate::platform::Gtk;
use crate::shell::{GtkShell, Shell};
use copper_core::TabId;
use copper_engine::LocalFuture;
use copper_mcp::drive::DriveEvent;
use gio::prelude::ApplicationExt as _;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

/// The server's state ([`copper_shell_core::agents::Agents`]) on GTK.
pub type Server = copper_shell_core::agents::Agents<Gtk>;

thread_local! {
    static AGENTS: RefCell<Option<Rc<Agents>>> = const { RefCell::new(None) };
}

/// The agents' state for this window (main thread only).
pub fn agents() -> Option<Rc<Agents>> {
    AGENTS.with(|a| a.borrow().clone())
}

/// Called once from `actions::register_lanes`.
pub fn register(shell: &Shell) {
    let marks = mark::Marks::new(shell);
    let stage = backstage::Stage::new(shell);
    let hands = Rc::new(Hands {
        marks: marks.clone(),
        stage: stage.clone(),
    });
    let server = copper_shell_core::agents::start(shell, hands);
    AGENTS.with(|a| {
        *a.borrow_mut() = Some(Rc::new(Agents {
            server,
            marks,
            stage,
        }))
    });
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
        AGENTS.with(|a| a.borrow_mut().take());
        copper_shell_core::agents::shutdown::<Gtk>();
    });
    qa::start(shell);
}

/// The server and what the window shows of it: the marks and the
/// backstage. Derefs to the [`Server`].
pub struct Agents {
    server: Rc<Server>,
    pub(crate) marks: Rc<mark::Marks>,
    pub(crate) stage: Rc<backstage::Stage>,
}

impl Deref for Agents {
    type Target = Server;

    fn deref(&self) -> &Server {
        &self.server
    }
}

/// GTK's side of [`AgentsApp`]: the backstage, the marks, and the
/// passwords lane's tools.
struct Hands {
    marks: Rc<mark::Marks>,
    stage: Rc<backstage::Stage>,
}

impl AgentsApp<Gtk> for Hands {
    fn holds(&self, tab: &TabId) -> bool {
        self.stage.holds(tab)
    }

    fn prepare(&self, shell: &Shell, tab: &TabId) -> bool {
        self.stage.prepare(shell, tab)
    }

    fn release(&self, shell: &Shell, tab: &TabId) {
        self.stage.release(shell, tab);
    }

    fn release_all(&self, shell: &Shell) {
        self.stage.release_all(shell);
    }

    fn announce(&self, text: &str) {
        self.marks.announce(text);
    }

    fn drive(&self, event: &DriveEvent) {
        self.marks.event(event);
    }

    fn clear_marks(&self) {
        self.marks.clear();
    }

    fn sign_in(
        &self,
        shell: Shell,
        tab: TabId,
        args: Map<String, Value>,
    ) -> LocalFuture<Result<Value, String>> {
        Box::pin(crate::forms::mcp::sign_in::<crate::forms::GtkForms>(
            shell, tab, args,
        ))
    }

    fn autofill(
        &self,
        shell: Shell,
        tab: TabId,
        args: Map<String, Value>,
    ) -> LocalFuture<Result<Value, String>> {
        Box::pin(crate::forms::mcp::autofill(shell, tab, args))
    }
}
