//! The Claude account in the shell (Mac: `ClaudeAccount.signIn(in:)`): one
//! [`copper_mcp::claude::Account`] for the process, its requests on the
//! cloud lane's runtime. [`sign_in`] opens claude.ai in a new tab in front;
//! while an attempt is under way the shell closes the tab it opened once
//! the attempt is over (if it still shows claude.ai or the callback) and
//! says who signed in. The Settings pages, the agent pane and `copper
//! claude` all start it here.

use crate::Platform;
use crate::shell::{OpenTab, Shell};
use copper_core::{Store, TabId};
use copper_mcp::claude::{Account, Phase};
use std::cell::Cell;
use std::sync::{Arc, OnceLock};

static ACCOUNT: OnceLock<Arc<Account>> = OnceLock::new();

thread_local! {
    /// A sign-in's tab is being looked after ([`sign_in`]).
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

/// The account (`claude.json` in `store`), made on first use.
pub fn account(store: &Store) -> Arc<Account> {
    ACCOUNT
        .get_or_init(|| Account::new(store.clone(), crate::cloud::runtime().handle().clone()))
        .clone()
}

/// Signed in: the `claude` lane can be asked.
pub fn ready(store: &Store) -> bool {
    account(store).signed_in()
}

/// "Sign in with Claude": claude.ai's page in a new tab in front. `Err`:
/// the loopback couldn't listen (the status says so too).
pub fn sign_in<P: Platform>(shell: &Shell<P>) -> Result<(), String> {
    let account = account(shell.store());
    let url = account.sign_in()?;
    let tab = shell.open(
        &url,
        OpenTab {
            foreground: true,
            ..Default::default()
        },
    );
    account.opened(tab.0);
    if !WATCHING.replace(true) {
        crate::main_loop::spawn::<P::Loop>(watch(shell.downgrade(), account));
    }
    Ok(())
}

/// Until the attempt is over: close the tabs it hands back, and toast
/// "Signed in to Claude as …" when it worked.
async fn watch<P: Platform>(shell: crate::WeakShell<P>, account: Arc<Account>) {
    let mut status = account.watch();
    let session = status.borrow().session;
    loop {
        let changed = status.changed().await;
        let Some(shell) = shell.upgrade() else { break };
        for id in account.take_closing() {
            let id = TabId(id);
            let showing = shell.browser().tab(&id).map(|t| t.url.clone());
            if showing.is_some_and(|url| sign_in_page(&url)) {
                shell.close(&id);
            }
        }
        let now = status.borrow_and_update().clone();
        if now.signed_in && now.session != session {
            shell.toast(&format!("Signed in to Claude as {}", now.who()));
        }
        if changed.is_err() || matches!(now.phase, Phase::Idle | Phase::Failed(_)) {
            break;
        }
    }
    WATCHING.set(false);
}

/// claude.ai and its sign-in pages, or the callback on this computer; a tab
/// that never got an address is still the sign-in's (`isSignInHost`).
pub fn sign_in_page(url: &str) -> bool {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .unwrap_or_default();
    if host.is_empty() || host == "localhost" || host == "127.0.0.1" {
        return true;
    }
    ["claude.ai", "claude.com", "anthropic.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_sign_in_pages_are_closed() {
        use super::sign_in_page;
        assert!(sign_in_page("https://claude.ai/oauth/authorize?x=1"));
        assert!(sign_in_page("https://login.anthropic.com/"));
        assert!(sign_in_page("http://localhost:53692/callback?code=x"));
        assert!(sign_in_page("about:blank"));
        assert!(!sign_in_page("https://news.ycombinator.com/"));
        assert!(!sign_in_page("https://notclaude.ai/"));
    }
}
