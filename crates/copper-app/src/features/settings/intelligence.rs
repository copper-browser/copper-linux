//! Settings › Intelligence (Mac: `IntelligencePage` in
//! `Fork/SettingsFork.swift`, `IntelligenceCheck.swift`): the gateway that
//! answers Jev's text and `jev_extract`, the model size and its names, Check,
//! and the Jev key.
//!
//! Keys live in `intelligence.json` (0600), the file the agents' MCP host
//! reads on every call ([`copper_mcp::intelligence::read_files`]), so a key
//! typed here is used at once. While Copper Cloud provides the gateway key
//! it decides the lane and the key and address rows fade; this computer's
//! own entries stay in the file for after sign-out
//! ([`copper_mcp::intelligence::merge`]). The Claude account lane is not on
//! Linux: there is no Use chooser, only the API key.

use super::{button, group, line, segmented};
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::intelligence::IntelligenceKeys;
use copper_mcp::intelligence::check::{self, Check, JEV_KEY_PAGE, is_web_address};
use copper_mcp::intelligence::{self as brain, Effective, Keys, Lane, Tier};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// The page's name in the dialog (`window/settings.rs ORDER`).
pub const ID: &str = "intelligence";

const TIERS: [(Tier, &str, &str); 3] = [
    (Tier::Haiku, "Haiku", "quick"),
    (Tier::Sonnet, "Sonnet", "the balance"),
    (Tier::Opus, "Opus", "thinks hardest"),
];

thread_local! {
    /// The last check outlives the page, as on the Mac.
    static CHECK: RefCell<Check> = RefCell::default();
}

/// `intelligence.json` as it is on disk (absent reads as empty).
pub fn local(shell: &Shell) -> IntelligenceKeys {
    shell
        .store()
        .load::<IntelligenceKeys>()
        .map(|l| l.value)
        .unwrap_or_default()
}

/// Change `intelligence.json`, keeping every field this page doesn't know.
pub fn edit(shell: &Shell, f: impl FnOnce(&mut IntelligenceKeys)) {
    let mut k = local(shell);
    f(&mut k);
    if let Err(e) = shell.store().save(&k) {
        tracing::warn!("intelligence.json: {e}");
    }
}

/// The keys in force: this computer's, Copper Cloud's and the environment's.
/// No Claude account on Linux.
pub fn effective(shell: &Shell) -> Effective {
    Effective::new(&brain::read_files(shell.store()), false)
}

/// What a key or address row says when it isn't this computer's own
/// (`Intelligence.cloudLine`).
fn elsewhere(shell: &Shell, eff: &Effective, field: &str) -> Option<String> {
    match eff.sources.get(field).map(String::as_str) {
        Some("cloud") => {
            let host = brain::read_files(shell.store())
                .cloud
                .and_then(|c| c.host)
                .unwrap_or_else(|| "your cloud".into());
            Some(format!("Provided by Copper Cloud ({host})"))
        }
        Some("env") => Some("Set for this run by COPPER_ROUTER_URL and COPPER_ROUTER_KEY".into()),
        _ => None,
    }
}

/// A text box at the end of a line; a secret one has an eye.
fn field(placeholder: &str, text: &str, secret: bool, chars: i32) -> gtk::Widget {
    let e: gtk::Widget = if secret {
        let p = gtk::PasswordEntry::new();
        p.set_show_peek_icon(true);
        p.set_placeholder_text(Some(placeholder));
        p.upcast()
    } else {
        let e = gtk::Entry::new();
        e.set_placeholder_text(Some(placeholder));
        e.upcast()
    };
    let editable = e.downcast_ref::<gtk::Editable>().expect("entries edit");
    editable.set_text(text);
    editable.set_width_chars(chars);
    e.add_css_class("monospace");
    e.set_valign(gtk::Align::Center);
    e
}

/// Save what is typed in `e` as it changes, then bring the rows in line.
fn on_change(
    rows: &Rc<Rows>,
    shell: &Shell,
    e: &gtk::Widget,
    f: impl Fn(&mut IntelligenceKeys, Option<String>) + 'static,
) {
    let (weak, rows) = (shell.downgrade(), Rc::downgrade(rows));
    e.downcast_ref::<gtk::Editable>()
        .expect("entries edit")
        .connect_changed(move |e| {
            let Some(s) = weak.upgrade() else { return };
            let text = e.text().trim().to_string();
            edit(&s, |k| f(k, (!text.is_empty()).then_some(text)));
            if let Some(rows) = rows.upgrade() {
                sync(&rows, &s);
            }
        });
}

