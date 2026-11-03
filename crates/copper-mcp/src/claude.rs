//! The Claude account (Mac: `Fork/ClaudeAccount.swift`): "Sign in with
//! Claude", and the account as Copper's model lane ([`messages`], Mac
//! `Fork/Claude.swift`). No toolkit here; each shell draws [`Status`] and
//! opens one tab.
//!
//! * **Sign in** ([`Account::sign_in`]) is claude.ai's OAuth with PKCE S256,
//!   the native-app loopback flow: a listener on `127.0.0.1:0` (tokio), and
//!   the authorize address for the shell to open in a new tab in front.
//!   claude.ai comes back to `http://localhost:<port>/callback` with `code`
//!   and `state` (the verifier, as the Mac sends it). A tab that can't come
//!   back shows a code to paste instead ([`Account::paste`]), exchanged with
//!   the attempt's address (or `http://localhost:53692/callback`). Ten
//!   minutes without either and the attempt fails.
//! * The code is exchanged at `platform.claude.com/v1/oauth/token`, the
//!   profile names the account, the tokens go in `claude.json` (0600, the
//!   store's secret file) and the lane in `intelligence.json` becomes
//!   `claude`. The tab the attempt opened is handed back to be closed
//!   ([`Account::take_closing`]) once it is over: signed in, cancelled,
//!   timed out or refused. A failed attempt keeps its verifier, so the
//!   code claude.ai showed can still be pasted.
//! * [`Account::token`] refreshes five minutes before expiry, one refresh
//!   at a time however many ask; a refresh turned away (400/401) signs out.
//! * [`Account::ask`] and [`Account::complete`] are the Messages API on the
//!   account, asked once more with a fresh token after a 401.
//!
//! Every network call runs on the runtime the account was made with, so a
//! UI thread may await them. Never logged: the code, the verifier, a token.

pub mod messages;
#[cfg(test)]
mod tests;

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use copper_cloud::sso::{pkce_pair, read_head, respond};
use copper_core::date::now_unix;
use copper_core::intelligence::{ClaudeCredential, IntelligenceKeys};
use copper_core::store::backup_path;
use copper_core::{DataFile, IsoDate, Store, SwiftDate};
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::AbortHandle;

pub use messages::{Failure, Marks};

pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
pub const SCOPES: &str = "org:create_api_key user:profile user:inference";
const CALLBACK_PATH: &str = "/callback";
/// Where a pasted code is exchanged when no attempt is waiting.
pub const FALLBACK_REDIRECT: &str = "http://localhost:53692/callback";
/// How long the tab has to come back.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);
/// A token this close to its end (seconds) is refreshed first.
const REFRESH_AHEAD: f64 = 300.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one loopback connection has to send its request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const NOT_SIGNED_IN: &str = "Not signed in to Claude — Settings › Intelligence › Model access";
const NO_LISTENER: &str = "Couldn't open the Claude sign-in listener";

/// Where the account's requests go (the tests point them at a mock).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub profile: String,
    pub messages: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            authorize: AUTHORIZE_URL.into(),
            token: TOKEN_URL.into(),
            profile: PROFILE_URL.into(),
            messages: messages::ENDPOINT.into(),
        }
    }
}

/// Where a sign-in is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    /// The claude.ai tab is open; waiting for it to come back (or a paste).
    Waiting,
    /// Trading the code for tokens.
    Exchanging,
    /// It didn't work, in words.
    Failed(String),
}

impl Phase {
    /// `idle`, `waiting`, `exchanging` or `failed`.
    pub fn name(&self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Waiting => "waiting",
            Phase::Exchanging => "exchanging",
            Phase::Failed(_) => "failed",
        }
    }

    /// What went wrong, when something did.
    pub fn trouble(&self) -> &str {
        match self {
            Phase::Failed(text) => text,
            _ => "",
        }
    }
}

/// What a page draws about the account. Never a token.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub signed_in: bool,
    pub email: String,
    pub name: String,
    pub organization: String,
    /// When the access token runs out, Unix seconds (0 signed out).
    pub expires: f64,
    pub phase: Phase,
    /// Counts sign-ins and sign-outs (not refreshes), so what was learnt
    /// about one sign-in — a check's verdict — can tell it is past.
    pub session: u64,
}

