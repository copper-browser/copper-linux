//! Settings › Agents (Mac: `AgentsPage` in `Fork/SettingsFork.swift`):
//! the server switch and its status (a busy port says so), announcing, and
//! under Advanced the port and the bearer token with Copy and Rotate; Jev
//! mode; one "Copy for another agent" row (HTTP and stdio configs). As the
//! Mac since Copper#115/#118: no "The key" section, the note is the server's.
//! The Jev key lives on Intelligence; Jev mode only points there while it is
//! missing (Copper#118), and folds the text model under its own Advanced.

use super::{Agents, Server, agents};
use crate::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

/// Open Settings on this page.
pub fn present(shell: &Shell) -> adw::PreferencesDialog {
    let d = crate::window::settings::dialog(shell);
    d.set_visible_page_name("agents");
    d
}

struct Rows {
    enabled: adw::SwitchRow,
    status: adw::ActionRow,
    dot: gtk::Box,
    announces: adw::SwitchRow,
    port: adw::SpinRow,
    jev: adw::SwitchRow,
    jev_key: adw::ActionRow,
    jev_more: adw::ExpanderRow,
    text_model: adw::ActionRow,
    text_dot: gtk::Box,
    text_field: gtk::Entry,
    token: gtk::Label,
    server: adw::PreferencesGroup,
    /// Set while the rows are being brought in line with the state, so
    /// their change handlers do not write it back.
    syncing: Cell<bool>,
}

