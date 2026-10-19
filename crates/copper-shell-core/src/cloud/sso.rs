//! Single sign-on in the shell (Mac: `CloudSSO`): "Continue with Google" on
//! Settings › Cloud's Connect step (a join link, `#sso=`) and Account step
//! (an instance this Copper is linked to already that offers it), and the
//! sign-in a `copper://cloud/join` link starts ([`super::join`]).
//!
//! No toolkit here. A shell — GTK, Win32 — drives one [`SignIn`]
//! ([`sign_in`]) and only draws its [`SsoState`]:
//!
//! * [`SignIn::start`] takes the shell's own way of opening the system
//!   browser (an [`Opener`]: `xdg-open`, `ShellExecuteW`, a test's fake
//!   browser) and answers with an [`SsoRun`]: a future of the outcome and
//!   the [`SsoCancel`] that stops it. The flow itself is copper-cloud's
//!   ([`copper_cloud::sso`]: loopback listener, PKCE, the provider's page,
//!   `POST /v1/auth/sso/{id}`, kept exactly like a pairing).
//! * Once signed in, the app's cloud starts ([`Cloud::start`]) and sync turns
//!   on with the default switches through the window's own engine
//!   ([`super::sync_after_sign_in`]) — never `offline::apply_sync`: the
//!   running window owns the profile.
//! * [`SignIn::watch`] tells the page what to draw: the phase ("Finish
//!   signing in in your browser…" + Cancel), the problem in words, the
//!   certificate fingerprint to compare when the instance's certificate is
//!   one nobody vouches for, and the code a `copper://cloud/join` link
//!   brought ([`JoinOffer`]).
//! * [`SignIn::discover`] asks an instance which sign-ins it offers
//!   (`GET /v1/auth/sso`), for the button's name and the domain hint.

use super::Cloud;
use copper_cloud::sso::{SsoCancel, SsoInfo, SsoOutcome, SsoProvider, discover, provider_title};
use copper_cloud::{Endpoint, Failure, JoinCode, SsoLink};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::watch;

/// Opens the system browser at an address, the shell's own way. An `Err`
/// (no browser, no display) ends the sign-in with the reason in words.
pub type Opener = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Where a sign-in goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SsoTarget {
    /// A join link (`#sso=`): links this Copper to its instance and signs in.
    Join(SsoLink),
    /// "Continue with …" on the instance this Copper is linked to already.
    Linked {
        endpoint: Endpoint,
        provider: String,
    },
}

impl SsoTarget {
    /// The instance.
    pub fn endpoint(&self) -> &Endpoint {
        match self {
            SsoTarget::Join(link) => &link.endpoint,
            SsoTarget::Linked { endpoint, .. } => endpoint,
        }
    }

    /// The provider's id on the instance (`google`).
    pub fn provider(&self) -> &str {
        match self {
            SsoTarget::Join(link) => &link.provider,
            SsoTarget::Linked { provider, .. } => provider,
        }
    }

    /// `host:port` as people read it.
    pub fn host(&self) -> String {
        self.endpoint().host_text()
    }

    /// Whether this sign-in goes to `endpoint`'s instance (any provider).
    pub fn reaches(&self, endpoint: &Endpoint) -> bool {
        same_instance(self.endpoint(), endpoint)
    }
}

/// Two addresses for one instance: the same scheme, host (any case) and
/// port, `:443` the same as none.
pub fn same_instance(a: &Endpoint, b: &Endpoint) -> bool {
    a.https == b.https
        && a.host.eq_ignore_ascii_case(&b.host)
        && a.effective_port() == b.effective_port()
}

/// Where a single sign-on is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SsoPhase {
    #[default]
    Idle,
    /// Reaching the instance, opening the browser.
    Starting,
    /// The browser is open at the provider (its name).
    Waiting(String),
    /// Signed in; sync is being turned on.
    Finishing,
    /// It didn't work: the refusal's code and the words to show.
    Failed { code: String, message: String },
}

impl SsoPhase {
    /// A sign-in is under way.
    pub fn busy(&self) -> bool {
        matches!(
            self,
            SsoPhase::Starting | SsoPhase::Waiting(_) | SsoPhase::Finishing
        )
    }

    /// What the waiting row says while it is.
    pub fn waiting_text(&self) -> Option<&'static str> {
        match self {
            SsoPhase::Starting => Some("Opening your browser…"),
            SsoPhase::Waiting(_) => Some("Finish signing in in your browser…"),
            SsoPhase::Finishing => Some("Signing you in…"),
            SsoPhase::Idle | SsoPhase::Failed { .. } => None,
        }
    }

    /// Cancel is offered (not once the browser has come back).
    pub fn cancellable(&self) -> bool {
        matches!(self, SsoPhase::Starting | SsoPhase::Waiting(_))
    }
}

