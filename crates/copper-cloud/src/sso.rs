//! Single sign-on: "Continue with Google" (Mac: `CloudSSO`).
//!
//! An instance that offers it lists its identity providers at
//! `GET /v1/auth/sso` (ungated, no auth; a 404 is an instance that predates
//! single sign-on). Signing in is the native-app loopback flow (RFC 8252)
//! with PKCE S256 (RFC 7636):
//!
//! 1. Listen on `127.0.0.1:0` (tokio; nothing toolkit- or OS-specific).
//! 2. Open the system browser — through the caller's own opener, so a GTK
//!    shell, a Windows shell and the command line each open it their way — at
//!    the provider's `authorization_endpoint` with `client_id`,
//!    `response_type=code`, `scope`, `redirect_uri=http://127.0.0.1:<port>/`,
//!    `state`, `code_challenge` (+ `code_challenge_method=S256`),
//!    `prompt=select_account` and `hd` when the instance names one.
//! 3. The browser comes back to the loopback with `code` and `state`. A
//!    `state` that isn't this attempt's is refused and the wait goes on.
//! 4. `POST /v1/auth/sso/{provider}` (ungated) with the code, the verifier,
//!    the redirect URI and this device. The client secret never leaves the
//!    server; it exchanges the code itself.
//! 5. The answer is kept exactly as a pairing's is (the link with the answer's
//!    `gate_key` as its key, the account, the token, the device id), and only
//!    then does the browser tab say "You're signed in — return to Copper".
//!
//! Turning sync on afterwards is the caller's: the same as after pairing
//! (every switch, or the ones this device had before).
//!
//! The wait is 5 minutes and can be cancelled ([`SsoCancel`]). Nothing here
//! is logged but the provider and the host; never the code, the verifier or
//! the token.

use crate::client::{Account, CloudClient, REQUEST_TIMEOUT, Signed};
use crate::error::{Failure, Result};
use crate::link::{Endpoint, SsoLink};
use crate::tls::Trust;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

/// How long the browser has to come back.
pub const SSO_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long one connection to the loopback has to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of a request the loopback reads.
const MAX_HEAD: usize = 16 * 1024;

/// `GET /v1/auth/sso`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SsoInfo {
    #[serde(default)]
    pub enabled: bool,
    /// Email and password still work for everyone (`false`: only admins).
    #[serde(default = "yes")]
    pub password_signin: bool,
    #[serde(default)]
    pub providers: Vec<SsoProvider>,
}

fn yes() -> bool {
    true
}

impl SsoInfo {
    /// The providers to offer: none when single sign-on is off.
    pub fn offered(&self) -> &[SsoProvider] {
        if self.enabled { &self.providers } else { &[] }
    }

    /// The offered provider with this id.
    pub fn provider(&self, id: &str) -> Option<&SsoProvider> {
        self.offered()
            .iter()
            .find(|p| p.id.eq_ignore_ascii_case(id))
    }
}

/// One identity provider an instance trusts.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SsoProvider {
    /// `google`; the `{provider_id}` of `POST /v1/auth/sso/{provider_id}`.
    pub id: String,
    /// `google` or `oidc`.
    #[serde(default)]
    pub kind: String,
    /// What the button says after "Continue with".
    #[serde(default)]
    pub name: String,
    pub client_id: String,
    pub authorization_endpoint: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The email domains the instance takes from this provider.
    #[serde(default)]
    pub domains: Vec<String>,
    /// Google's hosted-domain hint, when there is exactly one domain.
    #[serde(default)]
    pub hd: Option<String>,
}

impl SsoProvider {
    /// "Google": the name, or a title made from the id.
    pub fn title(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            provider_title(&self.id)
        } else {
            name.to_string()
        }
    }

    /// "Continue with Google".
    pub fn button(&self) -> String {
        format!("Continue with {}", self.title())
    }
}

/// A provider id as people read it, before the instance has said its name.
pub fn provider_title(id: &str) -> String {
    match id.to_ascii_lowercase().as_str() {
        "google" => "Google".into(),
        "microsoft" | "azure" | "entra" => "Microsoft".into(),
        "github" => "GitHub".into(),
        "gitlab" => "GitLab".into(),
        "okta" => "Okta".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => "your company account".into(),
            }
        }
    }
}

/// Stops a sign-in that is waiting for the browser. Cheap to clone; every
/// clone cancels the same attempt.
#[derive(Clone, Debug)]
pub struct SsoCancel {
    tx: Arc<watch::Sender<bool>>,
}