/// The page (registered with `actions::register_settings_page`).
pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let Some(a) = agents() else {
        return page;
    };

    // — the server —
    let server = adw::PreferencesGroup::new();
    server.set_title("MCP server");
    let enabled = adw::SwitchRow::new();
    enabled.set_title("Let agents drive this window");
    enabled.set_subtitle(
        "An MCP server on this computer only (127.0.0.1). Agents in your terminal or editor see your tabs and act in them, signed in as you.",
    );
    server.add(&enabled);
    let status = adw::ActionRow::new();
    status.set_title("Status");
    status.set_subtitle_selectable(true);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("agent-status-dot");
    dot.set_valign(gtk::Align::Center);
    dot.set_halign(gtk::Align::Center);
    dot.set_size_request(8, 8);
    dot.update_property(&[gtk::accessible::Property::Label("Server status")]);
    status.add_suffix(&dot);
    server.add(&status);
    let announces = adw::SwitchRow::new();
    announces.set_title("Say what the agent does");
    announces.set_subtitle("Each tool call, in the bar over the page it works in");
    server.add(&announces);
    // Mac: Advanced folds the port and the token (Copper#118).
    let advanced = adw::ExpanderRow::new();
    advanced.set_title("Advanced");
    advanced.set_subtitle("2 more settings");
    let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
    port.set_title("Port");
    port.set_numeric(true);
    port.set_digits(0);
    port.set_climb_rate(1.0);
    advanced.add_row(&port);
    let token_row = adw::ActionRow::new();
    token_row.set_title("Bearer token");
    token_row.set_subtitle(
        "Every request must carry it; it stays in agent.json, readable by you alone. A new token stops every client set up with the old one.",
    );
    let token = gtk::Label::new(None);
    token.add_css_class("agent-token");
    token.set_valign(gtk::Align::Center);
    token_row.add_suffix(&token);
    token_row.add_suffix(&copy_button("Copy", |a| a.token()));
    let rotate = gtk::Button::with_label("Rotate token…");
    rotate.set_valign(gtk::Align::Center);
    rotate.connect_clicked(|b| confirm_rotate(b.upcast_ref()));
    token_row.add_suffix(&rotate);
    advanced.add_row(&token_row);
    server.add(&advanced);
    page.add(&server);

    // — Jev —
    let jev_group = adw::PreferencesGroup::new();
    jev_group.set_title("Jev mode");
    let jev = adw::SwitchRow::new();
    jev.set_title("Let the agent hand Copper a goal");
    jev.set_subtitle(
        "Adds jev_run, jev_step, jev_observe and jev_extract: Jev clicks and types in this window about every 200 ms until the goal is done.",
    );
    jev_group.add(&jev);
    let jev_key = adw::ActionRow::new();
    jev_key.set_title("Jev key");
    jev_key.set_subtitle(
        "Jev mode needs a key from your TypeSafe account. It goes in Intelligence › Jev.",
    );
    let add_key = gtk::Button::with_label("Add key");
    add_key.add_css_class("suggested-action");
    add_key.set_valign(gtk::Align::Center);
    add_key.connect_clicked(|b| {
        if let Some(d) = b
            .ancestor(adw::PreferencesDialog::static_type())
            .and_downcast::<adw::PreferencesDialog>()
        {
            d.set_visible_page_name(crate::features::settings::intelligence::ID);
        }
    });
    jev_key.add_suffix(&add_key);
    jev_group.add(&jev_key);
    // Mac: Jev mode's Advanced (Copper#118); Linux has the text model only.
    let jev_more = adw::ExpanderRow::new();
    jev_more.set_title("Advanced");
    jev_more.set_subtitle("1 more setting");
    let text_model = adw::ActionRow::new();
    text_model.set_title("Text model");
    text_model.set_subtitle_lines(3);
    let text_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    text_dot.add_css_class("agent-status-dot");
    text_dot.set_valign(gtk::Align::Center);
    text_dot.set_size_request(8, 8);
    text_model.add_suffix(&text_dot);
    let text_field = gtk::Entry::new();
    text_field.set_text(
        crate::features::settings::intelligence::local(shell)
            .text_model
            .as_deref()
            .unwrap_or(""),
    );
    text_field.set_width_chars(14);
    text_field.add_css_class("monospace");
    text_field.set_valign(gtk::Align::Center);
    text_field.update_property(&[gtk::accessible::Property::Label("Text model")]);
    text_model.add_suffix(&text_field);
    jev_more.add_row(&text_model);
    jev_group.add(&jev_more);
    page.add(&jev_group);

    // — connecting —
    let connect = adw::PreferencesGroup::new();
    connect.set_title("Connect a client");
    let copy = adw::ActionRow::new();
    copy.set_title("Copy for another agent");
    copy.set_subtitle(
        "The MCP config, with the address and token, for Claude Code, phi or Cursor; or over stdio, for Claude Desktop",
    );
    copy.add_suffix(&copy_button("Copy config", |a| a.http_config()));
    copy.add_suffix(&copy_button("Copy stdio config", |a| a.stdio_config()));
    connect.add(&copy);
    page.add(&connect);

    let rows = Rc::new(Rows {
        enabled,
        status,
        dot,
        announces,
        port,
        jev,
        jev_key,
        jev_more,
        text_model,
        text_dot,
        text_field,
        token,
        server,
        syncing: Cell::new(false),
    });
    sync(&rows, &a, shell);

    rows.enabled.connect_active_notify({
        let rows = rows.clone();
        move |r| {
            if !rows.syncing.get()
                && let Some(a) = agents()
            {
                a.set_enabled(r.is_active());
            }
        }
    });
    rows.announces.connect_active_notify({
        let rows = rows.clone();
        move |r| {
            if !rows.syncing.get()
                && let Some(a) = agents()
            {
                a.set_announces(r.is_active());
            }
        }
    });
    rows.jev.connect_active_notify({
        let rows = rows.clone();
        move |r| {
            if !rows.syncing.get()
                && let Some(a) = agents()
            {
                a.set_jev(r.is_active());
            }
        }
    });
    rows.text_field.connect_changed({
        let weak = shell.downgrade();
        let rows = Rc::downgrade(&rows);
        move |e| {
            let Some(s) = weak.upgrade() else { return };
            let text = e.text().trim().to_string();
            crate::features::settings::intelligence::edit(&s, |k| {
                k.text_model = (!text.is_empty()).then_some(text)
            });
            if let (Some(rows), Some(a)) = (rows.upgrade(), agents()) {
                sync(&rows, &a, &s);
            }
        }
    });
    rows.port.connect_value_notify({
        let rows = rows.clone();
        move |r| {
            if !rows.syncing.get()
                && let Some(a) = agents()
            {
                a.set_port(r.value().round().clamp(1.0, 65535.0) as u16);
            }
        }
    });

    let weak_page = page.downgrade();
    let weak_shell = shell.downgrade();
    a.watch(move |a| {
        let (Some(_), Some(shell)) = (weak_page.upgrade(), weak_shell.upgrade()) else {
            return false;
        };
        sync(&rows, a, &shell);
        true
    });
    page
}