/// A code a `copper://cloud/join` link brought, for Settings › Cloud: its
/// Connect field takes it, or — linked to another instance — the page says
/// "This link is for HOST" until it is dismissed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinOffer {
    /// Each offer its own, so the same link opened twice is taken twice.
    pub id: u64,
    /// The code as it would be pasted.
    pub text: String,
    pub code: JoinCode,
}

impl JoinOffer {
    /// The instance the code is for.
    pub fn endpoint(&self) -> &Endpoint {
        code_endpoint(&self.code)
    }

    /// `host:port` as people read it.
    pub fn host(&self) -> String {
        self.endpoint().host_text()
    }
}

/// The instance a code is for.
pub fn code_endpoint(code: &JoinCode) -> &Endpoint {
    match code {
        JoinCode::Link(l) => &l.endpoint,
        JoinCode::Pairing(p) => &p.endpoint,
        JoinCode::Sso(s) => &s.endpoint,
    }
}

/// A finished sign-in, for the shell to say so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsoDone {
    pub host: String,
    pub email: String,
    /// The provider's name ("Google").
    pub provider: String,
    /// The instance made the account just now.
    pub created: bool,
}

impl SsoDone {
    /// "Signed in to HOST as EMAIL".
    pub fn text(&self) -> String {
        format!(
            "Signed in to {} as {}{}",
            self.host,
            self.email,
            if self.created { " (new account)" } else { "" }
        )
    }
}

/// Everything a page draws about single sign-on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SsoState {
    pub phase: SsoPhase,
    /// The sign-in under way, or the last one.
    pub target: Option<SsoTarget>,
    /// `untrusted`: the fingerprint (hex) of the certificate the instance
    /// showed — nobody vouches for it and the link has no pin — for a person
    /// to compare and trust ([`SignIn::start`]'s `trusting`).
    pub seen: Option<String>,
    /// What a `copper://cloud/join` link brought.
    pub offer: Option<JoinOffer>,
    /// The last sign-in that worked.
    pub signed_in: Option<SsoDone>,
    /// Bumped whenever an instance's sign-in options are learned.
    pub discovered: u64,
}

impl SsoState {
    /// Whether a sign-in to `endpoint`'s instance is under way.
    pub fn running_for(&self, endpoint: &Endpoint) -> bool {
        self.phase.busy() && self.target.as_ref().is_some_and(|t| t.reaches(endpoint))
    }

    /// The last attempt at `endpoint`'s instance failed: its code and words.
    pub fn failure_for(&self, endpoint: &Endpoint) -> Option<(&str, &str)> {
        match &self.phase {
            SsoPhase::Failed { code, message }
                if self.target.as_ref().is_some_and(|t| t.reaches(endpoint)) =>
            {
                Some((code.as_str(), message.as_str()))
            }
            _ => None,
        }
    }

    /// The problem to show beside `endpoint`'s instance — not the
    /// certificate question, which has a card of its own ([`Self::seen_for`]).
    pub fn problem_for(&self, endpoint: &Endpoint) -> Option<&str> {
        self.failure_for(endpoint)
            .filter(|(code, _)| *code != "untrusted" || self.seen.is_none())
            .map(|(_, message)| message)
    }

    /// The fingerprint `endpoint`'s instance showed, waiting to be compared
    /// and trusted.
    pub fn seen_for(&self, endpoint: &Endpoint) -> Option<&str> {
        match self.failure_for(endpoint) {
            Some(("untrusted", _)) => self.seen.as_deref(),
            _ => None,
        }
    }
}

/// A sign-in under way: its outcome, and how to stop it.
pub struct SsoRun {
    pub cancel: SsoCancel,
    /// Signed in and sync on (`Ok`), or why not. Await it from any executor
    /// (the UI's main loop included); it never blocks.
    pub done: tokio::task::JoinHandle<Result<SsoOutcome, Failure>>,
}

/// "Use your @exowatt.com account" — or "@a.com or @b.com", "@a.com, @b.com
/// or @c.com". `None` with no domains.
pub fn domain_hint(domains: &[String]) -> Option<String> {
    let named: Vec<String> = domains
        .iter()
        .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .map(|d| format!("@{d}"))
        .collect();
    let list = match named.as_slice() {
        [] => return None,
        [one] => one.clone(),
        many => format!(
            "{} or {}",
            many[..many.len() - 1].join(", "),
            many[many.len() - 1]
        ),
    };
    Some(format!("Use your {list} account"))
}

/// The app's single sign-on (one per process).
pub fn sign_in() -> &'static SignIn {
    static ONE: OnceLock<SignIn> = OnceLock::new();
    ONE.get_or_init(SignIn::new)
}

