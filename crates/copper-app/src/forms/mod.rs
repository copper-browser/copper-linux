//! Passwords, form fill and moving in from Chrome (lane: creds-ui; PLAN
//! T-015 + T-016 app side).
//!
//! * [`watch`] — every page gets the forms relay (`officeForms`); a sign-in
//!   the page sends is *held* ([`copper_creds::offer::SignInWatch`]) and
//!   offered for saving only once it took ([`offer`]: a quiet capsule over
//!   the top of the page). A click into a name or password box offers the
//!   saved accounts in a small list under the box ([`picker`]); nothing is
//!   ever filled without a click.
//! * [`vault`] — the Secret Service, Bitwarden (`bw`) and 1Password (`op`),
//!   on the lane's own Tokio runtime ([`rt`]).
//! * [`settings`] — Settings › Passwords.
//! * [`import`] — the "Bring in from Chrome, Chromium or Brave" sheet.
//! * [`mcp`] — `browser_sign_in` / `browser_autofill` for the agents lane.

use crate::actions::{Command, SettingsPage, register_command, register_settings_page};
use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_creds::credential::{AgentHint, Credential, Source};
use copper_creds::onepassword::{Mode, Remembered};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub mod import;
pub mod mcp;
pub mod offer;
pub mod picker;
pub mod rt;
#[cfg(debug_assertions)]
pub mod selftest;
pub mod settings;
pub mod vault;
pub mod watch;

use vault::Vault;

/// Settings keys the Mac uses for the same things.
pub mod keys {
    pub const NEVER: &str = "passwords.never";
    pub const SHARE_ALL: &str = "agent.credentials.shareAll";
    pub const ALLOWED: &str = "agent.credentials.allowed";
    pub const BW_STAY: &str = "bitwarden.stayUnlocked";
    pub const BW_SERVER: &str = "bitwarden.server";
    pub const OP_STAY: &str = "onepassword.stayUnlocked";
    pub const OP_MODE: &str = "onepassword.mode";
    pub const OP_ACCOUNT: &str = "onepassword.account";
    pub const OP_EMAIL: &str = "onepassword.email";
    pub const OP_URL: &str = "onepassword.url";
}

/// The lane's main-thread state.
pub struct Forms {
    pub shell: WeakShell,
    pub vault: Arc<Vault>,
    pub(crate) tabs: RefCell<std::collections::HashMap<TabId, watch::TabForms>>,
    pub(crate) epoch: std::time::Instant,
}

thread_local! {
    static FORMS: RefCell<Option<Rc<Forms>>> = const { RefCell::new(None) };
}

/// The lane's state, once [`register`] ran.
pub fn forms() -> Option<Rc<Forms>> {
    FORMS.with(|f| f.borrow().clone())
}

