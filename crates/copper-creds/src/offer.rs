//! When to offer to save a password (Tab.swift `sentSignIn`/`settleSignIn`
//! and Browser.swift `onCredentials`), as pure logic the shell drives.
//!
//! A name and password the page sent are *held*, not offered: whether the
//! sign-in worked is only known afterwards. A page that comes back without a
//! password box took it; one that still has the box refused it. A new
//! document is judged at once; fields the page removed by itself
//! (`officeForms` → `settled`) get 1.5 s first, since an in-page form closes
//! the moment the button is pressed and comes back if the server says no.
//! Anything older than 45 s is dropped.
//!
//! Flow for the shell:
//! 1. `officeForms` `submit` → [`SignInWatch::sent`] (host taken *now*).
//! 2. load finished → [`SignInWatch::settle`]`(navigated = true)`;
//!    `settled` message → `settle(false)`, and on [`Settle::RecheckAfter`]
//!    call [`SignInWatch::recheck`] after the delay.
//! 3. [`Settle::AskPage`] → evaluate
//!    `!!(window.__officeForms && window.__officeForms.hasPassword())` and
//!    pass the answer to [`SignInWatch::page_answered`]; a `Some` is a sign-in
//!    that took.
//! 4. [`decide`] → touch / offer save / offer update / nothing.

use std::time::Duration;

use crate::credential::Source;
use crate::site::is_never;

/// How long a sent sign-in is worth judging.
pub const WINDOW: Duration = Duration::from_secs(45);
/// How long a page that hid its own form is given before judging.
pub const SETTLE_DELAY: Duration = Duration::from_millis(1500);
/// What the shell evaluates in the page for [`Settle::AskPage`].
pub const HAS_PASSWORD_JS: &str = "!!(window.__officeForms && window.__officeForms.hasPassword())";

/// A sign-in the page sent: host (lowercased, no `www.`), user, password,
/// and when (seconds on any monotonic clock the caller keeps).
#[derive(Clone, PartialEq)]
pub struct Sent {
    pub host: String,
    pub user: String,
    pub password: String,
    pub at: f64,
}

impl std::fmt::Debug for Sent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sent")
            .field("host", &self.host)
            .field("user", &self.user)
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

/// What to do next after [`SignInWatch::settle`].
#[derive(Debug, Clone, PartialEq)]
pub enum Settle {
    /// Nothing held (or it went stale and was dropped).
    Nothing,
    /// Call [`SignInWatch::recheck`] with this stamp after the delay.
    RecheckAfter { delay: Duration, stamp: f64 },
    /// Ask the page whether a password box is still there.
    AskPage,
}

/// One tab's held sign-in.
#[derive(Debug, Default, Clone)]
pub struct SignInWatch {
    sent: Option<Sent>,
}

impl SignInWatch {
    pub fn held(&self) -> Option<&Sent> {
        self.sent.as_ref()
    }

    /// The page sent a sign-in from `host` (the URL's host *now*, while the
    /// page is still the sign-in page). No host, nothing held.
    pub fn sent(&mut self, host: Option<&str>, user: &str, password: &str, now: f64) {
        let Some(host) = host.map(str::to_lowercase).filter(|h| !h.is_empty()) else {
            return;
        };
        let bare = host
            .strip_prefix("www.")
            .map(str::to_string)
            .unwrap_or(host);
        self.sent = Some(Sent {
            host: bare,
            user: user.into(),
            password: password.into(),
            at: now,
        });
    }

    /// The page moved on: a new document (`navigated`) or its sign-in
    /// fields went away by themselves.
    pub fn settle(&mut self, navigated: bool, now: f64) -> Settle {
        let Some(sent) = &self.sent else {
            return Settle::Nothing;
        };
        if now - sent.at >= WINDOW.as_secs_f64() {
            self.sent = None;
            return Settle::Nothing;
        }
        if !navigated {
            return Settle::RecheckAfter {
                delay: SETTLE_DELAY,
                stamp: sent.at,
            };
        }
        Settle::AskPage
    }

    /// The delay is over: judge it, unless something newer went out.
    pub fn recheck(&mut self, stamp: f64, now: f64) -> Settle {
        match &self.sent {
            Some(s) if s.at == stamp => self.settle(true, now),
            _ => Settle::Nothing,
        }
    }

    /// The page said whether a password box is still there. `false` (or no
    /// answer) is a sign-in that took: it is handed over and forgotten.
    /// `true` is a refusal or a second step: kept a moment longer.
    pub fn page_answered(&mut self, still_has_password: Option<bool>) -> Option<Sent> {
        if still_has_password == Some(true) {
            return None;
        }
        self.sent.take()
    }

    /// A private tab or a closed one: forget it.
    pub fn clear(&mut self) {
        self.sent = None;
    }
}

/// Where a save goes (`Credentials.saveTarget`): the chosen vault while it
/// can take one, else the Secret Service.
pub fn save_target(
    chosen: Source,
    bitwarden_unlocked: bool,
    one_password_unlocked: bool,
    one_password_has_save_vault: bool,
) -> Source {
    match chosen {
        Source::Bitwarden if bitwarden_unlocked => Source::Bitwarden,
        Source::OnePassword if one_password_unlocked && one_password_has_save_vault => {
            Source::OnePassword
        }
        _ => Source::Keychain,
    }
}