/// One sign-in at a time, what it is doing, and what each instance offers.
pub struct SignIn {
    state: watch::Sender<SsoState>,
    /// The attempt under way and its cancel.
    current: Mutex<Option<(u64, SsoCancel)>>,
    attempts: AtomicU64,
    offers: AtomicU64,
    /// `GET /v1/auth/sso` by instance (`Endpoint::base`): `None` for an
    /// instance from before single sign-on.
    known: Mutex<BTreeMap<String, Option<SsoInfo>>>,
}

impl Default for SignIn {
    fn default() -> Self {
        Self::new()
    }
}

impl SignIn {
    pub fn new() -> Self {
        SignIn {
            state: watch::channel(SsoState::default()).0,
            current: Mutex::new(None),
            attempts: AtomicU64::new(0),
            offers: AtomicU64::new(0),
            known: Mutex::new(BTreeMap::new()),
        }
    }

    /// Now.
    pub fn state(&self) -> SsoState {
        self.state.borrow().clone()
    }

    /// Every change to [`SignIn::state`].
    pub fn watch(&self) -> watch::Receiver<SsoState> {
        self.state.subscribe()
    }

    // MARK: what instances offer

    /// What `endpoint`'s instance said it offers, once asked (`None`: not
    /// asked yet, couldn't be asked, or it predates single sign-on).
    pub fn known(&self, endpoint: &Endpoint) -> Option<SsoInfo> {
        self.known
            .lock()
            .expect("sso known")
            .get(&endpoint.base())
            .cloned()
            .flatten()
    }

    /// The provider `id` as `endpoint`'s instance offers it, once known.
    pub fn provider(&self, endpoint: &Endpoint, id: &str) -> Option<SsoProvider> {
        self.known(endpoint)?.provider(id).cloned()
    }

    /// The first provider `endpoint`'s instance offers, once known — the
    /// Account step's button.
    pub fn first_provider(&self, endpoint: &Endpoint) -> Option<SsoProvider> {
        self.known(endpoint)?.offered().first().cloned()
    }

    /// "Google": the instance's name for provider `id`, or one made from the
    /// id until it has said.
    pub fn provider_name(&self, endpoint: &Endpoint, id: &str) -> String {
        self.provider(endpoint, id)
            .map(|p| p.title())
            .unwrap_or_else(|| provider_title(id))
    }

    /// Ask `endpoint`'s instance which sign-ins it offers (through `pin`, or
    /// a public certificate), once unless `force`. `None` when it couldn't
    /// be asked or offers none.
    pub async fn discover(
        &self,
        endpoint: &Endpoint,
        pin: Option<&str>,
        force: bool,
    ) -> Option<SsoInfo> {
        let key = endpoint.base();
        if !force && let Some(known) = self.known.lock().expect("sso known").get(&key) {
            return known.clone();
        }
        let answer = match discover(endpoint, pin).await {
            Ok(answer) => answer,
            Err(f) => {
                tracing::debug!(host = %endpoint.host_text(), code = %f.code, "sign-in options not read");
                return self.known(endpoint);
            }
        };
        let changed = {
            let mut known = self.known.lock().expect("sso known");
            let before = known.insert(key, answer.clone());
            before != Some(answer.clone())
        };
        if changed {
            self.state.send_modify(|s| s.discovered += 1);
        }
        answer
    }

    // MARK: join links

    /// Keep the code a `copper://cloud/join` link brought, for the page.
    pub fn offer(&self, text: &str, code: JoinCode) -> JoinOffer {
        let offer = JoinOffer {
            id: self.offers.fetch_add(1, Ordering::SeqCst) + 1,
            text: text.trim().to_string(),
            code,
        };
        let kept = offer.clone();
        self.state.send_modify(|s| s.offer = Some(kept));
        offer
    }

    /// "Dismiss".
    pub fn dismiss_offer(&self) {
        self.state.send_if_modified(|s| s.offer.take().is_some());
    }

    // MARK: the flow

    /// Sign in to `target` through its single sign-on, on the cloud's
    /// runtime; a sign-in already under way is cancelled. `trusting` is a
    /// fingerprint a person compared and accepted for an instance with no
    /// pin ([`SsoState::seen`]); `open` opens the system browser. Once signed
    /// in, `cloud` starts and sync turns on as after pairing.
    pub fn start(
        &'static self,
        cloud: &'static Cloud,
        target: SsoTarget,
        trusting: Option<String>,
        open: Opener,
    ) -> SsoRun {
        let cancel = SsoCancel::new();
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((_, old)) = self
            .current
            .lock()
            .expect("sso current")
            .replace((attempt, cancel.clone()))
        {
            old.cancel();
        }
        self.state.send_modify(|s| {
            s.phase = SsoPhase::Starting;
            s.target = Some(target.clone());
            s.seen = None;
        });
        tracing::info!(host = %target.host(), provider = %target.provider(), "single sign-on started");
        let stop = cancel.clone();
        let done = super::spawn(async move {
            let result = self
                .run(attempt, cloud, &target, trusting.as_deref(), open, &stop)
                .await;
            self.finish(attempt, &target, &result);
            result
        });
        SsoRun { cancel, done }
    }

