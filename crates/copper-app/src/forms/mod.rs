//! Passwords, form fill and moving in from Chrome in the GTK window (lane:
//! creds-ui; PLAN T-015 + T-016 app side). Watching pages, the vaults and
//! the agents' sign-in tools are copper-shell-core's
//! [`copper_shell_core::forms`] (re-exported here); this is what GTK adds:
//!
//! * [`offer`] — the save offer: a quiet capsule over the top of the page.
//! * [`picker`] — the saved accounts in a small list under the box.
//! * [`keyring`] — the Secret Service, looked at without opening it.
//! * [`settings`] — Settings › Passwords.
//! * [`import`] — the "Bring in from Chrome, Chromium or Brave" sheet.

// `vault` is the module and the handle (`vault()`).
pub use copper_shell_core::forms::{
    OfferData, Row, RowKind, agent_allowed, agent_allowed_id, chosen_backend, keys, mcp,
    never_list, remember_op, rt, set_agent_allowed, set_backend, set_never, site_host,
    source_title, vault, view, watch,
};

pub mod import;
pub mod keyring;
pub mod offer;
pub mod picker;
#[cfg(debug_assertions)]
pub mod selftest;
pub mod settings;

use crate::actions::{Command, SettingsPage, register_command, register_settings_page};
use crate::platform::Gtk;
use crate::shell::Shell;
use copper_core::TabId;
use copper_pagejs::messages::FieldRect;
use copper_shell_core::forms::FormsUi;
use copper_shell_core::shell::pages::PageSlot;
use std::rc::Rc;

/// The lane's main-thread state, with GTK's widgets.
pub type Forms = copper_shell_core::forms::Forms<GtkForms>;

/// The forms lane's widgets on GTK: overlays on the page's frame.
pub enum GtkForms {}

impl FormsUi for GtkForms {
    type Platform = Gtk;
    type Offer = offer::Offer;
    type Picker = picker::Picker;

    fn show_offer(
        shell: &Shell,
        forms: &Rc<Forms>,
        tab: &TabId,
        slot: &Rc<PageSlot<Gtk>>,
        data: OfferData,
    ) -> offer::Offer {
        offer::show(shell, forms, tab, slot, data)
    }

    fn show_picker(
        shell: &Shell,
        forms: &Rc<Forms>,
        tab: &TabId,
        slot: &Rc<PageSlot<Gtk>>,
        rect: FieldRect,
        rows: Vec<Row>,
    ) -> picker::Picker {
        picker::show(shell, forms, tab, slot, rect, rows)
    }
}

/// The lane's state, once [`register`] ran.
pub fn forms() -> Option<Rc<Forms>> {
    copper_shell_core::forms::get::<GtkForms>()
}

const CSS: &str = r#"
.copper-forms-offer {
  background-color: @popover_bg_color; color: @popover_fg_color;
  border-radius: 999px; padding: 5px 6px 5px 16px;
  box-shadow: 0 0 0 1px alpha(@popover_fg_color, .12), 0 2px 5px alpha(black, .10);
}
.copper-forms-offer .text { font-size: 13px; }
.copper-forms-offer button { min-height: 26px; padding: 0 12px; border-radius: 999px; font-size: 12.5px; }
.copper-forms-offer button.flat { color: alpha(@popover_fg_color, .7); }
.copper-forms-picker {
  background-color: @popover_bg_color; color: @popover_fg_color;
  border-radius: 10px; padding: 4px;
  box-shadow: 0 0 0 1px alpha(@popover_fg_color, .12), 0 3px 6px alpha(black, .12);
}
.copper-forms-picker list { background: transparent; }
.copper-forms-picker row { border-radius: 7px; padding: 6px 10px; min-height: 30px; }
.copper-forms-picker row:hover { background-color: alpha(@popover_fg_color, .07); }
.copper-forms-picker .user { font-size: 13px; font-weight: 500; }
.copper-forms-picker .where { font-size: 11.5px; opacity: .6; }
.copper-forms-picker .key { opacity: .55; }
.copper-forms-picker .busy { font-size: 11.5px; opacity: .6; }
.copper-forms-secret { font-family: monospace; }
"#;

/// Called once from `actions::register_lanes`.
pub fn register(shell: &Shell) {
    copper_shell_core::forms::start::<GtkForms>(shell, keyring::look);
    copper_shell_core::forms::autolock(shell);

    let css = gtk4::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    register_settings_page(
        shell,
        SettingsPage {
            id: "passwords".into(),
            title: "Passwords".into(),
            icon: "dialog-password-symbolic".into(),
            build: Rc::new(settings::page),
        },
    );
    crate::actions::register_action(shell, "bring-in", &[], import::present);
    register_command(
        shell,
        Command::new(
            "bring-in",
            "Bring In from Chrome, Chromium or Brave…",
            "document-open-symbolic",
            import::present,
        ),
    );
    crate::actions::register_menu_item(shell, "Bring In from Chrome…", "win.bring-in");
    register_command(
        shell,
        Command::new("passwords", "Passwords", "dialog-password-symbolic", |s| {
            settings::present(s)
        }),
    );

    #[cfg(debug_assertions)]
    if std::env::var("COPPER_SELFTEST").is_ok_and(|v| v == "forms") {
        selftest::start(shell);
    }
}