/// Everything [`decide`] needs to know about the world.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    /// `passwords.save` (Settings).
    pub saves_passwords: bool,
    /// A private ("shy") tab never offers.
    pub private_tab: bool,
    /// `passwords.never`.
    pub never: &'a [String],
    /// Where a save would go ([`save_target`]).
    pub target: Source,
    /// Users already kept in `target` for this host.
    pub known_users: &'a [String],
}

/// What to do with a sign-in that took.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Not offered (off, private, empty password, a never-site).
    Ignore,
    /// The same account is kept in `target`: read its one secret, then call
    /// [`after_compare`] — same password → touch, else offer an update.
    Compare,
    /// A new account: offer to save.
    OfferSave,
}

/// Whether and what to offer (`Browser.onCredentials`).
pub fn decide(sent: &Sent, ctx: &Context<'_>) -> Decision {
    if !ctx.saves_passwords
        || sent.password.is_empty()
        || ctx.private_tab
        || is_never(ctx.never.iter().map(String::as_str), &sent.host)
    {
        return Decision::Ignore;
    }
    if ctx.known_users.contains(&sent.user) {
        Decision::Compare
    } else {
        Decision::OfferSave
    }
}

/// The comparison for [`Decision::Compare`]: `Ok(current)` — the kept
/// password; `Err` — reading it failed (the offer is still shown, as new).
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Unchanged: only mark it used.
    Touch,
    /// Offer "Update password?".
    OfferUpdate,
    /// The kept one couldn't be read: offer it as a new save rather than
    /// lose a human's offer.
    OfferSave,
}

pub fn after_compare(sent: &Sent, current: Result<Option<&str>, ()>) -> Outcome {
    match current {
        Ok(Some(p)) if p == sent.password => Outcome::Touch,
        Ok(_) => Outcome::OfferUpdate,
        Err(()) => Outcome::OfferSave,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_judges_at_once() {
        let mut w = SignInWatch::default();
        w.sent(Some("WWW.Example.com"), "ann", "pw", 10.0);
        assert_eq!(w.held().unwrap().host, "example.com");
        assert_eq!(w.settle(true, 11.0), Settle::AskPage);
        assert_eq!(
            w.page_answered(Some(true)),
            None,
            "box still there: refused"
        );
        assert!(w.held().is_some());
        let took = w.page_answered(Some(false)).unwrap();
        assert_eq!((took.user.as_str(), took.password.as_str()), ("ann", "pw"));
        assert!(w.held().is_none());
    }

    #[test]
    fn in_page_forms_wait_and_newer_wins() {
        let mut w = SignInWatch::default();
        w.sent(Some("example.com"), "ann", "pw", 0.0);
        let Settle::RecheckAfter { delay, stamp } = w.settle(false, 0.2) else {
            panic!()
        };
        assert_eq!(delay, SETTLE_DELAY);
        w.sent(Some("example.com"), "ann", "pw2", 0.5);
        assert_eq!(
            w.recheck(stamp, 1.7),
            Settle::Nothing,
            "a newer one went out"
        );
        let Settle::RecheckAfter { stamp, .. } = w.settle(false, 0.6) else {
            panic!()
        };
        assert_eq!(w.recheck(stamp, 2.1), Settle::AskPage);
    }

    #[test]
    fn stale_and_hostless() {
        let mut w = SignInWatch::default();
        w.sent(None, "ann", "pw", 0.0);
        assert!(w.held().is_none());
        w.sent(Some("example.com"), "ann", "pw", 0.0);
        assert_eq!(w.settle(true, 45.0), Settle::Nothing);
        assert!(w.held().is_none());
    }

    #[test]
    fn decisions() {
        let sent = Sent {
            host: "accounts.example.com".into(),
            user: "ann".into(),
            password: "pw".into(),
            at: 0.0,
        };
        let known = vec!["ann".to_string()];
        let never = vec![];
        let mut ctx = Context {
            saves_passwords: true,
            private_tab: false,
            never: &never,
            target: Source::Keychain,
            known_users: &[],
        };
        assert_eq!(decide(&sent, &ctx), Decision::OfferSave);
        ctx.known_users = &known;
        assert_eq!(decide(&sent, &ctx), Decision::Compare);
        assert_eq!(after_compare(&sent, Ok(Some("pw"))), Outcome::Touch);
        assert_eq!(after_compare(&sent, Ok(Some("old"))), Outcome::OfferUpdate);
        assert_eq!(after_compare(&sent, Err(())), Outcome::OfferSave);
        let never = vec!["example.com".to_string()];
        ctx.never = &never;
        assert_eq!(decide(&sent, &ctx), Decision::Ignore);
        ctx.never = &[];
        ctx.private_tab = true;
        assert_eq!(decide(&sent, &ctx), Decision::Ignore);
        assert_eq!(
            save_target(Source::Bitwarden, false, true, true),
            Source::Keychain
        );
        assert_eq!(
            save_target(Source::OnePassword, false, true, false),
            Source::Keychain
        );
        assert_eq!(
            save_target(Source::OnePassword, false, true, true),
            Source::OnePassword
        );
    }
}