impl Default for SsoCancel {
    fn default() -> Self {
        let (tx, _) = watch::channel(false);
        SsoCancel { tx: Arc::new(tx) }
    }
}

impl SsoCancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Give up: the sign-in fails with `sso_cancelled` at once.
    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        let _ = rx.wait_for(|c| *c).await;
    }
}

/// A finished sign-in.
#[derive(Debug, Clone, PartialEq)]
pub struct SsoOutcome {
    /// Who is signed in now.
    pub account: Account,
    /// The instance made the account just now.
    pub created: bool,
    /// The provider's id.
    pub provider: String,
}

/// `GET /v1/auth/sso` on any instance, through `fingerprint` (or, with none,
/// a certificate the public authorities vouch for). `Ok(None)`: the instance
/// predates single sign-on (404) — hide the button.
pub async fn discover(endpoint: &Endpoint, fingerprint: Option<&str>) -> Result<Option<SsoInfo>> {
    let probe = Trust::new(fingerprint.filter(|_| endpoint.https));
    discover_with(&probe.http_client(), &probe, endpoint).await
}

async fn discover_with(
    http: &reqwest::Client,
    probe: &Trust,
    endpoint: &Endpoint,
) -> Result<Option<SsoInfo>> {
    let resp = http
        .get(endpoint.url("v1/auth/sso"))
        .header("Accept", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| probe.explain(&e))?;
    let status = resp.status().as_u16();
    let body = resp.bytes().await.unwrap_or_default();
    if status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        return Err(sso_failure(Failure::from_body(&body, status), &[]));
    }
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| Failure::decode(status, "the sign-in options", e))
}

/// The refusals of `POST /v1/auth/sso/{id}` (and `GET /v1/auth/sso`), in
/// words. `domains` are the provider's, for `sso_domain`.
pub fn sso_failure(mut failure: Failure, domains: &[String]) -> Failure {
    match (failure.status, failure.code.as_str()) {
        (_, "sso_domain") => {
            let shown: Vec<String> = domains
                .iter()
                .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
                .filter(|d| !d.is_empty())
                .map(|d| format!("@{d}"))
                .collect();
            failure.message = match shown.as_slice() {
                [] if !failure.message.trim().is_empty() => failure.message.clone(),
                [] => "This instance doesn't take accounts from that domain — use your company account".into(),
                [one] => format!("Use your {one} account"),
                many => format!(
                    "Use your {} or {} account",
                    many[..many.len() - 1].join(", "),
                    many[many.len() - 1]
                ),
            }
        }
        (_, "sso_failed") => {
            failure.message =
                "The sign-in didn't go through — the identity provider didn't confirm it. Try again."
                    .into()
        }
        (_, "sso_disabled") => {
            failure.message =
                "This instance has single sign-on turned off — ask its admin, or sign in with email and password".into()
        }
        (_, "account_disabled") => {
            failure.message = "Your account on this instance is disabled — ask its admin".into()
        }
        (_, "sso_no_account") => {
            failure.message = "There's no account for that email on this instance, and it doesn't make new ones — ask its admin for an invite".into()
        }
        (_, "bad_redirect") => {
            failure.message =
                "The instance refused Copper's sign-in return address — update Copper or copper-cloud"
                    .into()
        }
        (429, _) | (_, "rate_limited") => {
            failure.message = "Too many tries — wait a minute and try again".into()
        }
        (404, _) => {
            failure.code = "sso_unsupported".into();
            failure.message = "This instance doesn't offer single sign-on — update copper-cloud, or connect with a link code".into()
        }
        _ => {}
    }
    failure
}

/// A PKCE verifier (43 characters of base64url: 32 random bytes) and its
/// S256 challenge.
pub(crate) fn pkce_pair() -> Result<(String, String)> {
    let verifier = random_token(32)?;
    Ok((verifier.clone(), challenge_for(&verifier)))
}

/// `BASE64URL(SHA256(verifier))`, unpadded.
pub(crate) fn challenge_for(verifier: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest.as_ref())
}

fn random_token(bytes: usize) -> Result<String> {
    use ring::rand::SecureRandom;
    let mut buf = vec![0u8; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| Failure::new(0, "random", "No randomness for the sign-in"))?;
    Ok(URL_SAFE_NO_PAD.encode(&buf))
}