impl Status {
    /// `Name · email`, the email alone, or `Signed in`.
    pub fn who(&self) -> String {
        match (self.name.is_empty(), self.email.is_empty()) {
            (false, false) => format!("{} · {}", self.name, self.email),
            (_, false) => self.email.clone(),
            _ => "Signed in".into(),
        }
    }

    /// `copper claude status` (`ClaudeAccount.status`).
    pub fn json(&self) -> Value {
        let expires = if self.signed_in {
            IsoDate(self.expires as i64).to_string()
        } else {
            String::new()
        };
        json!({
            "signedIn": self.signed_in,
            "email": self.email,
            "name": self.name,
            "organization": self.organization,
            "expires": expires,
            "phase": self.phase.name(),
            "trouble": self.phase.trouble(),
        })
    }
}

/// One attempt's loose ends.
#[derive(Default)]
struct Flow {
    verifier: Option<String>,
    redirect: Option<String>,
    /// The callback (or a paste) was taken; later ones are not.
    handled: bool,
    /// The tab the shell opened for it.
    tab: Option<String>,
    listener: Option<AbortHandle>,
    timeout: Option<AbortHandle>,
    exchange: Option<AbortHandle>,
}

impl Flow {
    fn stop(slot: &mut Option<AbortHandle>) {
        if let Some(task) = slot.take() {
            task.abort();
        }
    }
}

#[derive(Default)]
struct Inner {
    credential: Option<ClaudeCredential>,
    phase: Phase,
    session: u64,
    /// The attempt under way (or the last one); its tasks carry the number.
    attempt: u64,
    flow: Flow,
    /// Tabs the shell should close (when they still show claude.ai).
    closing: Vec<String>,
}

/// The Claude account: one per process (the shell keeps it).
pub struct Account {
    store: Store,
    endpoints: Endpoints,
    runtime: tokio::runtime::Handle,
    http: reqwest::Client,
    inner: Mutex<Inner>,
    status: watch::Sender<Status>,
    /// One refresh at a time.
    refreshing: tokio::sync::Mutex<()>,
}

impl Account {
    /// The account in `store`'s `claude.json`, its requests on `runtime`.
    pub fn new(store: Store, runtime: tokio::runtime::Handle) -> Arc<Account> {
        Self::with_endpoints(store, runtime, Endpoints::default())
    }

