//! Actions, shortcuts and the hooks later lanes plug into.
//!
//! Every user-facing operation is a `win.<name>` action with its shortcut
//! (handled in the capture phase, so a page or a field cannot swallow
//! `Ctrl+T`), and — when it makes sense to type its name — a palette
//! [`Command`] (`Ctrl+K`).
//!
//! # Adding a lane
//!
//! A later lane (shield, reader, panels, settings pages, MCP…) adds one
//! line to [`register_lanes`] calling its own `register(shell)`, and from
//! there uses:
//!
//! * [`register_action`] — a `win.<name>` action with shortcuts;
//! * [`register_command`] — a palette row;
//! * [`register_settings_page`] — a page in Settings;
//! * [`register_panel`] — a side panel the stage can show
//!   ([`crate::window::Stage::show_panel`]), with a palette command;
//! * [`register_menu_item`] — an entry in the window menu.
//!
//! Nothing here needs editing to add a lane except [`register_lanes`].

use crate::palette::Mode;
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::TabId;
use copper_core::settings::{Look, TabsPosition};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::rc::Rc;

/// Whether a command is offered right now.
pub type Predicate = Rc<dyn Fn(&Shell) -> bool>;

/// A palette row that runs something.
#[derive(Clone)]
pub struct Command {
    /// Stable id (`new-space`, `toggle-layout`…).
    pub id: String,
    pub name: String,
    /// A symbolic icon name.
    pub icon: String,
    /// The shortcut, as shown (`Ctrl+Shift+T`).
    pub shortcut: Option<String>,
    pub run: Rc<dyn Fn(&Shell)>,
    /// Shown only when this says so (default: always).
    pub when: Option<Predicate>,
}

impl Command {
    pub fn new(id: &str, name: &str, icon: &str, run: impl Fn(&Shell) + 'static) -> Command {
        Command {
            id: id.into(),
            name: name.into(),
            icon: icon.into(),
            shortcut: None,
            run: Rc::new(run),
            when: None,
        }
    }

    pub fn shortcut(mut self, s: &str) -> Command {
        self.shortcut = Some(s.into());
        self
    }

    pub fn when(mut self, f: impl Fn(&Shell) -> bool + 'static) -> Command {
        self.when = Some(Rc::new(f));
        self
    }

    pub fn available(&self, shell: &Shell) -> bool {
        self.when.as_ref().is_none_or(|w| w(shell))
    }
}

/// A page in Settings.
#[derive(Clone)]
pub struct SettingsPage {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub build: Rc<dyn Fn(&Shell) -> adw::PreferencesPage>,
}

/// A side panel (history, downloads, agent…).
#[derive(Clone)]
pub struct Panel {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub build: Rc<dyn Fn(&Shell) -> gtk::Widget>,
}

/// What lanes have added.
#[derive(Default)]
pub struct Registry {
    pub commands: Vec<Command>,
    pub settings_pages: Vec<SettingsPage>,
    pub panels: Vec<Panel>,
    /// `(label, detailed action name)` for the window menu.
    pub menu_items: Vec<(String, String)>,
}

/// A `win.<name>` action, with shortcuts (`<Control>t`, GTK syntax).
pub fn register_action(shell: &Shell, name: &str, accels: &[&str], f: impl Fn(&Shell) + 'static) {
    let action = gio::SimpleAction::new(name, None);
    let weak = shell.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(s) = weak.upgrade() {
            f(&s);
        }
    });
    shell.window().add_action(&action);
    for a in accels {
        shell.ui().add_shortcut(a, &format!("win.{name}"));
    }
}

/// A `win.<name>` action taking a string (a tab or space id).
pub fn register_action_with_id(shell: &Shell, name: &str, f: impl Fn(&Shell, &str) + 'static) {
    let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
    let weak = shell.downgrade();
    action.connect_activate(move |_, arg| {
        let (Some(s), Some(id)) = (weak.upgrade(), arg.and_then(|a| a.get::<String>())) else {
            return;
        };
        f(&s, &id);
    });
    shell.window().add_action(&action);
}

