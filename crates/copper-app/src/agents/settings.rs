//! Settings › Agents (Mac: `AgentsPage` in `Fork/SettingsFork.swift`):
//! the server switch and its status (a busy port says so), announcing, the
//! port, Jev mode, the two "Copy config" rows (HTTP and stdio), and the
//! bearer token with Copy and Rotate.

use super::{Agents, agents};
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
    token: gtk::Label,
    note: gtk::Label,
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
        "An MCP server on this computer only (127.0.0.1). Agents in your terminal or editor see your open tabs and act in them, in the browser you're already signed in to.",
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
    let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
    port.set_title("Port");
    port.set_numeric(true);
    port.set_digits(0);
    port.set_climb_rate(1.0);
    server.add(&port);
    page.add(&server);

    // — Jev —
    let jev_group = adw::PreferencesGroup::new();
    jev_group.set_title("Jev mode");
    let jev = adw::SwitchRow::new();
    jev.set_title("Let the agent hand Copper a goal");
    jev.set_subtitle(
        "Adds jev_run, jev_step, jev_observe and jev_extract. Jev picks the next click or key about every 200 ms and Copper does it in this window, until the goal is done.",
    );
    jev_group.add(&jev);
    let jev_key = adw::ActionRow::new();
    jev_key.set_title("Jev key");
    jev_group.add(&jev_key);
    page.add(&jev_group);

    // — connecting —
    let connect = adw::PreferencesGroup::new();
    connect.set_title("Connect a client");
    let http = adw::ActionRow::new();
    http.set_title("Claude Code, phi, Cursor");
    http.set_subtitle(
        "Over HTTP. Paste into ~/.claude.json, ~/.pi/agent/mcp.json or the editor's MCP settings.",
    );
    http.add_suffix(&copy_button("Copy config", |a| a.http_config()));
    connect.add(&http);
    let stdio = adw::ActionRow::new();
    stdio.set_title("Claude Desktop and other stdio clients");
    stdio.set_subtitle(
        "Runs copper --mcp-stdio, a pipe to this window that starts Copper if it isn't running.",
    );
    stdio.add_suffix(&copy_button("Copy config", |a| a.stdio_config()));
    connect.add(&stdio);
    page.add(&connect);

    // — the key —
    let key = adw::PreferencesGroup::new();
    key.set_title("The key");
    key.set_description(Some(
        "Every request must carry it. Kept in agent.json, readable by you alone.",
    ));
    let token_row = adw::ActionRow::new();
    token_row.set_title("Bearer token");
    token_row.set_subtitle("A new one stops every client set up with the old one.");
    let token = gtk::Label::new(None);
    token.add_css_class("agent-token");
    token.set_valign(gtk::Align::Center);
    token_row.add_suffix(&token);
    token_row.add_suffix(&copy_button("Copy", |a| a.token()));
    let rotate = gtk::Button::with_label("Rotate token…");
    rotate.set_valign(gtk::Align::Center);
    rotate.connect_clicked(|b| confirm_rotate(b.upcast_ref()));
    token_row.add_suffix(&rotate);
    key.add(&token_row);
    let note = gtk::Label::new(None);
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.set_margin_top(12);
    key.add(&note);
    page.add(&key);

    let rows = Rc::new(Rows {
        enabled,
        status,
        dot,
        announces,
        port,
        jev,
        jev_key,
        token,
        note,
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
fn sync(rows: &Rows, a: &Agents, shell: &Shell) {
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
    rows.jev_key.set_visible(a.jev());
    let source = copper_mcp::intelligence::read_files(shell.store());
    let effective = copper_mcp::intelligence::Effective::new(&source, false);
    let jev_line = match effective.sources.get("jevKey").map(String::as_str) {
        _ if !effective.jev_ready() => {
            "Jev mode needs a Jev key (ts-…) in intelligence.json, the one Settings › Intelligence uses."
        }
        Some("cloud") => "From Copper Cloud.",
        _ => "Set in intelligence.json, the same key Settings › Intelligence uses.",
    };
    rows.jev_key.set_subtitle(jev_line);
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
            "{calls} tool call{} this session — last: {last}\n",
            if calls == 1 { "" } else { "s" }
        ));
    }
    note.push_str(
        "Agents act as you: whatever you are signed in to, they are too. Turn the server off when you don't need it.",
    );
    rows.note.set_text(&note);
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