    /// [`Account::new`] with other addresses.
    pub fn with_endpoints(
        store: Store,
        runtime: tokio::runtime::Handle,
        endpoints: Endpoints,
    ) -> Arc<Account> {
        let credential = match store.load_opt::<ClaudeCredential>() {
            Ok(loaded) => loaded.value,
            Err(e) => {
                tracing::warn!(target: "copper::claude", "claude.json: {e}");
                None
            }
        };
        let inner = Inner {
            credential,
            ..Inner::default()
        };
        let status = watch::channel(snapshot(&inner)).0;
        Arc::new(Account {
            store,
            endpoints,
            runtime,
            http: reqwest::Client::new(),
            inner: Mutex::new(inner),
            status,
            refreshing: tokio::sync::Mutex::new(()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn publish(&self, inner: &Inner) {
        self.status.send_replace(snapshot(inner));
    }

    /// Now.
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Every change to [`Account::status`], and every tab to close.
    pub fn watch(&self) -> watch::Receiver<Status> {
        self.status.subscribe()
    }

    /// Signed in: the `claude` lane can be asked.
    pub fn signed_in(&self) -> bool {
        self.lock().credential.is_some()
    }

    /// Tabs whose sign-in is over, for the shell to close if they still
    /// show claude.ai or the callback (`closeSignInTabIfAppropriate`).
    pub fn take_closing(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().closing)
    }

    // MARK: - the sign-in

    /// Start a sign-in (one under way is cancelled first): the loopback
    /// listens, and the answer is the claude.ai address to open in a new
    /// tab in front, then name with [`Account::opened`]. `Err` (also the
    /// status): no listener.
    pub fn sign_in(self: &Arc<Self>) -> Result<String, String> {
        let mut inner = self.lock();
        if matches!(inner.phase, Phase::Waiting | Phase::Exchanging) {
            self.cancel_locked(&mut inner);
        }
        inner.attempt += 1;
        inner.flow = Flow::default();
        let bound = pkce_pair().ok().and_then(|pair| {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
            listener.set_nonblocking(true).ok()?;
            let port = listener.local_addr().ok()?.port();
            Some((pair, listener, port))
        });
        let Some(((verifier, challenge), listener, port)) = bound else {
            set_phase(&mut inner, Phase::Failed(NO_LISTENER.into()));
            self.publish(&inner);
            return Err(NO_LISTENER.into());
        };
        let redirect = format!("http://localhost:{port}{CALLBACK_PATH}");
        let Ok(mut url) = url::Url::parse(&self.endpoints.authorize) else {
            set_phase(
                &mut inner,
                Phase::Failed("Couldn't build the Claude sign-in URL".into()),
            );
            self.publish(&inner);
            return Err("Couldn't build the Claude sign-in URL".into());
        };
        url.query_pairs_mut()
            .append_pair("code", "true")
            .append_pair("client_id", CLIENT_ID)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect)
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &verifier);
        let attempt = inner.attempt;
        inner.flow.verifier = Some(verifier);
        inner.flow.redirect = Some(redirect);
        let me = self.clone();
        inner.flow.listener = Some(
            self.runtime
                .spawn(me.listen(attempt, listener))
                .abort_handle(),
        );
        let me = self.clone();
        let timeout = self.runtime.spawn(async move {
            tokio::time::sleep(SIGN_IN_TIMEOUT).await;
            me.timed_out(attempt);
        });
        inner.flow.timeout = Some(timeout.abort_handle());
        set_phase(&mut inner, Phase::Waiting);
        self.publish(&inner);
        Ok(url.to_string())
    }

    /// The tab the shell opened for the sign-in under way.
    pub fn opened(&self, tab: String) {
        let mut inner = self.lock();
        if inner.phase == Phase::Waiting {
            inner.flow.tab = Some(tab);
        }
    }

    /// Finish with what claude.ai showed: the callback address, its query,
    /// or `code#state` (`ClaudeAccount.complete(pasted:)`).
    pub fn paste(self: &Arc<Self>, pasted: &str) {
        let mut inner = self.lock();
        let (code, state) = parse_redirect(pasted);
        let Some(code) = code.filter(|c| !c.is_empty()) else {
            self.fail_locked(&mut inner, "That does not contain a Claude sign-in code");
            return;
        };
        let verifier = inner.flow.verifier.clone();
        if let Some(state) = &state
            && verifier.as_ref() != Some(state)
        {
            self.fail_locked(&mut inner, "That code is from a different sign-in attempt");
            return;
        }
        let redirect = inner
            .flow
            .redirect
            .clone()
            .unwrap_or_else(|| FALLBACK_REDIRECT.into());
        inner.flow.handled = true;
        Flow::stop(&mut inner.flow.listener);
        let state = state.or(verifier).unwrap_or_default();
        self.begin_exchange(&mut inner, code, state, redirect);
    }

    /// Give up the sign-in under way; its tab goes too.
    pub fn cancel(&self) {
        let mut inner = self.lock();
        self.cancel_locked(&mut inner);
        self.publish(&inner);
    }

    /// Forget the account (`claude.json` and its backup).
    pub fn sign_out(&self) {
        let mut inner = self.lock();
        self.cancel_locked(&mut inner);
        if inner.credential.take().is_some() {
            inner.session += 1;
            tracing::info!(target: "copper::claude", "signed out of Claude");
        }
        let file = self.store.path(DataFile::Claude);
        for path in [backup_path(&file), file] {
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(target: "copper::claude", "{}: {e}", path.display());
            }
        }
        self.publish(&inner);
    }

    fn cancel_locked(&self, inner: &mut Inner) {
        Flow::stop(&mut inner.flow.listener);
        Flow::stop(&mut inner.flow.exchange);
        end_flow(inner);
        set_phase(inner, Phase::Idle);
    }

    /// The attempt failed, but its verifier and address stay for a paste;
    /// its tab is the person's now.
    fn fail_locked(&self, inner: &mut Inner, text: &str) {
        Flow::stop(&mut inner.flow.timeout);
        Flow::stop(&mut inner.flow.listener);
        inner.flow.tab = None;
        inner.flow.handled = false;
        set_phase(inner, Phase::Failed(text.into()));
        self.publish(inner);
    }