pub fn register_command(shell: &Shell, command: Command) {
    let mut reg = shell.registry().borrow_mut();
    reg.commands.retain(|c| c.id != command.id);
    reg.commands.push(command);
}

pub fn register_settings_page(shell: &Shell, page: SettingsPage) {
    shell.registry().borrow_mut().settings_pages.push(page);
}

pub fn register_panel(shell: &Shell, panel: Panel) {
    let id = panel.id.clone();
    let title = panel.title.clone();
    let icon = panel.icon.clone();
    shell.registry().borrow_mut().panels.push(panel);
    let pid = id.clone();
    register_command(
        shell,
        Command::new(&format!("panel-{id}"), &title, &icon, move |s| {
            s.ui().stage.toggle_panel(s, &pid)
        }),
    );
}

pub fn register_menu_item(shell: &Shell, label: &str, detailed_action: &str) {
    shell
        .registry()
        .borrow_mut()
        .menu_items
        .push((label.into(), detailed_action.into()));
}

/// The commands the palette offers right now.
pub fn commands(shell: &Shell) -> Vec<Command> {
    let list = shell.registry().borrow().commands.clone();
    list.into_iter().filter(|c| c.available(shell)).collect()
}

/// Run a command by id.
pub fn run_command(shell: &Shell, id: &str) -> bool {
    let cmd = shell
        .registry()
        .borrow()
        .commands
        .iter()
        .find(|c| c.id == id)
        .cloned();
    match cmd {
        Some(c) => {
            (c.run)(shell);
            true
        }
        None => false,
    }
}

/// Later lanes register here: one call each.
pub fn register_lanes(shell: &Shell) {
    crate::features::register(shell);
    crate::forms::register(shell);
    crate::cloud::register(shell);
    crate::canvas::register(shell);
    crate::agents::register(shell);
}

fn with_active(s: &Shell, f: impl FnOnce(&Shell, &TabId)) {
    if let Some(id) = s.active_id() {
        f(s, &id);
    }
}

