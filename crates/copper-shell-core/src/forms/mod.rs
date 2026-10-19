//! Passwords and form fill, without widgets (lane: creds-ui; PLAN T-015 +
//! T-016 app side).
//!
//! * [`watch`] — every page gets the forms relay (`officeForms`); a sign-in
//!   the page sends is *held* ([`copper_creds::offer::SignInWatch`]) and
//!   offered for saving only once it took. A click into a name or password
//!   box offers the saved accounts in a small list under the box; nothing
//!   is ever filled without a click. The offer and the list are the app's
//!   widgets ([`view::FormsUi`]).
//! * [`vault`](mod@vault) — the platform's secret store (Secret Service, Credential
//!   Manager), Bitwarden (`bw`) and 1Password (`op`), on the lane's own
//!   Tokio runtime ([`rt`]).
//! * [`mcp`] — `browser_sign_in` / `browser_autofill` for the agents lane.
//!
//! The app adds Settings › Passwords, the "Bring in from Chrome" sheet and
//! the widgets (GTK: `copper-app`'s `forms`).

use crate::Platform;
use crate::main_loop::MainLoop;
use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_creds::credential::{AgentHint, Credential, Source};
use copper_creds::onepassword::{Mode, Remembered};
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub mod mcp;
pub mod rt;
pub mod vault;
pub mod view;
pub mod watch;

use vault::Vault;
pub use view::{FormsUi, OfferData, OfferView, PickerView, Row, RowKind};

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
pub struct Forms<U: FormsUi> {
    pub shell: WeakShell<U::Platform>,
    pub vault: Arc<Vault>,
    pub(crate) tabs: RefCell<std::collections::HashMap<TabId, watch::TabForms<U>>>,
    pub(crate) epoch: std::time::Instant,
}

thread_local! {
    /// The `Rc<Forms<U>>` ([`start`]; one UI per process).
    static FORMS: RefCell<Option<Rc<dyn Any>>> = const { RefCell::new(None) };
    static VAULT: RefCell<Option<Arc<Vault>>> = const { RefCell::new(None) };
}

/// The lane's state, once [`start`] ran.
pub fn get<U: FormsUi>() -> Option<Rc<Forms<U>>> {
    FORMS.with(|f| f.borrow().clone()?.downcast::<Forms<U>>().ok())
}

/// The vault handle (the MCP tools use it from the main loop).
pub fn vault() -> Option<Arc<Vault>> {
    VAULT.with(|v| v.borrow().clone())
}

/// Open the vaults and start watching pages. Once, from the app's lane
/// registration. `look` is how the platform's secret store is looked at
/// without opening it ([`vault::Look`]).
pub fn start<U: FormsUi>(shell: &Shell<U::Platform>, look: vault::Look) -> Rc<Forms<U>> {
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
    let vault = Vault::new(&shell.store().paths().data, bw_stay, op_stay, look);
    let forms = Rc::new(Forms {
        shell: shell.downgrade(),
        vault: vault.clone(),
        tabs: RefCell::default(),
        epoch: std::time::Instant::now(),
    });
    FORMS.with(|f| *f.borrow_mut() = Some(forms.clone() as Rc<dyn Any>));
    VAULT.with(|v| *v.borrow_mut() = Some(vault.clone()));

    // What last launch left: an open Bitwarden, 1Password. The keyring is
    // only looked at (no prompt); the first fill, save or Settings visit
    // opens it.
    <U::Platform as Platform>::Loop::spawn_local(async move {
        vault.note_probe(vault.probe(false).await);
        let v = vault.clone();
        rt::run(async move { v.start(remembered).await }).await;
    });

    watch::start(&forms, shell);
    forms
}

// MARK: settings helpers

/// `passwords.never`: sites told to stop offering.
pub fn never_list<P: Platform>(shell: &Shell<P>) -> Vec<String> {
    string_list(shell, keys::NEVER)
}

fn string_list<P: Platform>(shell: &Shell<P>, key: &str) -> Vec<String> {
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

fn set_string_list<P: Platform>(shell: &Shell<P>, key: &str, mut list: Vec<String>) {
    list.sort();
    list.dedup();
    let value = serde_json::Value::Array(list.into_iter().map(Into::into).collect());
    shell.update_settings(|s| s.set(key, value));
}

pub fn set_never<P: Platform>(shell: &Shell<P>, host: &str, never: bool) {
    let mut list = never_list(shell);
    list.retain(|h| h != host);
    if never {
        list.push(host.to_string());
    }
    set_string_list(shell, keys::NEVER, list);
}

/// Where new passwords go (`passwords.backend`).
pub fn chosen_backend<P: Platform>(shell: &Shell<P>) -> Source {
    match shell
        .settings()
        .string(copper_core::settings::keys::PASSWORDS_BACKEND)
    {
        Some("bitwarden") => Source::Bitwarden,
        Some("onepassword") => Source::OnePassword,
        _ => Source::Keychain,
    }
}

pub fn set_backend<P: Platform>(shell: &Shell<P>, source: Source) {
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
pub fn agent_allowed<P: Platform>(shell: &Shell<P>, c: &Credential) -> bool {
    if c.agent_hint == AgentHint::Deny {
        return false;
    }
    shell.settings().bool_or(keys::SHARE_ALL, false)
        || c.agent_hint == AgentHint::Allow
        || string_list(shell, keys::ALLOWED).contains(&c.id.to_string())
}

/// A card / identity / field id (`bw:<id>` / `op:<id>`) shared with agents.
pub fn agent_allowed_id<P: Platform>(shell: &Shell<P>, stable: &str) -> bool {
    shell.settings().bool_or(keys::SHARE_ALL, false)
        || string_list(shell, keys::ALLOWED)
            .iter()
            .any(|a| a == stable)
}

pub fn set_agent_allowed<P: Platform>(shell: &Shell<P>, c: &Credential, on: bool) {
    let id = c.id.to_string();
    let mut list = string_list(shell, keys::ALLOWED);
    list.retain(|a| *a != id);
    if on {
        list.push(id);
    }
    set_string_list(shell, keys::ALLOWED, list);
}

/// Remember how 1Password is connected (never a token).
pub fn remember_op<P: Platform>(shell: &Shell<P>, r: &Remembered) {
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