    fn timed_out(&self, attempt: u64) {
        let mut inner = self.lock();
        if inner.attempt == attempt && inner.phase == Phase::Waiting {
            // The code that tab shows has run out too.
            if let Some(tab) = inner.flow.tab.take() {
                inner.closing.push(tab);
            }
            self.fail_locked(&mut inner, "Sign-in timed out — try again");
        }
    }

    /// The loopback: each connection's request read on a task of its own
    /// (a browser opens connections it never uses).
    async fn listen(self: Arc<Self>, attempt: u64, listener: std::net::TcpListener) {
        let listener = match TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(target: "copper::claude", "sign-in listener: {e}");
                let mut inner = self.lock();
                if inner.attempt == attempt {
                    self.fail_locked(&mut inner, NO_LISTENER);
                }
                return;
            }
        };
        loop {
            let Ok((mut stream, peer)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let me = self.clone();
            tokio::spawn(async move {
                let Ok(Some(head)) =
                    tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await
                else {
                    return;
                };
                let (status, html) = me.callback(attempt, &head);
                respond(&mut stream, status, &html).await;
            });
        }
    }

    /// One request to the loopback: the callback, or nothing.
    fn callback(self: &Arc<Self>, attempt: u64, head: &str) -> (u16, String) {
        let mut parts = head.lines().next().unwrap_or("").split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mut inner = self.lock();
        if method != "GET"
            || path != CALLBACK_PATH
            || inner.flow.handled
            || inner.attempt != attempt
            || inner.phase != Phase::Waiting
        {
            return (404, String::new());
        }
        let (code, state) = code_and_state(query);
        let expected = (inner.flow.verifier.clone(), inner.flow.redirect.clone());
        match (code.filter(|c| !c.is_empty()), state, expected) {
            (Some(code), Some(state), (Some(verifier), Some(redirect))) if state == verifier => {
                inner.flow.handled = true;
                Flow::stop(&mut inner.flow.listener);
                self.begin_exchange(&mut inner, code, state, redirect);
                (
                    200,
                    page(
                        "Copper is signed in with your Claude account.",
                        "You can close this tab.",
                    ),
                )
            }
            _ => {
                self.fail_locked(&mut inner, "The Claude sign-in callback was not valid");
                (
                    400,
                    page(
                        "Copper could not sign in.",
                        "The callback was not valid. You can close this tab.",
                    ),
                )
            }
        }
    }

    fn begin_exchange(
        self: &Arc<Self>,
        inner: &mut Inner,
        code: String,
        state: String,
        redirect: String,
    ) {
        Flow::stop(&mut inner.flow.timeout);
        Flow::stop(&mut inner.flow.exchange);
        set_phase(inner, Phase::Exchanging);
        let verifier = inner.flow.verifier.clone().unwrap_or_else(|| state.clone());
        let attempt = inner.attempt;
        let me = self.clone();
        let task = self.runtime.spawn(async move {
            let got = me.exchange(&code, &state, &redirect, &verifier).await;
            me.exchanged(attempt, got);
        });
        inner.flow.exchange = Some(task.abort_handle());
        self.publish(inner);
    }

    fn exchanged(&self, attempt: u64, got: Result<ClaudeCredential, String>) {
        let mut inner = self.lock();
        if inner.attempt != attempt || inner.phase != Phase::Exchanging {
            // Cancelled, or another sign-in owns the account now.
            return;
        }
        inner.flow.exchange = None;
        // Either way the attempt is spent, and so is its tab.
        end_flow(&mut inner);
        match got {
            Ok(credential) => {
                self.save(&credential);
                self.use_claude_lane();
                inner.credential = Some(credential);
                inner.session += 1;
                tracing::info!(target: "copper::claude", "signed in to Claude");
                set_phase(&mut inner, Phase::Idle);
            }
            Err(text) => set_phase(&mut inner, Phase::Failed(text)),
        }
        self.publish(&inner);
    }

    /// The code for tokens, and the profile for who it is.
    async fn exchange(
        &self,
        code: &str,
        state: &str,
        redirect: &str,
        verifier: &str,
    ) -> Result<ClaudeCredential, String> {
        let body = json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": redirect,
            "code_verifier": verifier,
        });
        let (status, data) = self
            .post_token(&body)
            .await
            .map_err(|e| format!("Claude sign-in request failed: {e}"))?;
        if !(200..300).contains(&status) {
            return Err(http_failure("Claude sign-in", status, &data));
        }
        let (access, refresh, expires_in) =
            parse_token(&data).ok_or("Claude sign-in returned an incomplete token")?;
        let mut credential = ClaudeCredential {
            expires: SwiftDate::from_unix(now_unix() + expires_in),
            ..ClaudeCredential::default()
        };
        if let Some((email, name, organization)) = self.profile(&access).await {
            credential.email = Some(email);
            credential.name = Some(name);
            credential.organization = Some(organization);
        }
        credential.access = access;
        credential.refresh = refresh;
        Ok(credential)
    }

    async fn post_token(&self, body: &Value) -> Result<(u16, Vec<u8>), String> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .timeout(REQUEST_TIMEOUT)
            .json(body)
            .send()
            .await
            .map_err(|e| words(&e))?;
        let status = response.status().as_u16();
        let data = response.bytes().await.map_err(|e| words(&e))?;
        Ok((status, data.to_vec()))
    }

    /// `(email, name, organization)`, or `None` when it can't be read.
    async fn profile(&self, access: &str) -> Option<(String, String, String)> {
        let response = self
            .http
            .get(&self.endpoints.profile)
            .timeout(REQUEST_TIMEOUT)
            .header("authorization", format!("Bearer {access}"))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("user-agent", messages::USER_AGENT)
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let payload: Value = response.json().await.ok()?;
        let account = payload.get("account")?.as_object()?;
        let text = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
        let display = text(account.get("display_name"));
        let name = if display.is_empty() {
            text(account.get("full_name"))
        } else {
            display
        };
        Some((
            text(account.get("email")),
            name,
            text(payload["organization"].get("name")),
        ))
    }

    fn save(&self, credential: &ClaudeCredential) {
        if let Err(e) = self.store.save(credential) {
            tracing::warn!(target: "copper::claude", "claude.json: {e}");
        }
    }

    /// A sign-in chooses the account's lane (`Intelligence.keys.lane`).
    fn use_claude_lane(&self) {
        // An unreadable file is left alone rather than written over.
        let Ok(loaded) = self.store.load::<IntelligenceKeys>() else {
            return;
        };
        let mut keys = loaded.value;
        if keys.lane.as_deref() != Some("claude") {
            keys.lane = Some("claude".into());
            if let Err(e) = self.store.save(&keys) {
                tracing::warn!(target: "copper::claude", "intelligence.json: {e}");
            }
        }
    }

    // MARK: - tokens

    /// An access token good for five minutes more, refreshed first if not.
    pub fn token(self: &Arc<Self>) -> impl Future<Output = Result<String, Failure>> + use<> {
        let me = self.clone();
        self.on_runtime(async move { me.token_here(false).await })
    }

    /// A fresh access token, whatever the old one's time (after a 401).
    pub fn refresh_now(self: &Arc<Self>) -> impl Future<Output = Result<String, Failure>> + use<> {
        let me = self.clone();
        self.on_runtime(async move { me.token_here(true).await })
    }

    fn credential(&self) -> Result<ClaudeCredential, Failure> {
        self.lock()
            .credential
            .clone()
            .ok_or_else(|| Failure::Account(NOT_SIGNED_IN.into()))
    }

    async fn token_here(&self, force: bool) -> Result<String, Failure> {
        let seen = self.credential()?;
        if !force && seen.expires.unix() - now_unix() > REFRESH_AHEAD {
            return Ok(seen.access);
        }
        let _one = self.refreshing.lock().await;
        let now = self.credential()?;
        if now.refresh != seen.refresh {
            // Refreshed while this one waited.
            return Ok(now.access);
        }
        self.refresh(now).await
    }

    async fn refresh(&self, original: ClaudeCredential) -> Result<String, Failure> {
        let body = json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": original.refresh,
        });
        let (status, data) = self
            .post_token(&body)
            .await
            .map_err(|e| Failure::Account(format!("Claude token refresh failed: {e}")))?;
        if status == 400 || status == 401 {
            self.sign_out();
            return Err(Failure::Account(
                "Claude sign-in expired — sign in again in Settings › Intelligence".into(),
            ));
        }
        if !(200..300).contains(&status) {
            return Err(Failure::Account(http_failure(
                "Claude token refresh",
                status,
                &data,
            )));
        }
        let (access, refresh, expires_in) = parse_token(&data).ok_or_else(|| {
            Failure::Account("Claude token refresh returned an incomplete token".into())
        })?;
        let mut inner = self.lock();
        let Some(credential) = inner
            .credential
            .as_mut()
            .filter(|c| c.refresh == original.refresh)
        else {
            return Err(Failure::Account(
                "Claude sign-in changed while the token was refreshing".into(),
            ));
        };
        credential.access = access.clone();
        credential.refresh = refresh;
        credential.expires = SwiftDate::from_unix(now_unix() + expires_in);
        let saved = credential.clone();
        self.save(&saved);
        self.publish(&inner);
        Ok(access)
    }

    /// `work` on the account's runtime, awaited from any loop.
    fn on_runtime<T, F>(&self, work: F) -> impl Future<Output = Result<T, Failure>> + use<T, F>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, Failure>> + Send + 'static,
    {
        let task = self.runtime.spawn(work);
        async move {
            task.await
                .map_err(|e| Failure::Network(format!("the request did not finish: {e}")))?
        }
    }

    // MARK: - the model lane

    /// One system + user message, answered as `(text, model that answered)`
    /// (`Claude.ask`).
    pub fn ask(
        self: &Arc<Self>,
        system: &str,
        user: &str,
        model: &str,
        max_tokens: u32,
        timeout: Duration,
    ) -> impl Future<Output = Result<(String, String), Failure>> + use<> {
        let model = model.to_string();
        let done = self.complete(
            &model,
            system,
            vec![json!({ "role": "user", "content": user })],
            Vec::new(),
            max_tokens.max(4096),
            timeout,
            Marks::Prefix,
        );
        async move {
            let payload = done.await?;
            let text = messages::joined_text(&payload["content"]);
            let answered = payload["model"].as_str().unwrap_or(&model).to_string();
            Ok((text, answered))
        }
    }

    /// One Messages request on the account (`Claude.complete`): the
    /// identity block, then `system`; asked again with a fresh token after
    /// a 401. The answer's JSON.
    #[allow(clippy::too_many_arguments)]
    pub fn complete(
        self: &Arc<Self>,
        model: &str,
        system: &str,
        messages: Vec<Value>,
        tools: Vec<Value>,
        max_tokens: u32,
        timeout: Duration,
        marks: Marks,
    ) -> impl Future<Output = Result<Value, Failure>> + use<> {
        let body = messages::body(
            model,
            messages::system(system),
            messages,
            tools,
            max_tokens,
            marks,
        );
        let bytes = serde_json::to_vec(&body).unwrap_or_default();
        let me = self.clone();
        self.on_runtime(async move {
            let token = me.token_here(false).await?;
            match me.send(&token, &bytes, timeout).await {
                Err(Failure::Http { status: 401, .. }) => {
                    let token = me.token_here(true).await?;
                    me.send(&token, &bytes, timeout).await
                }
                other => other,
            }
        })
    }

    async fn send(&self, token: &str, body: &[u8], timeout: Duration) -> Result<Value, Failure> {
        let failed = |e: reqwest::Error| {
            if e.is_timeout() {
                Failure::TimedOut
            } else {
                Failure::Network(words(&e))
            }
        };
        let response = self
            .http
            .post(&self.endpoints.messages)
            .timeout(timeout)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", messages::BETAS)
            .header("user-agent", messages::USER_AGENT)
            .header("x-app", "cli")
            .header("anthropic-dangerous-direct-browser-access", "true")
            .body(body.to_vec())
            .send()
            .await
            .map_err(failed)?;
        let status = response.status().as_u16();
        let data = response.bytes().await.map_err(failed)?;
        if status != 200 {
            return Err(messages::failure(status, &data));
        }
        serde_json::from_slice(&data).map_err(|_| Failure::Http {
            status,
            text: format!("Claude {status}: {}", messages::prefix(&data, 300)),
        })
    }

    // MARK: - `copper claude`

    /// The loopback-only `copper/claude` method (`ClaudeAccount.control`):
    /// `op` is `status` (default), `signin`, `paste` (with `code`),
    /// `signout` or `cancel`; the answer is the status, never a token.
    /// `sign_in` is the shell's: open the tab, or say why not.
    pub fn control(
        self: &Arc<Self>,
        params: &Map<String, Value>,
        sign_in: impl FnOnce() -> Result<(), String>,
    ) -> Value {
        match params.get("op").and_then(Value::as_str).unwrap_or("status") {
            "status" => {}
            "signin" => {
                if let Err(e) = sign_in() {
                    return json!({ "error": e });
                }
            }
            "paste" => match params.get("code").and_then(Value::as_str) {
                Some(code) => self.paste(code),
                None => return json!({ "error": "paste needs code" }),
            },
            "signout" => self.sign_out(),
            "cancel" => self.cancel(),
            op => {
                return json!({
                    "error": format!("unknown claude op {op} (status, signin, paste, signout, cancel)")
                });
            }
        }
        self.status().json()
    }
}