/// A key's dot: set, turned away by the last check, or none.
fn dot() -> gtk::Box {
    let d = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    d.add_css_class("agent-status-dot"); // agents/mark.rs's style
    d.set_valign(gtk::Align::Center);
    d.set_size_request(8, 8);
    d
}

fn set_dot(d: &gtk::Box, ready: bool, refused: bool) {
    for c in ["on", "trouble"] {
        d.remove_css_class(c);
    }
    if refused {
        d.add_css_class("trouble");
    } else if ready {
        d.add_css_class("on");
    }
    d.set_tooltip_text(Some(match (refused, ready) {
        (true, _) => "Turned away by the last check",
        (false, true) => "Set",
        (false, false) => "Not set",
    }));
}

struct Rows {
    lane: adw::ActionRow,
    key: adw::ActionRow,
    key_dot: gtk::Box,
    key_field: gtk::Widget,
    gateway: adw::ActionRow,
    gateway_field: gtk::Widget,
    model: adw::ActionRow,
    check: adw::ActionRow,
    test: gtk::Button,
    jev: adw::ActionRow,
    jev_dot: gtk::Box,
    jev_field: gtk::Widget,
    get_key: gtk::Button,
    jev_group: adw::PreferencesGroup,
}

/// The page (registered by `features::settings`).
pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let file = local(shell);
    let mine = Keys::from_file(&file);
    let eff = effective(shell);
    // While Copper Cloud decides the lane its key is used: the boxes read empty.
    let cloud_lane = eff.sources.get("lane").map(String::as_str) == Some("cloud");

    // — Model access —
    let access = group(Some("Model access"));
    // Only when this computer's file chose the Mac's Claude account lane.
    let lane = line(
        "Use",
        "This computer is set to a Claude account, which can't sign in on Linux yet — use an API key for a model gateway",
    );
    let use_key = button("Use an API key", true);
    lane.add_suffix(&use_key);
    access.add(&lane);
    let key = line("API key", "");
    let key_dot = dot();
    let key_field = field(
        "sk-…",
        if cloud_lane { "" } else { &mine.router_key },
        true,
        24,
    );
    key_field.update_property(&[gtk::accessible::Property::Label("Gateway API key")]);
    key.add_suffix(&key_dot);
    key.add_suffix(&key_field);
    access.add(&key);
    let gateway = line("Gateway address", "");
    let typed_url = match (&file.router_url, eff.sources.get("routerURL")) {
        _ if cloud_lane => "",
        (None, Some(s)) if s == "cloud" => "",
        _ => mine.router_url.as_str(),
    };
    let gateway_field = field("https://…", typed_url, false, 24);
    gateway_field.update_property(&[gtk::accessible::Property::Label("Gateway address")]);
    gateway.add_suffix(&gateway_field);
    access.add(&gateway);
    let model = line("Model", "");
    let at = TIERS
        .iter()
        .position(|(t, ..)| *t == mine.tier)
        .unwrap_or(1);
    let names: Vec<&str> = TIERS.iter().map(|(_, n, _)| *n).collect();
    let rows_cell: Rc<RefCell<Option<std::rc::Weak<Rows>>>> = Rc::default();
    let picked = rows_cell.clone();
    model.add_suffix(&segmented(shell, &names, at, move |s, i| {
        edit(s, |k| k.tier = Some(TIERS[i].0.name().into()));
        if let Some(rows) = picked.borrow().as_ref().and_then(|w| w.upgrade()) {
            sync(&rows, s);
        }
    }));
    access.add(&model);
    let check_row = line("Check", "");
    let test = button("Test", false);
    check_row.add_suffix(&test);
    access.add(&check_row);
    // Model names are for gateways that call them something else: folded
    // under Advanced (Copper#118).
    let advanced = adw::ExpanderRow::new();
    advanced.set_title("Advanced");
    advanced.set_subtitle("1 more setting");
    advanced.add_row(&line(
        "Model names",
        "What Haiku, Sonnet and Opus are called on your gateway. Leave one empty for the plain name.",
    ));
    let mut name_fields = Vec::new();
    for (t, title, _) in TIERS {
        let typed = file
            .router_models
            .as_ref()
            .and_then(|m| m.get(t.name()))
            .cloned()
            .unwrap_or_default();
        let r = line(title, "");
        let e = field(t.name(), &typed, false, 18);
        e.update_property(&[gtk::accessible::Property::Label(&format!(
            "{title} model name"
        ))]);
        r.add_suffix(&e);
        advanced.add_row(&r);
        name_fields.push((t, e));
    }
    access.add(&advanced);
    page.add(&access);

    // — Jev —
    let jev_group = group(Some("Jev"));
    let jev = line("Jev key", "");
    let jev_dot = dot();
    let get_key = button("Get a key", false);
    get_key.set_tooltip_text(Some(JEV_KEY_PAGE));
    let weak = shell.downgrade();
    get_key.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            open_key_page(&s);
        }
    });
    let jev_field = field("ts-…", &mine.jev_key, true, 16);
    jev_field.update_property(&[gtk::accessible::Property::Label("Jev key")]);
    jev.add_suffix(&get_key);
    jev.add_suffix(&jev_dot);
    jev.add_suffix(&jev_field);
    jev_group.add(&jev);
    page.add(&jev_group);

    let rows = Rc::new(Rows {
        lane,
        key,
        key_dot,
        key_field,
        gateway,
        gateway_field,
        model,
        check: check_row,
        test,
        jev,
        jev_dot,
        jev_field,
        get_key,
        jev_group,
    });
    *rows_cell.borrow_mut() = Some(Rc::downgrade(&rows));
    on_change(&rows, shell, &rows.key_field, |k, v| k.router_key = v);
    on_change(&rows, shell, &rows.gateway_field, |k, v| k.router_url = v);
    on_change(&rows, shell, &rows.jev_field, |k, v| k.jev_key = v);
    for (t, e) in name_fields {
        on_change(&rows, shell, &e, move |k, v| {
            // The whole map, as the Mac writes it, so a lone routerModel
            // that named Sonnet's model keeps doing so.
            let mut map = Keys::from_file(k).router_models;
            match v {
                Some(n) => map.insert(t.name().into(), n),
                None => map.remove(t.name()),
            };
            k.router_models = Some(map);
        });
    }
    let (weak, w) = (shell.downgrade(), Rc::downgrade(&rows));
    use_key.connect_clicked(move |_| {
        if let (Some(s), Some(rows)) = (weak.upgrade(), w.upgrade()) {
            edit(&s, |k| k.lane = Some("key".into()));
            sync(&rows, &s);
        }
    });
    let (weak, w) = (shell.downgrade(), Rc::downgrade(&rows));
    rows.test.connect_clicked(move |_| {
        if let (Some(s), Some(rows)) = (weak.upgrade(), w.upgrade()) {
            run_check(&rows, &s);
        }
    });
    sync(&rows, shell);
    // The rows live as long as the page.
    let keep = rows.clone();
    page.connect_destroy(move |_| {
        let _ = &keep;
    });
    page
}