/// The vault handle (the MCP tools use it from the main loop).
pub fn vault() -> Option<Arc<Vault>> {
    forms().map(|f| f.vault.clone())
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
    let (bw_stay, op_stay, remembered) = {
        let s = shell.settings();
        let rem = Remembered {
            mode: s.string(keys::OP_MODE).and_then(Mode::parse),
            account: s.string(keys::OP_ACCOUNT).map(str::to_string),
            email: s.string(keys::OP_EMAIL).map(str::to_string),
            url: s.string(keys::OP_URL).map(str::to_string),
        };
        (
            s.bool_or(keys::BW_STAY, false),
            s.bool_or(keys::OP_STAY, false),
            rem,
        )
    };
    let vault = Vault::new(&shell.store().paths().data, bw_stay, op_stay);
    let forms = Rc::new(Forms {
        shell: shell.downgrade(),
        vault: vault.clone(),
        tabs: RefCell::default(),
        epoch: std::time::Instant::now(),
    });
    FORMS.with(|f| *f.borrow_mut() = Some(forms.clone()));

    let css = gtk4::CssProvider::new();
    css.load_from_string(CSS);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    // What last launch left: the keyring, an open Bitwarden, 1Password.
    glib::spawn_future_local(async move {
        let v = vault.clone();
        rt::run(async move { v.start(remembered).await }).await;
    });

    watch::start(&forms, shell);

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

// MARK: settings helpers

/// `passwords.never`: sites told to stop offering.
pub fn never_list(shell: &Shell) -> Vec<String> {
    string_list(shell, keys::NEVER)
}

fn string_list(shell: &Shell, key: &str) -> Vec<String> {
    shell
        .settings()
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn set_string_list(shell: &Shell, key: &str, mut list: Vec<String>) {
    list.sort();
    list.dedup();
    let value = serde_json::Value::Array(list.into_iter().map(Into::into).collect());
    shell.update_settings(|s| s.set(key, value));
}

pub fn set_never(shell: &Shell, host: &str, never: bool) {
    let mut list = never_list(shell);
    list.retain(|h| h != host);
    if never {
        list.push(host.to_string());
    }
    set_string_list(shell, keys::NEVER, list);
}

/// Where new passwords go (`passwords.backend`).
pub fn chosen_backend(shell: &Shell) -> Source {
    match shell
        .settings()
        .string(copper_core::settings::keys::PASSWORDS_BACKEND)
    {
        Some("bitwarden") => Source::Bitwarden,
        Some("onepassword") => Source::OnePassword,
        _ => Source::Keychain,
    }
}

pub fn set_backend(shell: &Shell, source: Source) {
    let v = match source {
        Source::Keychain => "keychain",
        Source::Bitwarden => "bitwarden",
        Source::OnePassword => "onepassword",
    };
    shell.update_settings(|s| s.set(copper_core::settings::keys::PASSWORDS_BACKEND, v));
}

/// How a person reads where a credential lives.
pub fn source_title(s: Source) -> &'static str {
    match s {
        Source::Keychain => "Keyring",
        Source::Bitwarden => "Bitwarden",
        Source::OnePassword => "1Password",
    }
}

/// Agent access (`AgentAccess.swift`): nothing is shared unless the user
/// shared it, shared everything, or put it in an `Agents` folder.
pub fn agent_allowed(shell: &Shell, c: &Credential) -> bool {
    if c.agent_hint == AgentHint::Deny {
        return false;
    }
    shell.settings().bool_or(keys::SHARE_ALL, false)
        || c.agent_hint == AgentHint::Allow
        || string_list(shell, keys::ALLOWED).contains(&c.id.to_string())
}

/// A card / identity / field id (`bw:<id>` / `op:<id>`) shared with agents.
pub fn agent_allowed_id(shell: &Shell, stable: &str) -> bool {
    shell.settings().bool_or(keys::SHARE_ALL, false)
        || string_list(shell, keys::ALLOWED)
            .iter()
            .any(|a| a == stable)
}

pub fn set_agent_allowed(shell: &Shell, c: &Credential, on: bool) {
    let id = c.id.to_string();
    let mut list = string_list(shell, keys::ALLOWED);
    list.retain(|a| *a != id);
    if on {
        list.push(id);
    }
    set_string_list(shell, keys::ALLOWED, list);
}

/// Remember how 1Password is connected (never a token).
pub fn remember_op(shell: &Shell, r: &Remembered) {
    let r = r.clone();
    shell.update_settings(move |s| {
        match r.mode {
            Some(m) => s.set(keys::OP_MODE, m.as_str()),
            None => s.remove(keys::OP_MODE),
        }
        for (k, v) in [
            (keys::OP_ACCOUNT, &r.account),
            (keys::OP_EMAIL, &r.email),
            (keys::OP_URL, &r.url),
        ] {
            match v {
                Some(v) => s.set(k, v.as_str()),
                None => s.remove(k),
            }
        }
    });
}

/// The site a page is on, as passwords are keyed: lowercased, no `www.`.
pub fn site_host(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    let h = u.host_str()?.to_lowercase();
    let h = h.strip_prefix("www.").map(str::to_string).unwrap_or(h);
    (!h.is_empty()).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::site_host;

    #[test]
    fn hosts() {
        assert_eq!(
            site_host("https://WWW.Example.com/login").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            site_host("http://localhost:8765/login.html").as_deref(),
            Some("localhost")
        );
        assert_eq!(site_host("about:blank"), None);
        assert_eq!(site_host("file:///tmp/x.html"), None);
    }
}