fn snapshot(inner: &Inner) -> Status {
    let c = inner.credential.as_ref();
    let text = |f: fn(&ClaudeCredential) -> &Option<String>| {
        c.and_then(|c| f(c).clone()).unwrap_or_default()
    };
    Status {
        signed_in: c.is_some(),
        email: text(|c| &c.email),
        name: text(|c| &c.name),
        organization: text(|c| &c.organization),
        expires: c.map_or(0.0, |c| c.expires.unix()),
        phase: inner.phase.clone(),
        session: inner.session,
    }
}

fn set_phase(inner: &mut Inner, next: Phase) {
    if inner.phase != next {
        tracing::info!(target: "copper::claude", "claude phase {}", next.name());
        inner.phase = next;
    }
}

/// The attempt is over (`cleanupFlow` + the tab): nothing to paste into.
fn end_flow(inner: &mut Inner) {
    Flow::stop(&mut inner.flow.timeout);
    inner.flow.verifier = None;
    inner.flow.redirect = None;
    inner.flow.handled = false;
    if let Some(tab) = inner.flow.tab.take() {
        inner.closing.push(tab);
    }
}

fn code_and_state(query: &str) -> (Option<String>, Option<String>) {
    let (mut code, mut state) = (None, None);
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" if code.is_none() => code = Some(v.into_owned()),
            "state" if state.is_none() => state = Some(v.into_owned()),
            _ => {}
        }
    }
    (code, state)
}