/// The provider's authorization URL for one attempt.
pub(crate) fn authorization_url(
    provider: &SsoProvider,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
) -> Result<String> {
    let bad = || {
        Failure::new(
            0,
            "sso_provider",
            format!(
                "The instance's sign-in address for {} isn't usable",
                provider.title()
            ),
        )
    };
    let mut url = url::Url::parse(provider.authorization_endpoint.trim()).map_err(|_| bad())?;
    let loopback = matches!(
        url.host(),
        Some(url::Host::Ipv4(ip)) if ip.is_loopback()
    ) || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        || url.host_str() == Some("localhost");
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
        return Err(bad());
    }
    if provider.client_id.trim().is_empty() {
        return Err(bad());
    }
    let scope = if provider.scopes.is_empty() {
        "openid email profile".to_string()
    } else {
        provider.scopes.join(" ")
    };
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", provider.client_id.trim())
            .append_pair("response_type", "code")
            .append_pair("scope", &scope)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "select_account");
        if let Some(hd) = provider
            .hd
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty())
        {
            q.append_pair("hd", hd);
        }
    }
    Ok(url.to_string())
}

/// What the browser brought back to the loopback.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Callback {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) error_description: Option<String>,
}

/// The request line of a request to the loopback, read: `GET /?…` is the
/// callback; anything else is `Err(status)`.
pub(crate) fn parse_request(head: &str) -> std::result::Result<Callback, u16> {
    let line = head.lines().next().unwrap_or("");
    let mut parts = line.split(' ');
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Err(400);
    };
    if method != "GET" {
        return Err(405);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/" {
        return Err(404);
    }
    let mut cb = Callback::default();
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        let v = Some(v.into_owned());
        match k.as_ref() {
            "code" => cb.code = v,
            "state" => cb.state = v,
            "error" => cb.error = v,
            "error_description" => cb.error_description = v,
            _ => {}
        }
    }
    Ok(cb)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

const PAGE_STYLE: &str = "body{font:16px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif;background:#f6f4f1;color:#1d1b19;display:grid;place-items:center;min-height:90vh;margin:0}main{max-width:28rem;padding:2rem;text-align:center}h1{font-size:1.4rem;font-weight:600;margin:0 0 .5rem}p{margin:0;color:#5c5650}@media(prefers-color-scheme:dark){body{background:#1d1b19;color:#f6f4f1}p{color:#b5ada5}}";

/// The page the browser shows when the sign-in worked.
pub fn success_page() -> String {
    page(
        "Signed in to Copper",
        "You\u{2019}re signed in \u{2014} return to Copper",
        "You can close this tab.",
    )
}

/// The page the browser shows when it didn't.
pub fn error_page(message: &str) -> String {
    page(
        "Copper sign-in",
        "Copper couldn\u{2019}t sign you in",
        &format!("{} Return to Copper to try again.", message.trim()),
    )
}

fn page(title: &str, heading: &str, detail: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{}</title><style>{PAGE_STYLE}</style></head><body><main><h1>{}</h1><p>{}</p></main></body></html>",
        escape(title),
        escape(heading),
        escape(detail)
    )
}

async fn respond(stream: &mut TcpStream, status: u16, html: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n",
        html.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(html.as_bytes()).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.windows(2).any(|w| w == b"\n\n") {
            return String::from_utf8(buf).ok();
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
    }
}

/// Aborts the accept loop when the attempt ends, however it ends.
struct Abort(tokio::task::JoinHandle<()>);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The loopback: every connection's request read on its own task (a browser
/// opens speculative connections that never send anything), callbacks handed
/// over with the connection so the answer page waits for the outcome.
async fn listen() -> Result<(u16, mpsc::Receiver<(TcpStream, Callback)>, Abort)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| {
        Failure::new(
            0,
            "sso_listen",
            format!("Copper couldn't listen for the sign-in to come back: {e}"),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| Failure::new(0, "sso_listen", e.to_string()))?
        .port();
    let (tx, rx) = mpsc::channel::<(TcpStream, Callback)>(8);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, peer)) = listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            if !peer.ip().is_loopback() {
                continue;
            }
            let tx = tx.clone();
            tokio::spawn(async move {
                let Ok(Some(head)) =
                    tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await
                else {
                    return;
                };
                match parse_request(&head) {
                    Ok(cb) => {
                        let _ = tx.send((stream, cb)).await;
                    }
                    Err(status) => respond(&mut stream, status, &error_page("Nothing here.")).await,
                }
            });
        }
    });
    Ok((port, rx, Abort(task)))
}

impl CloudClient {
    /// The sign-in options of the instance this Copper is linked to
    /// (`Ok(None)`: it predates single sign-on). For the sign-in step's
    /// "Continue with …" button.
    pub async fn sso_info(&self) -> Result<Option<SsoInfo>> {
        let link = self.link().ok_or_else(Failure::not_linked)?;
        let trust = self.trust();
        discover_with(&trust.http_client(), &trust, &link.endpoint).await
    }