/// TypeSafe's console, where a Jev key is made (`Jev.keyPage`).
fn open_key_page(shell: &Shell) {
    gtk::UriLauncher::new(JEV_KEY_PAGE).launch(Some(shell.window()), gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            tracing::warn!(error = %e, "Jev key page");
        }
    });
}

/// Ask Jev and the gateway one small question each, off the main thread's
/// back ([`check::run`] does its requests on the cloud runtime).
fn run_check(rows: &Rc<Rows>, shell: &Shell) {
    if CHECK.with(|c| std::mem::replace(&mut c.borrow_mut().running, true)) {
        return;
    }
    sync(rows, shell);
    let eff = effective(shell);
    let net = brain::Net::new(
        copper_shell_core::cloud::runtime().handle().clone(),
        copper_shell_core::updates::version(),
    );
    let (weak, w) = (shell.downgrade(), Rc::downgrade(rows));
    glib::spawn_future_local(async move {
        let done = check::run(&net, &eff).await;
        CHECK.with(|c| *c.borrow_mut() = done);
        if let (Some(s), Some(rows)) = (weak.upgrade(), w.upgrade()) {
            sync(&rows, &s);
        }
    });
}

/// Bring every row in line with the files and the last check.
fn sync(rows: &Rows, shell: &Shell) {
    let mine = Keys::from_file(&local(shell));
    let eff = effective(shell);
    let cloud_lane = eff.sources.get("lane").map(String::as_str) == Some("cloud");
    let (line, running, router_refused, jev_refused) = CHECK.with(|c| {
        let c = c.borrow();
        (
            c.line(&eff.keys),
            c.running,
            c.refused(&eff.keys, "router"),
            c.refused(&eff.keys, "jev"),
        )
    });

    rows.lane
        .set_visible(mine.lane == Lane::Claude && !cloud_lane);

    rows.key
        .set_subtitle(&elsewhere(shell, &eff, "routerKey").unwrap_or_else(|| {
            "From your gateway — anything that speaks /v1/chat/completions.".into()
        }));
    let key_from_cloud = eff.sources.get("routerKey").map(String::as_str) == Some("cloud");
    let key = rows.key_field.downcast_ref::<gtk::PasswordEntry>();
    if let Some(k) = key {
        k.set_placeholder_text(Some(&if key_from_cloud {
            copper_cloud::intelligence::masked(Some(&eff.keys.router_key))
        } else {
            "sk-…".into()
        }));
    }
    rows.key_field.set_sensitive(!cloud_lane);
    set_dot(&rows.key_dot, eff.router_ready(), router_refused);

    rows.gateway
        .set_subtitle(&elsewhere(shell, &eff, "routerURL").unwrap_or_else(|| {
            if is_web_address(&eff.keys.router_url) {
                "Where the gateway lives".into()
            } else {
                "That isn't a web address — it should start with https://".into()
            }
        }));
    if let Some(g) = rows.gateway_field.downcast_ref::<gtk::Entry>() {
        let from_cloud = eff.sources.get("routerURL").map(String::as_str) == Some("cloud");
        g.set_placeholder_text(Some(if from_cloud || cloud_lane {
            eff.keys.router_url.as_str()
        } else {
            "https://…"
        }));
    }
    rows.gateway_field.set_sensitive(!cloud_lane);

    let (_, title, blurb) = TIERS
        .iter()
        .find(|(t, ..)| *t == eff.keys.tier)
        .copied()
        .unwrap_or(TIERS[1]);
    rows.model.set_subtitle(&format!(
        "{title} — {blurb}. Asks for “{}”.",
        brain::model(&eff.keys, None)
    ));

    rows.check.set_subtitle(
        line.as_deref()
            .unwrap_or("Asks Jev and the model one small question each, so you know they answer."),
    );
    rows.test.set_label(match (running, &line) {
        (true, _) => "Asking…",
        (false, None) => "Test",
        (false, Some(_)) => "Test again",
    });
    rows.test.set_sensitive(!running);

    let speed = "Quick decisions for agents, in about a fifth of a second.";
    rows.jev
        .set_subtitle(&elsewhere(shell, &eff, "jevKey").unwrap_or_else(|| {
            if eff.jev_ready() {
                speed.into()
            } else {
                format!("{speed} Needs a key from your TypeSafe account.")
            }
        }));
    if let Some(j) = rows.jev_field.downcast_ref::<gtk::PasswordEntry>() {
        let from_cloud = eff.sources.get("jevKey").map(String::as_str) == Some("cloud");
        j.set_placeholder_text(Some(&if from_cloud {
            let masked = copper_cloud::intelligence::masked(Some(&eff.keys.jev_key));
            format!("{masked} · type to override")
        } else {
            "ts-…".into()
        }));
    }
    rows.get_key.set_visible(!eff.jev_ready());
    set_dot(&rows.jev_dot, eff.jev_ready(), jev_refused);

    let mut footnote = "Keys stay on this computer, in a file only you can read.".to_string();
    if eff.sources.values().any(|s| s == "cloud") {
        footnote += " Keys from Copper Cloud go when you sign out of it. While it provides a gateway key, that key is used; a Jev key typed here still wins over the cloud's.";
    }
    rows.jev_group.set_description(Some(&footnote));
}