/// What a person pasted: a callback address, its query, `code#state`, or
/// the code alone (`ClaudeAccount.parseRedirect`).
fn parse_redirect(pasted: &str) -> (Option<String>, Option<String>) {
    let t = pasted.trim();
    if let Ok(url) = url::Url::parse(t)
        && let Some(query) = url.query()
    {
        return code_and_state(query);
    }
    if let Some((code, state)) = t.split_once('#') {
        let decoded = |s: &str| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8()
                .map_or_else(|_| s.to_string(), |d| d.into_owned())
        };
        return (Some(decoded(code)), Some(decoded(state)));
    }
    let query = t.strip_prefix('?').unwrap_or(t);
    if url::form_urlencoded::parse(query.as_bytes()).any(|(k, _)| k == "code") {
        return code_and_state(query);
    }
    (Some(t.to_string()), None)
}

/// `(access, refresh, expires_in)` from a token answer.
fn parse_token(data: &[u8]) -> Option<(String, String, f64)> {
    let v: Value = serde_json::from_slice(data).ok()?;
    Some((
        v["access_token"].as_str()?.to_string(),
        v["refresh_token"].as_str()?.to_string(),
        v["expires_in"].as_f64()?,
    ))
}

/// `"<prefix> failed: <what the server said>"` (`ClaudeAccount.httpFailure`).
fn http_failure(prefix: &str, status: u16, data: &[u8]) -> String {
    let v: Value = serde_json::from_slice(data).unwrap_or_default();
    let detail = ["error_description", "message", "error"]
        .iter()
        .find_map(|k| v[k].as_str().filter(|s| !s.is_empty()))
        .map_or_else(
            || format!("HTTP {status}"),
            |s| s.chars().take(240).collect(),
        );
    format!("{prefix} failed: {detail}")
}

/// A transport error and its causes.
fn words(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        text = format!("{text}: {s}");
        source = s.source();
    }
    text
}

/// The page the tab shows when it comes back (the Mac's, word for word).
fn page(heading: &str, line: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Copper</title><style>body{{font-family:system-ui,sans-serif;max-width:38rem;margin:12vh auto;padding:0 1.5rem;line-height:1.5;color:#202124}}h1{{font-size:1.35rem;font-weight:600}}p{{color:#5f6368}}</style></head><body><h1>{heading}</h1><p>{line}</p></body></html>"
    )
}