    /// Link and sign in through a sign-in link (`#sso=`), in one step:
    /// `/healthz` and `GET /v1/auth/sso` through the link's pin (or
    /// `trusting`, a fingerprint a person confirmed after an `untrusted`
    /// failure), then the browser flow. `open` opens the system browser at
    /// the URL it is given; an `Err` from it ends the attempt.
    pub async fn sign_in_with_sso(
        &self,
        link: &SsoLink,
        trusting: Option<&str>,
        open: impl FnOnce(&str) -> std::result::Result<(), String>,
        cancel: &SsoCancel,
    ) -> Result<SsoOutcome> {
        let pin = Self::pin_for(&link.endpoint, link.fingerprint.as_deref(), trusting)?;
        self.sso_flow(&link.endpoint, pin, &link.provider, open, cancel)
            .await
    }

    /// "Continue with …" on the sign-in step of the instance this Copper is
    /// linked to already.
    pub async fn sign_in_with_sso_here(
        &self,
        provider: &str,
        open: impl FnOnce(&str) -> std::result::Result<(), String>,
        cancel: &SsoCancel,
    ) -> Result<SsoOutcome> {
        let link = self.link().ok_or_else(Failure::not_linked)?;
        self.sso_flow(
            &link.endpoint,
            link.fingerprint.clone(),
            provider,
            open,
            cancel,
        )
        .await
    }