/// Bring every row in line with the state.
fn sync(rows: &Rows, a: &Server, shell: &Shell) {
    rows.syncing.set(true);
    rows.enabled.set_active(a.enabled());
    rows.announces.set_active(a.announces());
    rows.jev.set_active(a.jev());
    let pinned = copper_mcp::config::port_override();
    rows.port.set_value(a.configured_port() as f64);
    rows.port.set_sensitive(pinned.is_none());
    rows.port.set_subtitle(&match pinned {
        Some(p) => format!("This run listens on {p}, set by SEARCH_MCP_PORT."),
        None => format!(
            "Change it if another program already uses {}.",
            a.configured_port()
        ),
    });
    let trouble = a.trouble();
    let line = match (&trouble, a.running(), a.enabled()) {
        (Some(t), _, _) if pinned.is_none() => format!("{t} — choose another port below."),
        (Some(t), _, _) => t.clone(),
        (None, true, _) => format!("Listening at {}", a.endpoint()),
        (None, false, true) => "Starting…".into(),
        (None, false, false) => "Off — no agent can reach this browser.".into(),
    };
    rows.status.set_subtitle(&line);
    for c in ["on", "trouble"] {
        rows.dot.remove_css_class(c);
    }
    if trouble.is_some() {
        rows.dot.add_css_class("trouble");
    } else if a.running() {
        rows.dot.add_css_class("on");
    }
    // Mac: the key row shows only while it is missing (Copper#118).
    let effective = crate::features::settings::intelligence::effective(shell);
    rows.jev_key.set_visible(a.jev() && !effective.jev_ready());
    rows.jev_more.set_visible(a.jev());
    let ready = effective.model_ready();
    let model = copper_mcp::intelligence::model(&effective.keys, None);
    rows.text_model.set_subtitle(&if ready {
        format!(
            "Writes what Jev types and answers jev_extract. Small and fast is the point; empty uses {model}."
        )
    } else {
        "Typing text and jev_extract need a model — set one up in Intelligence.".into()
    });
    rows.text_field.set_placeholder_text(Some(&model));
    for c in ["on", "trouble"] {
        rows.text_dot.remove_css_class(c);
    }
    rows.text_dot
        .add_css_class(if ready { "on" } else { "trouble" });
    rows.text_dot.set_tooltip_text(Some(if ready {
        "A model is set up"
    } else {
        "No model is set up"
    }));
    let token = a.token();
    let shown: String = token.chars().take(8).collect();
    rows.token.set_text(&format!("{shown}…"));
    rows.token
        .update_property(&[gtk::accessible::Property::Label(&format!(
            "Token starting {shown}"
        ))]);
    let (calls, last) = a.stats();
    let mut note = String::new();
    if calls > 0 {
        note.push_str(&format!(
            "{calls} tool call{} this session — last: {last}. ",
            if calls == 1 { "" } else { "s" }
        ));
    }
    note.push_str(
        "Agents act as you: whatever you are signed in to, they are too. Turn the server off when you don't need it.",
    );
    rows.server.set_description(Some(&note));
    rows.syncing.set(false);
}

/// A button that copies `what()` and says so for a moment.
fn copy_button(label: &'static str, what: impl Fn(&Agents) -> String + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_valign(gtk::Align::Center);
    b.connect_clicked(move |b| {
        let Some(a) = agents() else { return };
        b.clipboard().set_text(&what(&a));
        b.set_label("Copied");
        let weak = b.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(1600), move || {
            if let Some(b) = weak.upgrade() {
                b.set_label(label);
            }
        });
    });
    b
}

fn confirm_rotate(from: &gtk::Widget) {
    let d = adw::AlertDialog::new(
        Some("Make a new token?"),
        Some(
            "Every client set up with the current token stops working until it is set up again with the new one.",
        ),
    );
    d.add_responses(&[("cancel", "Cancel"), ("rotate", "Make a New Token")]);
    d.set_response_appearance("rotate", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    d.connect_response(None, |_, resp| {
        if resp == "rotate"
            && let Some(a) = agents()
        {
            a.rotate_token();
        }
    });
    d.present(Some(from));
}