    async fn run(
        &'static self,
        attempt: u64,
        cloud: &'static Cloud,
        target: &SsoTarget,
        trusting: Option<&str>,
        open: Opener,
        cancel: &SsoCancel,
    ) -> Result<SsoOutcome, Failure> {
        // What the instance calls it, for "Continue with …" and the row.
        let pin = match target {
            SsoTarget::Join(link) => link
                .fingerprint
                .clone()
                .or_else(|| trusting.map(str::to_string)),
            SsoTarget::Linked { .. } => cloud.client.link().and_then(|l| l.fingerprint),
        };
        let endpoint = target.endpoint().clone();
        let _ = self.discover(&endpoint, pin.as_deref(), false).await;
        let name = self.provider_name(&endpoint, target.provider());
        let opener = move |url: &str| {
            self.set_phase(attempt, SsoPhase::Waiting(name));
            open(url)
        };
        let outcome = match target {
            SsoTarget::Join(link) => {
                cloud
                    .client
                    .sign_in_with_sso(link, trusting, opener, cancel)
                    .await
            }
            SsoTarget::Linked { provider, .. } => {
                if !cloud
                    .client
                    .link()
                    .is_some_and(|l| same_instance(&l.endpoint, &endpoint))
                {
                    return Err(Failure::new(
                        0,
                        "sso_moved",
                        "Copper is connected to another instance now — open Settings › Cloud again",
                    ));
                }
                cloud
                    .client
                    .sign_in_with_sso_here(provider, opener, cancel)
                    .await
            }
        }?;
        self.set_phase(attempt, SsoPhase::Finishing);
        cloud.start();
        super::sync_after_sign_in(cloud).await;
        Ok(outcome)
    }

    fn is_current(&self, attempt: u64) -> bool {
        matches!(*self.current.lock().expect("sso current"), Some((a, _)) if a == attempt)
    }

    fn set_phase(&self, attempt: u64, phase: SsoPhase) {
        if self.is_current(attempt) {
            self.state.send_modify(|s| s.phase = phase);
        }
    }

    fn finish(&self, attempt: u64, target: &SsoTarget, result: &Result<SsoOutcome, Failure>) {
        {
            let mut current = self.current.lock().expect("sso current");
            if !matches!(*current, Some((a, _)) if a == attempt) {
                // Cancelled, or another sign-in owns the page now.
                return;
            }
            *current = None;
        }
        let name = self.provider_name(target.endpoint(), target.provider());
        self.state.send_modify(|s| match result {
            Ok(outcome) => {
                tracing::info!(host = %target.host(), "single sign-on done");
                s.phase = SsoPhase::Idle;
                s.seen = None;
                s.signed_in = Some(SsoDone {
                    host: target.host(),
                    email: outcome.account.email.clone(),
                    provider: name,
                    created: outcome.created,
                });
                if s.offer
                    .as_ref()
                    .is_some_and(|o| target.reaches(o.endpoint()))
                {
                    s.offer = None;
                }
            }
            Err(f) if f.code == "sso_cancelled" => {
                s.phase = SsoPhase::Idle;
                s.seen = None;
            }
            Err(f) => {
                tracing::info!(host = %target.host(), code = %f.code, "single sign-on failed");
                s.seen = (f.code == "untrusted").then(|| f.seen.clone()).flatten();
                s.phase = SsoPhase::Failed {
                    code: f.code.clone(),
                    message: f.message.clone(),
                };
            }
        });
    }

    /// "Cancel": stop waiting for the browser (the listener closes, nothing
    /// is sent). Nothing once the browser has come back.
    pub fn cancel(&self) {
        if !self.state.borrow().phase.cancellable() {
            return;
        }
        if let Some((_, cancel)) = self.current.lock().expect("sso current").take() {
            cancel.cancel();
        }
        self.state.send_modify(|s| {
            s.phase = SsoPhase::Idle;
            s.seen = None;
        });
        tracing::info!("single sign-on cancelled");
    }

    /// Forget the last failure (the code was edited, the page moved on).
    pub fn clear_problem(&self) {
        self.state.send_if_modified(|s| {
            if matches!(s.phase, SsoPhase::Failed { .. }) || s.seen.is_some() {
                if !s.phase.busy() {
                    s.phase = SsoPhase::Idle;
                }
                s.seen = None;
                true
            } else {
                false
            }
        });
    }
}

#[cfg(test)]
mod tests;