    async fn sso_flow(
        &self,
        endpoint: &Endpoint,
        pin: Option<String>,
        provider_id: &str,
        open: impl FnOnce(&str) -> std::result::Result<(), String>,
        cancel: &SsoCancel,
    ) -> Result<SsoOutcome> {
        let probe = Trust::new(pin.as_deref());
        let http = probe.http_client();
        Self::healthz(&http, &probe, endpoint).await?;
        let info = discover_with(&http, &probe, endpoint)
            .await?
            .ok_or_else(|| sso_failure(Failure::new(404, "http_404", ""), &[]))?;
        if !info.enabled {
            return Err(sso_failure(Failure::new(404, "sso_disabled", ""), &[]));
        }
        let provider = info.provider(provider_id).cloned().ok_or_else(|| {
            Failure::new(
                0,
                "sso_provider",
                format!(
                    "This instance doesn't sign in with {} any more — ask its admin for a new link",
                    provider_title(provider_id)
                ),
            )
        })?;

        let (port, mut callbacks, _accepting) = listen().await?;
        let redirect_uri = format!("http://127.0.0.1:{port}/");
        let state = random_token(24)?;
        let (verifier, challenge) = pkce_pair()?;
        let url = authorization_url(&provider, &redirect_uri, &state, &challenge)?;
        tracing::info!(provider = %provider.id, host = %endpoint.host_text(), "single sign-on: opening the browser");
        open(&url).map_err(|e| {
            Failure::new(
                0,
                "browser",
                format!("Copper couldn't open the browser for the sign-in: {e}"),
            )
        })?;

        let deadline = tokio::time::Instant::now() + SSO_TIMEOUT;
        let (mut stream, code) = loop {
            let next = tokio::select! {
                next = callbacks.recv() => next,
                _ = tokio::time::sleep_until(deadline) => {
                    return Err(Failure::new(0, "sso_timeout", "Nothing came back from the browser in 5 minutes — try again"));
                }
                _ = cancel.cancelled() => {
                    return Err(Failure::new(0, "sso_cancelled", "Sign-in cancelled"));
                }
            };
            let Some((mut stream, cb)) = next else {
                return Err(Failure::new(
                    0,
                    "sso_listen",
                    "Copper stopped listening for the sign-in",
                ));
            };
            if cb.state.as_deref() != Some(state.as_str()) {
                // Not this attempt's: refused, and the wait goes on.
                respond(
                    &mut stream,
                    400,
                    &error_page("That sign-in is from another attempt."),
                )
                .await;
                continue;
            }
            if let Some(error) = cb.error.as_deref() {
                let f = if error == "access_denied" {
                    Failure::new(
                        0,
                        "sso_denied",
                        format!("The sign-in was cancelled at {}", provider.title()),
                    )
                } else {
                    let detail = cb
                        .error_description
                        .as_deref()
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .unwrap_or(error);
                    Failure::new(
                        0,
                        "sso_failed",
                        format!("{} refused the sign-in: {detail}", provider.title()),
                    )
                };
                respond(&mut stream, 400, &error_page(&f.message)).await;
                return Err(f);
            }
            match cb.code.filter(|c| !c.is_empty()) {
                Some(code) => break (stream, code),
                None => {
                    let f = Failure::new(
                        0,
                        "sso_failed",
                        format!("{} sent no sign-in code back", provider.title()),
                    );
                    respond(&mut stream, 400, &error_page(&f.message)).await;
                    return Err(f);
                }
            }
        };

        let outcome = self
            .sso_exchange(
                endpoint,
                pin,
                &probe,
                &http,
                &provider,
                &code,
                &verifier,
                &redirect_uri,
            )
            .await;
        match &outcome {
            Ok(_) => respond(&mut stream, 200, &success_page()).await,
            Err(f) => respond(&mut stream, 400, &error_page(&f.message)).await,
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    async fn sso_exchange(
        &self,
        endpoint: &Endpoint,
        pin: Option<String>,
        probe: &Trust,
        http: &reqwest::Client,
        provider: &SsoProvider,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<SsoOutcome> {
        let resp = http
            .post(endpoint.url(&format!("v1/auth/sso/{}", provider.id)))
            .header("Accept", "application/json")
            .json(&json!({
                "code": code,
                "code_verifier": verifier,
                "redirect_uri": redirect_uri,
                "device": self.device_body(),
            }))
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| probe.explain(&e))?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(sso_failure(
                Failure::from_body(&body, status),
                &provider.domains,
            ));
        }
        #[derive(Deserialize)]
        struct Created {
            #[serde(default)]
            created: bool,
        }
        let created = serde_json::from_slice::<Created>(&body)
            .map(|c| c.created)
            .unwrap_or(false);
        let answer: Signed =
            serde_json::from_slice(&body).map_err(|e| Failure::decode(status, "the sign-in", e))?;
        let account = self.keep_signed_in(endpoint, pin, probe, answer).await?;
        tracing::info!(provider = %provider.id, created, "signed in with single sign-on");
        Ok(SsoOutcome {
            account,
            created,
            provider: provider.id.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_is_s256() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let (verifier, challenge) = pkce_pair().unwrap();
        assert_eq!(verifier.len(), 43);
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_eq!(challenge, challenge_for(&verifier));
        assert_ne!(pkce_pair().unwrap().0, verifier);
    }

    fn google() -> SsoProvider {
        SsoProvider {
            id: "google".into(),
            kind: "google".into(),
            name: "Google".into(),
            client_id: "123-abc.apps.googleusercontent.com".into(),
            authorization_endpoint: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
            domains: vec!["exowatt.com".into()],
            hd: Some("exowatt.com".into()),
        }
    }

    #[test]
    fn the_authorization_url() {
        let url = authorization_url(&google(), "http://127.0.0.1:53682/", "st", "ch").unwrap();
        let u = url::Url::parse(&url).unwrap();
        assert_eq!(u.host_str(), Some("accounts.google.com"));
        let q: std::collections::BTreeMap<String, String> = u.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "123-abc.apps.googleusercontent.com");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["scope"], "openid email profile");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:53682/");
        assert_eq!(q["state"], "st");
        assert_eq!(q["code_challenge"], "ch");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["prompt"], "select_account");
        assert_eq!(q["hd"], "exowatt.com");
        // No hd, no scopes: the OIDC defaults.
        let mut oidc = google();
        oidc.hd = None;
        oidc.scopes.clear();
        let url = authorization_url(&oidc, "http://127.0.0.1:1/", "s", "c").unwrap();
        assert!(!url.contains("hd="));
        assert!(url.contains("scope=openid+email+profile"));
        // Only https, or http on loopback.
        oidc.authorization_endpoint = "http://evil.example/auth".into();
        assert!(authorization_url(&oidc, "http://127.0.0.1:1/", "s", "c").is_err());
        oidc.authorization_endpoint = "http://127.0.0.1:9/auth".into();
        assert!(authorization_url(&oidc, "http://127.0.0.1:1/", "s", "c").is_ok());
        oidc.authorization_endpoint = "javascript:alert(1)".into();
        assert!(authorization_url(&oidc, "http://127.0.0.1:1/", "s", "c").is_err());
    }