/// The shell's own actions and commands.
pub fn register_core(shell: &Shell) {
    // Tabs.
    register_action(shell, "new-tab", &["<Control>t"], |s| {
        s.ui().card.open(s, Mode::Launch)
    });
    register_action(shell, "new-private-tab", &["<Control><Shift>n"], |s| {
        s.new_private_tab()
    });
    register_action(shell, "close-tab", &["<Control>w", "<Control>F4"], |s| {
        s.close_active()
    });
    register_action(shell, "reopen-tab", &["<Control><Shift>t"], |s| {
        s.reopen();
    });
    register_action(
        shell,
        "next-tab",
        &["<Control>Tab", "<Control>Page_Down"],
        |s| s.cycle_tab(true),
    );
    register_action(
        shell,
        "previous-tab",
        &[
            "<Control><Shift>Tab",
            "<Control><Shift>ISO_Left_Tab",
            "<Control>Page_Up",
        ],
        |s| s.cycle_tab(false),
    );
    for n in 1..=9u32 {
        let accel = format!("<Control>{n}");
        let alt = format!("<Alt>{n}");
        register_action(
            shell,
            &format!("select-tab-{n}"),
            &[&accel, &alt],
            move |s| s.select_nth(if n == 9 { None } else { Some(n as usize - 1) }),
        );
    }
    register_action(shell, "pin-tab", &["<Control><Shift>d"], |s| {
        with_active(s, |s, id| s.toggle_pin(id))
    });

    // The field and the palette.
    register_action(shell, "focus-field", &["<Control>l", "<Alt>d", "F6"], |s| {
        s.ui().edit_address(s)
    });
    register_action(shell, "palette", &["<Control>k"], |s| {
        s.ui().card.toggle(s, Mode::Palette)
    });
    register_action(shell, "escape", &[], |s| s.ui().escape(s));

    // The page.
    register_action(shell, "reload", &["<Control>r", "F5"], |s| {
        s.reload_active()
    });
    register_action(
        shell,
        "hard-reload",
        &["<Control><Shift>r", "<Shift>F5"],
        |s| s.hard_reload_active(),
    );
    register_action(shell, "stop", &[], |s| s.stop_active());
    register_action(shell, "back", &["<Control>bracketleft", "<Alt>Left"], |s| {
        s.back()
    });
    register_action(
        shell,
        "forward",
        &["<Control>bracketright", "<Alt>Right"],
        |s| s.forward(),
    );
    register_action(shell, "find", &["<Control>f"], |s| {
        s.ui().stage.find.open(s)
    });
    register_action(shell, "find-next", &["<Control>g"], |s| {
        s.ui().stage.find.step(s, true)
    });
    register_action(shell, "find-previous", &["<Control><Shift>g"], |s| {
        s.ui().stage.find.step(s, false)
    });
    register_action(
        shell,
        "zoom-in",
        &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
        |s| s.zoom(Some(true)),
    );
    register_action(
        shell,
        "zoom-out",
        &["<Control>minus", "<Control>KP_Subtract"],
        |s| s.zoom(Some(false)),
    );
    register_action(shell, "zoom-reset", &["<Control>0", "<Control>KP_0"], |s| {
        s.zoom(None)
    });
    register_action(shell, "inspector", &["<Control><Shift>i", "F12"], |s| {
        s.show_inspector()
    });

    // Spaces.
    register_action(shell, "next-space", &["<Control><Alt>Right"], |s| {
        s.cycle_space(true)
    });
    register_action(shell, "previous-space", &["<Control><Alt>Left"], |s| {
        s.cycle_space(false)
    });
    register_action(shell, "new-space", &[], crate::spaces::ui::new_space);
    register_action(shell, "edit-space", &[], |s| {
        let id = s.browser().current_space_id().to_string();
        crate::spaces::ui::edit_space(s, &id);
    });
    register_action(shell, "delete-space", &[], |s| {
        let id = s.browser().current_space_id().to_string();
        crate::spaces::ui::ask_delete(s, &id);
    });

    // Window.
    register_action(shell, "toggle-layout", &["<Control><Shift>s"], |s| {
        let next = match s.tabs_position() {
            TabsPosition::Left => TabsPosition::Top,
            TabsPosition::Top => TabsPosition::Left,
        };
        s.set_tabs_position(next);
    });
    register_action(
        shell,
        "settings",
        &["<Control>comma"],
        crate::window::settings::present,
    );
    register_action(shell, "quit", &["<Control>q"], |s| {
        s.shutdown();
        s.app().quit();
    });
    let look = gio::SimpleAction::new_stateful(
        "look",
        Some(glib::VariantTy::STRING),
        &shell.look().as_str().to_variant(),
    );
    let weak = shell.downgrade();
    look.connect_activate(move |a, arg| {
        let Some(v) = arg.and_then(|v| v.get::<String>()) else {
            return;
        };
        let Some(s) = weak.upgrade() else { return };
        let look = match v.as_str() {
            "dark" => Look::Dark,
            "system" => Look::System,
            _ => Look::Light,
        };
        s.set_look(look);
        a.set_state(&v.to_variant());
    });
    shell.window().add_action(&look);
    let layout = gio::SimpleAction::new_stateful(
        "layout",
        Some(glib::VariantTy::STRING),
        &layout_name(shell.tabs_position()).to_variant(),
    );
    let weak = shell.downgrade();
    layout.connect_activate(move |a, arg| {
        let Some(v) = arg.and_then(|v| v.get::<String>()) else {
            return;
        };
        let Some(s) = weak.upgrade() else { return };
        s.set_tabs_position(if v == "top" {
            TabsPosition::Top
        } else {
            TabsPosition::Left
        });
        a.set_state(&v.to_variant());
    });
    shell.window().add_action(&layout);

    // Per-tab actions for context menus (`win.tab-close::<id>`).
    let tab = |f: fn(&Shell, &TabId)| {
        move |s: &Shell, id: &str| {
            let id = TabId(id.to_string());
            f(s, &id)
        }
    };
    register_action_with_id(
        shell,
        "tab-close",
        // The menu's Close Tab is explicit: a Saved row goes too (and
        // comes back saved with Reopen Closed Tab).
        tab(|s, id| {
            s.close_force(id);
        }),
    );
    register_action_with_id(shell, "tab-pin", tab(|s, id| s.toggle_pin(id)));
    register_action_with_id(shell, "tab-reload", tab(|s, id| s.reload(id)));
    register_action_with_id(
        shell,
        "tab-keep",
        tab(|s, id| {
            let saved = s.browser().tab(id).is_some_and(|t| t.saved);
            s.set_saved(id, !saved);
        }),
    );
    register_action_with_id(
        shell,
        "tab-duplicate",
        tab(|s, id| {
            let (url, private) = match s.browser().tab(id) {
                Some(t) => (t.url.clone(), t.private),
                None => return,
            };
            s.open(
                &url,
                crate::shell::OpenTab {
                    foreground: true,
                    private,
                    after: Some(id.clone()),
                    ..Default::default()
                },
            );
        }),
    );
    register_action_with_id(
        shell,
        "tab-close-others",
        tab(|s, id| {
            let others: Vec<TabId> = s
                .visible()
                .all()
                .filter(|t| t.id != *id && t.pin.is_none())
                .map(|t| t.id.clone())
                .collect();
            // Today rows close; Saved rows are only unloaded (Arc).
            for o in others {
                s.close(&o);
            }
        }),
    );
    register_action_with_id(
        shell,
        "tab-copy-address",
        tab(|s, id| {
            if let Some(t) = s.browser().tab(id) {
                s.window().clipboard().set_text(&t.url);
            }
        }),
    );
    // `win.tab-to-space::<tab id>|<space id>`.
    register_action_with_id(shell, "tab-to-space", |s, arg| {
        if let Some((tab, space)) = arg.split_once('|') {
            s.move_to_space(&TabId(tab.to_string()), space);
        }
    });
    register_action_with_id(shell, "space-switch", |s, id| {
        s.switch_space(id);
    });
    register_action_with_id(shell, "space-edit", crate::spaces::ui::edit_space);
    register_action_with_id(shell, "space-delete", crate::spaces::ui::ask_delete);
    register_action_with_id(shell, "space-left", |s, id| s.nudge_space(id, -1));
    register_action_with_id(shell, "space-right", |s, id| s.nudge_space(id, 1));

    // The palette's own commands.
    let cmds = [
        Command::new("new-tab", "New Tab", "tab-new-symbolic", |s| {
            s.ui().card.open(s, Mode::Launch)
        })
        .shortcut("Ctrl+T"),
        Command::new(
            "new-private-tab",
            "New Private Tab",
            "view-conceal-symbolic",
            |s| s.new_private_tab(),
        )
        .shortcut("Ctrl+Shift+N"),
        Command::new(
            "reopen-tab",
            "Reopen Closed Tab",
            "edit-undo-symbolic",
            |s| {
                s.reopen();
            },
        )
        .shortcut("Ctrl+Shift+T")
        .when(|s| !s.browser().closed_tabs().is_empty()),
        Command::new("close-tab", "Close Tab", "window-close-symbolic", |s| {
            s.close_active()
        })
        .shortcut("Ctrl+W")
        .when(|s| s.active_id().is_some()),
        Command::new("pin-tab", "Pin or Unpin Tab", "view-pin-symbolic", |s| {
            with_active(s, |s, id| s.toggle_pin(id))
        })
        .shortcut("Ctrl+Shift+D")
        .when(|s| s.active_id().is_some()),
        Command::new("find", "Find in Page", "edit-find-symbolic", |s| {
            s.ui().stage.find.open(s)
        })
        .shortcut("Ctrl+F")
        .when(|s| s.active_page().is_some()),
        Command::new(
            "hard-reload",
            "Reload Without Cache",
            "view-refresh-symbolic",
            |s| s.hard_reload_active(),
        )
        .shortcut("Ctrl+Shift+R")
        .when(|s| s.active_page().is_some()),
        Command::new("copy-address", "Copy Address", "edit-copy-symbolic", |s| {
            if let Some(t) = s.browser().active() {
                s.window().clipboard().set_text(&t.url);
            }
            s.toast("Address copied");
        })
        .when(|s| s.active_id().is_some()),
        Command::new("new-space", "New Space", "list-add-symbolic", |s| {
            crate::spaces::ui::new_space(s)
        }),
        Command::new("edit-space", "Edit Space", "document-edit-symbolic", |s| {
            let id = s.browser().current_space_id().to_string();
            crate::spaces::ui::edit_space(s, &id);
        }),
        Command::new("next-space", "Next Space", "go-next-symbolic", |s| {
            s.cycle_space(true)
        })
        .shortcut("Ctrl+Alt+→")
        .when(|s| s.browser().spaces().count() > 1),
        Command::new(
            "previous-space",
            "Previous Space",
            "go-previous-symbolic",
            |s| s.cycle_space(false),
        )
        .shortcut("Ctrl+Alt+←")
        .when(|s| s.browser().spaces().count() > 1),
        Command::new(
            "delete-space",
            "Delete Space…",
            "user-trash-symbolic",
            |s| {
                let id = s.browser().current_space_id().to_string();
                crate::spaces::ui::ask_delete(s, &id);
            },
        )
        .when(|s| s.browser().spaces().count() > 1),
        Command::new(
            "toggle-layout",
            "Move Tabs to the Top or Side",
            "view-dual-symbolic",
            |s| {
                let next = match s.tabs_position() {
                    TabsPosition::Left => TabsPosition::Top,
                    TabsPosition::Top => TabsPosition::Left,
                };
                s.set_tabs_position(next);
            },
        )
        .shortcut("Ctrl+Shift+S"),
        Command::new(
            "look-light",
            "Light Appearance",
            "weather-clear-symbolic",
            |s| s.set_look(Look::Light),
        )
        .when(|s| s.look() != Look::Light),
        Command::new(
            "look-dark",
            "Dark Appearance",
            "weather-clear-night-symbolic",
            |s| s.set_look(Look::Dark),
        )
        .when(|s| s.look() != Look::Dark),
        Command::new(
            "look-system",
            "Match System Appearance",
            "emblem-system-symbolic",
            |s| s.set_look(Look::System),
        )
        .when(|s| s.look() != Look::System),
        Command::new(
            "inspector",
            "Web Inspector",
            "applications-engineering-symbolic",
            |s| s.show_inspector(),
        )
        .shortcut("Ctrl+Shift+I")
        .when(|s| s.active_page().is_some()),
        Command::new("settings", "Settings", "emblem-system-symbolic", |s| {
            crate::window::settings::present(s)
        })
        .shortcut("Ctrl+,"),
    ];
    for c in cmds {
        register_command(shell, c);
    }
}

fn layout_name(p: TabsPosition) -> &'static str {
    match p {
        TabsPosition::Left => "left",
        TabsPosition::Top => "top",
    }
}

/// A space's "Switch to …" commands are built fresh each time the palette
/// opens (spaces come and go).
pub fn space_commands(shell: &Shell) -> Vec<Command> {
    let b = shell.browser();
    let current = b.current_space_id().to_string();
    b.spaces()
        .filter(|sp| sp.id != current)
        .map(|sp| {
            let id = sp.id.clone();
            Command::new(
                &format!("space-{}", sp.id),
                &format!("Switch to {}", sp.name),
                "view-grid-symbolic",
                move |s| {
                    s.switch_space(&id);
                },
            )
        })
        .collect()
}