    #[test]
    fn discovery_reads() {
        let info: SsoInfo = serde_json::from_value(json!({
            "enabled": true, "password_signin": true,
            "providers": [{"id": "google", "kind": "google", "name": "Google",
              "client_id": "123-abc.apps.googleusercontent.com",
              "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
              "scopes": ["openid", "email", "profile"], "domains": ["exowatt.com"], "hd": "exowatt.com"}]
        }))
        .unwrap();
        assert_eq!(info.provider("Google"), Some(&google()));
        assert_eq!(info.offered()[0].button(), "Continue with Google");
        let off: SsoInfo = serde_json::from_value(
            json!({"enabled": false, "password_signin": true, "providers": []}),
        )
        .unwrap();
        assert!(off.offered().is_empty());
        // Off but listing providers anyway: still nothing offered.
        let mut odd = info.clone();
        odd.enabled = false;
        assert!(odd.provider("google").is_none());
    }

    #[test]
    fn refusals_in_words() {
        let domains = vec!["exowatt.com".to_string()];
        let f = sso_failure(
            Failure::new(403, "sso_domain", "only exowatt.com"),
            &domains,
        );
        assert_eq!(f.message, "Use your @exowatt.com account");
        let two = vec!["a.com".to_string(), "@B.com".to_string()];
        let f = sso_failure(Failure::new(403, "sso_domain", "x"), &two);
        assert_eq!(f.message, "Use your @a.com or @b.com account");
        let f = sso_failure(Failure::new(403, "sso_domain", "Only a.com"), &[]);
        assert_eq!(f.message, "Only a.com");
        for (status, code, needle) in [
            (401, "sso_failed", "didn't go through"),
            (404, "sso_disabled", "turned off"),
            (403, "account_disabled", "disabled"),
            (403, "sso_no_account", "no account"),
            (400, "bad_redirect", "return address"),
            (429, "rate_limited", "Too many"),
        ] {
            let f = sso_failure(Failure::new(status, code, "raw"), &domains);
            assert!(f.message.contains(needle), "{code}: {}", f.message);
            assert_eq!(f.code, code);
        }
        let f = sso_failure(Failure::new(404, "http_404", "Not Found"), &[]);
        assert_eq!(f.code, "sso_unsupported");
    }

    #[test]
    fn the_loopback_reads_callbacks() {
        let cb =
            parse_request("GET /?state=s%2B1&code=4%2F0Ab&scope=email HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
        assert_eq!(cb.state.as_deref(), Some("s+1"));
        assert_eq!(cb.code.as_deref(), Some("4/0Ab"));
        let cb = parse_request("GET /?error=access_denied&state=s HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(cb.error.as_deref(), Some("access_denied"));
        assert_eq!(parse_request("GET /favicon.ico HTTP/1.1\r\n\r\n"), Err(404));
        assert_eq!(parse_request("POST /?code=x HTTP/1.1\r\n\r\n"), Err(405));
        assert_eq!(parse_request(""), Err(400));
    }

    #[test]
    fn pages_say_it_plainly() {
        assert!(success_page().contains("You\u{2019}re signed in \u{2014} return to Copper"));
        let page = error_page("Use your <b>@exowatt.com</b> account");
        assert!(page.contains("Use your &lt;b&gt;@exowatt.com&lt;/b&gt; account"));
        assert!(!page.contains("<b>"));
    }

    #[test]
    fn provider_titles() {
        assert_eq!(provider_title("google"), "Google");
        assert_eq!(provider_title("corp-okta"), "Corp-okta");
        assert_eq!(provider_title("okta"), "Okta");
    }

    // MARK: against a fake instance

    use crate::link::Endpoint;
    use copper_core::{DataFile, Paths, Store};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Seen {
        /// The `code_challenge` the browser was sent to the provider with.
        challenge: Option<String>,
        /// The bodies of `POST /v1/auth/sso/google`.
        exchanges: Vec<serde_json::Value>,
        /// `GET /v1/auth/sso` answers 404 (an old instance).
        old: bool,
    }

    /// A copper-cloud that knows `/healthz`, `GET /v1/auth/sso` and
    /// `POST /v1/auth/sso/google`, on plain loopback HTTP.
    async fn fake_instance(seen: Arc<Mutex<Seen>>) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, mut body) = loop {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break (
                                String::from_utf8_lossy(&buf[..at]).to_string(),
                                buf[at + 4..].to_vec(),
                            );
                        }
                    };
                    let length = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    while body.len() < length {
                        let n = stream.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&chunk[..n]);
                    }
                    let line = head.lines().next().unwrap_or("").to_string();
                    let (status, answer) = route(&line, &body, port, &seen);
                    let text = answer.to_string();
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        port
    }

    fn route(line: &str, body: &[u8], port: u16, seen: &Mutex<Seen>) -> (u16, serde_json::Value) {
        let mut seen = seen.lock().unwrap();
        if line.starts_with("GET /healthz ") {
            return (200, json!({"ok": true}));
        }
        if line.starts_with("GET /v1/auth/sso ") {
            if seen.old {
                return (404, json!({"error": "not_found", "message": "Not Found"}));
            }
            return (
                200,
                json!({"enabled": true, "password_signin": true, "providers": [{
                    "id": "google", "kind": "google", "name": "Google",
                    "client_id": "123-abc.apps.googleusercontent.com",
                    "authorization_endpoint": format!("http://127.0.0.1:{port}/authorize"),
                    "scopes": ["openid", "email", "profile"],
                    "domains": ["exowatt.com"], "hd": "exowatt.com"}]}),
            );
        }
        if line.starts_with("POST /v1/auth/sso/google ") {
            let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
            seen.exchanges.push(v.clone());
            let verifier = v["code_verifier"].as_str().unwrap_or("");
            if Some(challenge_for(verifier)) != seen.challenge {
                return (401, json!({"error": "sso_failed", "message": "pkce"}));
            }
            if !v["redirect_uri"]
                .as_str()
                .is_some_and(|r| r.starts_with("http://127.0.0.1:") && r.ends_with('/'))
            {
                return (400, json!({"error": "bad_redirect", "message": "redirect"}));
            }
            return match v["code"].as_str() {
                Some("good-code") => (
                    200,
                    json!({
                        "token": "tok_secret_sso",
                        "user": {"id": "7d1c3a8e-0000-4000-8000-000000000001", "email": "farce@exowatt.com",
                                 "display_name": "Felipe Arce", "created_at": "2026-10-19T00:00:00Z"},
                        "device": {"id": "0b6e8f8a-1111-4111-8111-111111111111", "name": "FluxBot · Vera"},
                        "gate_key": "ck_minted_for_sso",
                        "created": true
                    }),
                ),
                Some("gmail-code") => (
                    403,
                    json!({"error": "sso_domain", "message": "Only exowatt.com accounts"}),
                ),
                _ => (401, json!({"error": "sso_failed", "message": "bad code"})),
            };
        }
        (404, json!({"error": "not_found", "message": "Not Found"}))
    }

    fn store(dir: &std::path::Path) -> Store {
        Store::new(Paths::new(dir.join("data"), dir.join("config")))
    }

    fn sso_link(port: u16) -> SsoLink {
        crate::link::parse_sso_link(&format!("http://127.0.0.1:{port}/#sso=google")).unwrap()
    }

    /// What the fake browser saw: the forged redirect's status, the page,
    /// the real redirect's status.
    type Pages = tokio::sync::oneshot::Receiver<(u16, String, u16)>;

    /// An opener that plays the browser: the provider "redirects" straight
    /// back to the loopback with `code` — first with a forged `state`, which
    /// must be refused — and hands over the page the loopback answered.
    fn browser(
        seen: Arc<Mutex<Seen>>,
        code: &'static str,
    ) -> (impl FnOnce(&str) -> std::result::Result<(), String>, Pages) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let open = move |url: &str| {
            let u = url::Url::parse(url).map_err(|e| e.to_string())?;
            let q: std::collections::BTreeMap<String, String> =
                u.query_pairs().into_owned().collect();
            assert_eq!(q["code_challenge_method"], "S256");
            assert_eq!(q["hd"], "exowatt.com");
            assert_eq!(q["prompt"], "select_account");
            seen.lock().unwrap().challenge = Some(q["code_challenge"].clone());
            let redirect = q["redirect_uri"].clone();
            let state = q["state"].clone();
            tokio::spawn(async move {
                let http = reqwest::Client::new();
                let forged = http
                    .get(format!("{redirect}?code={code}&state=forged"))
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .as_u16();
                let resp = http
                    .get(format!("{redirect}?code={code}&state={state}&scope=email"))
                    .send()
                    .await
                    .unwrap();
                let status = resp.status().as_u16();
                let text = resp.text().await.unwrap();
                let _ = tx.send((forged, text, status));
            });
            Ok(())
        };
        (open, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn signs_in_like_pairing() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let port = fake_instance(seen.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let client = CloudClient::open(store(tmp.path())).unwrap();
        let before = client.device_id();

        let info = discover(
            &Endpoint {
                https: false,
                host: "127.0.0.1".into(),
                port: Some(port),
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(info.offered()[0].title(), "Google");

        let (open, page) = browser(seen.clone(), "good-code");
        let outcome = client
            .sign_in_with_sso(&sso_link(port), None, open, &SsoCancel::new())
            .await
            .expect("signed in");
        assert!(outcome.created);
        assert_eq!(outcome.provider, "google");
        assert_eq!(outcome.account.email, "farce@exowatt.com");
        let (forged, html, status) = page.await.unwrap();
        assert_eq!(forged, 400, "a forged state is refused");
        assert_eq!(status, 200);
        assert!(
            html.contains("You’re signed in — return to Copper"),
            "{html}"
        );

        // What the instance was sent: the code, the verifier, the loopback,
        // this device (its old id, lowercase).
        let sent = seen.lock().unwrap().exchanges.clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["code"], "good-code");
        assert_eq!(sent[0]["device"]["id"], before.to_ascii_lowercase());

        // Kept exactly like a pairing: the gate key as the link's key, the
        // account, the token; the server's device id and name adopted.
        assert!(client.is_signed_in());
        let link = client.link().unwrap();
        assert_eq!(link.key, "ck_minted_for_sso");
        assert_eq!(link.endpoint.port, Some(port));
        assert_eq!(client.device_id(), "0B6E8F8A-1111-4111-8111-111111111111");
        assert_eq!(client.device_name(), "FluxBot · Vera");
        assert_eq!(
            client.account().unwrap().device_id,
            "0B6E8F8A-1111-4111-8111-111111111111"
        );
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store(tmp.path()).path(DataFile::Cloud)).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["token"], "tok_secret_sso");
        assert_eq!(saved["link"]["key"], "ck_minted_for_sso");
        assert_eq!(saved["deviceId"], "0B6E8F8A-1111-4111-8111-111111111111");
        // A fresh read sees the same.
        let again = CloudClient::open(store(tmp.path())).unwrap();
        assert!(again.is_signed_in());
        assert_eq!(again.account().unwrap().email, "farce@exowatt.com");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_is_explained_and_nothing_is_kept() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let port = fake_instance(seen.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let client = CloudClient::open(store(tmp.path())).unwrap();
        let (open, page) = browser(seen.clone(), "gmail-code");
        let err = client
            .sign_in_with_sso(&sso_link(port), None, open, &SsoCancel::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, "sso_domain");
        assert_eq!(err.message, "Use your @exowatt.com account");
        let (_, html, status) = page.await.unwrap();
        assert_eq!(status, 400);
        assert!(html.contains("Use your @exowatt.com account"), "{html}");
        assert!(!client.is_linked());
        assert!(!client.is_signed_in());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_while_waiting() {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let port = fake_instance(seen.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        let client = CloudClient::open(store(tmp.path())).unwrap();
        let cancel = SsoCancel::new();
        let later = cancel.clone();
        let opened = Arc::new(Mutex::new(None::<String>));
        let url = opened.clone();
        let open = move |u: &str| {
            *url.lock().unwrap() = Some(u.to_string());
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                later.cancel();
            });
            Ok(())
        };
        let err = tokio::time::timeout(
            Duration::from_secs(10),
            client.sign_in_with_sso(&sso_link(port), None, open, &cancel),
        )
        .await
        .expect("cancelled promptly")
        .unwrap_err();
        assert_eq!(err.code, "sso_cancelled");
        assert!(
            opened
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(|u| u.contains("/authorize?"))
        );
        assert!(!client.is_signed_in());
        // The opener failing ends the attempt too.
        let err = client
            .sign_in_with_sso(
                &sso_link(port),
                None,
                |_: &str| Err("no display".into()),
                &SsoCancel::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "browser");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_old_instance_has_no_sign_in() {
        let seen = Arc::new(Mutex::new(Seen {
            old: true,
            ..Default::default()
        }));
        let port = fake_instance(seen.clone()).await;
        let end = Endpoint {
            https: false,
            host: "127.0.0.1".into(),
            port: Some(port),
        };
        assert_eq!(discover(&end, None).await.unwrap(), None);
        let tmp = tempfile::tempdir().unwrap();
        let client = CloudClient::open(store(tmp.path())).unwrap();
        let err = client
            .sign_in_with_sso(&sso_link(port), None, |_: &str| Ok(()), &SsoCancel::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, "sso_unsupported");
    }

    #[tokio::test]
    async fn cancel_wakes_a_waiter() {
        let cancel = SsoCancel::new();
        let other = cancel.clone();
        let waiter = tokio::spawn(async move { other.cancelled().await });
        tokio::task::yield_now().await;
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("woken")
            .unwrap();
        assert!(cancel.is_cancelled());
    }
}
