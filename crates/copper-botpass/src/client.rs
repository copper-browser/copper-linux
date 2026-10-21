//! The BotPass HTTP API, as a paired Copper uses it.
//!
//! * `GET /v1/status` (no auth) — version, locked, the TLS fingerprint.
//! * `POST /v1/devices/pair` — a pairing code for a device token.
//! * `GET /v1/fill/accounts?origin=` — the accounts this device may use on a
//!   page's origin: usernames only.
//! * `POST /v1/fill` — one account's password or current code, for that
//!   origin, at the moment of filling. BotPass checks its grants again and
//!   records the use.
//!
//! TLS goes through [`copper_cloud::tls::Trust`]: with a fingerprint, the
//! server's leaf certificate must hash to it (and nothing else is trusted);
//! without one, the public authorities. Plain `http://` only for loopback.
//! Limits are short (a fill is waited on by a click or an agent).

use crate::error::Error;
use crate::pairing::Pairing;
use copper_cloud::tls::Trust;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use url::Url;

const CONNECT: Duration = Duration::from_secs(4);
const REQUEST: Duration = Duration::from_secs(8);

/// `https://host[:port]` (or loopback `http://`), with no path.
pub fn normalize_url(text: &str) -> Result<Url, Error> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Usage("the BotPass server URL is empty".into()));
    }
    let with_scheme = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    let mut url = Url::parse(&with_scheme)
        .map_err(|e| Error::Usage(format!("not a server URL: {text} ({e})")))?;
    let host = url
        .host_str()
        .ok_or_else(|| Error::Usage(format!("no host in {text}")))?
        .to_ascii_lowercase();
    match url.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        "http" => {
            return Err(Error::Usage(format!(
                "BotPass at {host} must be https (plain http only for this machine)"
            )));
        }
        other => return Err(Error::Usage(format!("unsupported scheme {other}:"))),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::Usage("put no user or password in the URL".into()));
    }
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn is_loopback(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    h == "localhost"
        || h.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A SHA-256 fingerprint as lowercase hex: colons, spaces and case are
/// ignored; anything but 64 hex digits is refused.
pub fn normalize_fingerprint(text: &str) -> Result<String, Error> {
    let hex: String = text
        .trim()
        .trim_start_matches("sha256:")
        .trim_start_matches("SHA256:")
        .chars()
        .filter(|c| !matches!(c, ':' | ' ' | '-'))
        .collect::<String>()
        .to_ascii_lowercase();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Usage(
            "the fingerprint must be a SHA-256 (64 hex digits, colons allowed)".into(),
        ));
    }
    Ok(hex)
}

/// A pairing link from `POST /v1/devices/pairing-code`
/// (`botpass://host[:port]/#c=CODE&fp=FP`): the server (https), the code
/// and the fingerprint, if it carries one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub server: String,
    pub code: String,
    pub fingerprint: Option<String>,
}

/// Read a `botpass://` pairing link; `None` for anything else.
pub fn parse_link(text: &str) -> Option<Link> {
    let rest = text.trim().strip_prefix("botpass://")?;
    let (host, fragment) = rest.split_once('#')?;
    let host = host.trim_end_matches('/');
    if host.is_empty() || host.contains('/') {
        return None;
    }
    let mut code = None;
    let mut fingerprint = None;
    for (k, v) in url::form_urlencoded::parse(fragment.as_bytes()) {
        match k.as_ref() {
            "c" => code = Some(v.into_owned()),
            "fp" => fingerprint = Some(v.into_owned()),
            _ => {}
        }
    }
    Some(Link {
        server: format!("https://{host}"),
        code: code.filter(|c| !c.is_empty())?,
        fingerprint: fingerprint.filter(|f| !f.is_empty()),
    })
}

/// The origin of a page (`scheme://host[:port]`), for http(s) pages only.
pub fn page_origin(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return None;
    }
    Some(u.origin().ascii_serialization())
}

/// `GET /v1/status`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub setup: bool,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub tls_fingerprint: Option<String>,
}

/// One account BotPass offers on an origin (no secret).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Account {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub has_totp: bool,
    #[serde(default)]
    pub vault: String,
}

/// What `POST /v1/fill` returns.
#[derive(Clone, PartialEq, Default, Deserialize)]
pub struct Filled {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub code_expires_in: Option<u64>,
}

impl std::fmt::Debug for Filled {
    // Never the password or the code.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Filled")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("code", &self.code.as_ref().map(|_| "<redacted>"))
            .field("code_expires_in", &self.code_expires_in)
            .finish()
    }
}

/// What to fetch for a fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Login,
    Password,
    Otp,
}

impl What {
    fn as_str(self) -> &'static str {
        match self {
            What::Login => "login",
            What::Password => "password",
            What::Otp => "otp",
        }
    }
}

/// A BotPass server and how it is trusted.
#[derive(Clone)]
pub struct Server {
    base: Url,
    trust: Trust,
    http: reqwest::Client,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("url", &self.base.as_str())
            .field("pin", &self.trust.pin())
            .finish()
    }
}

/// The `{"error": …}` of a refusal, else the start of the body.
fn refusal(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| {
            String::from_utf8_lossy(&body[..body.len().min(200)])
                .trim()
                .to_string()
        })
}

impl Server {
    /// A server at `url`, pinned to `fingerprint` when given (ignored for a
    /// loopback `http://` server, which has no certificate).
    pub fn new(url: &str, fingerprint: Option<&str>) -> Result<Server, Error> {
        let base = normalize_url(url)?;
        let pin = match fingerprint.map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) if base.scheme() == "https" => Some(normalize_fingerprint(f)?),
            _ => None,
        };
        let trust = Trust::new(pin.as_deref());
        let http = reqwest::Client::builder()
            .use_preconfigured_tls((*trust.client_config()).clone())
            .no_proxy()
            .user_agent(copper_cloud::tls::user_agent())
            .connect_timeout(CONNECT)
            .timeout(REQUEST)
            .redirect(reqwest::redirect::Policy::none())
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| Error::Usage(format!("HTTP client: {e}")))?;
        Ok(Server { base, trust, http })
    }

    /// The server a pairing names.
    pub fn for_pairing(p: &Pairing) -> Result<Server, Error> {
        Server::new(&p.url, p.fingerprint.as_deref())
    }

    /// `https://host[:port]` with no trailing slash.
    pub fn url(&self) -> String {
        self.base.as_str().trim_end_matches('/').to_string()
    }

    /// The pinned fingerprint.
    pub fn pin(&self) -> Option<&str> {
        self.trust.pin()
    }

    fn endpoint(&self, path: &str) -> Result<Url, Error> {
        self.base
            .join(path)
            .map_err(|e| Error::Usage(format!("{path}: {e}")))
    }

    fn transport(&self, e: reqwest::Error) -> Error {
        if self.trust.mismatched() {
            return Error::Fingerprint {
                seen: self.trust.seen(),
            };
        }
        let failure = self.trust.explain(&e);
        let tls = failure.code == "tls" || failure.message.contains("certificate");
        if tls && self.trust.pin().is_none() && self.trust.seen().is_some() {
            return Error::Untrusted {
                seen: self.trust.seen(),
            };
        }
        Error::Unreachable(failure.message)
    }

    /// One request; the status and the body. Tokens go only in the
    /// `Authorization` header and are never logged.
    async fn send(
        &self,
        method: reqwest::Method,
        url: Url,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<(u16, Vec<u8>), Error> {
        let mut req = self.http.request(method, url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| self.transport(e))?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| self.transport(e))?;
        Ok((status, bytes.to_vec()))
    }

    /// A device call's answer: 2xx parsed, the rest mapped to [`Error`].
    fn answer<T: serde::de::DeserializeOwned>(status: u16, body: &[u8]) -> Result<T, Error> {
        match status {
            200..=299 => serde_json::from_slice(body).map_err(|e| Error::Decode(e.to_string())),
            401 => Err(Error::Unpaired),
            403 => Err(Error::Denied(refusal(body))),
            404 => Err(Error::NotFound(refusal(body))),
            423 => Err(Error::Locked),
            _ => Err(Error::Server {
                status,
                message: refusal(body),
            }),
        }
    }

    /// `GET /v1/status`.
    pub async fn status(&self) -> Result<Status, Error> {
        let (status, body) = self
            .send(
                reqwest::Method::GET,
                self.endpoint("v1/status")?,
                None,
                None,
            )
            .await?;
        Self::answer(status, &body)
    }

    /// Trade a pairing code for a device token (`kind: copper`).
    pub async fn pair(&self, code: &str, name: &str) -> Result<Pairing, Error> {
        let code = code.trim();
        if code.is_empty() {
            return Err(Error::Usage("the pairing code is empty".into()));
        }
        let (status, body) = self
            .send(
                reqwest::Method::POST,
                self.endpoint("v1/devices/pair")?,
                None,
                Some(json!({"code": code, "name": name, "kind": "copper"})),
            )
            .await?;
        if status == 401 {
            return Err(Error::BadCode(refusal(&body)));
        }
        if status == 429 {
            return Err(Error::Server {
                status,
                message: refusal(&body),
            });
        }
        #[derive(Deserialize)]
        struct Device {
            #[serde(default)]
            id: String,
            #[serde(default)]
            name: String,
            #[serde(default)]
            bot_id: Option<String>,
            #[serde(default)]
            personal: bool,
        }
        #[derive(Deserialize)]
        struct Bot {
            #[serde(default)]
            id: String,
            #[serde(default)]
            name: String,
        }
        #[derive(Deserialize)]
        struct Paired {
            device: Device,
            token: String,
            #[serde(default)]
            bot: Option<Bot>,
        }
        let p: Paired = Self::answer(status, &body)?;
        if !p.token.starts_with("bpd_") {
            return Err(Error::Decode("the answer carried no device token".into()));
        }
        let paired_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .ok();
        Ok(Pairing {
            url: self.url(),
            fingerprint: self.trust.pin().map(str::to_string),
            token: p.token,
            device_name: if p.device.name.is_empty() {
                name.to_string()
            } else {
                p.device.name
            },
            device_id: p.device.id,
            bot: p.bot.as_ref().map(|b| b.name.clone()),
            bot_id: p.bot.map(|b| b.id).or(p.device.bot_id),
            personal: p.device.personal,
            paired_at,
            extra: Default::default(),
        })
    }

    /// The accounts this device may use on `origin` (usernames only).
    pub async fn accounts(&self, token: &str, origin: &str) -> Result<Vec<Account>, Error> {
        let mut url = self.endpoint("v1/fill/accounts")?;
        url.query_pairs_mut().append_pair("origin", origin);
        let (status, body) = self
            .send(reqwest::Method::GET, url, Some(token), None)
            .await?;
        #[derive(Deserialize)]
        struct Accounts {
            #[serde(default)]
            accounts: Vec<Account>,
        }
        Ok(Self::answer::<Accounts>(status, &body)?.accounts)
    }

    /// One account's secret for `origin`, fetched now.
    pub async fn fill(
        &self,
        token: &str,
        id: &str,
        origin: &str,
        what: What,
    ) -> Result<Filled, Error> {
        let (status, body) = self
            .send(
                reqwest::Method::POST,
                self.endpoint("v1/fill")?,
                Some(token),
                Some(json!({"id": id, "origin": origin, "what": what.as_str()})),
            )
            .await?;
        Self::answer(status, &body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            normalize_url("bp.example.com:8443/x?y#z").unwrap().as_str(),
            "https://bp.example.com:8443/"
        );
        assert_eq!(
            normalize_url("http://127.0.0.1:17491").unwrap().as_str(),
            "http://127.0.0.1:17491/"
        );
        assert!(normalize_url("http://localhost:1").is_ok());
        assert!(normalize_url("http://[::1]:1").is_ok());
        assert!(normalize_url("http://bp.example.com").is_err());
        assert!(normalize_url("ftp://bp.example.com").is_err());
        assert!(normalize_url("https://u:p@bp.example.com").is_err());
        assert!(normalize_url("  ").is_err());
    }

    #[test]
    fn fingerprints() {
        let hex = "ab".repeat(32);
        let colons = hex
            .as_bytes()
            .chunks(2)
            .map(|c| std::str::from_utf8(c).unwrap().to_uppercase())
            .collect::<Vec<_>>()
            .join(":");
        assert_eq!(normalize_fingerprint(&colons).unwrap(), hex);
        assert_eq!(
            normalize_fingerprint(&format!("sha256:{hex}")).unwrap(),
            hex
        );
        assert!(normalize_fingerprint("abcd").is_err());
        assert!(normalize_fingerprint(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn links() {
        let l = parse_link("botpass://bp.lan:7443/#c=ABCD-EFGH&fp=abcd").unwrap();
        assert_eq!(l.server, "https://bp.lan:7443");
        assert_eq!(l.code, "ABCD-EFGH");
        assert_eq!(l.fingerprint.as_deref(), Some("abcd"));
        let l = parse_link("botpass://bp.lan/#c=X").unwrap();
        assert_eq!(l.fingerprint, None);
        assert_eq!(parse_link("botpass://bp.lan/#fp=x"), None);
        assert_eq!(parse_link("https://bp.lan/#c=X"), None);
        assert_eq!(parse_link("ABCD-EFGH"), None);
    }

    #[test]
    fn origins() {
        assert_eq!(
            page_origin("https://www.Example.com/login?x=1").as_deref(),
            Some("https://www.example.com")
        );
        assert_eq!(
            page_origin("http://localhost:8765/a").as_deref(),
            Some("http://localhost:8765")
        );
        assert_eq!(
            page_origin("https://example.com:443/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(page_origin("about:blank"), None);
        assert_eq!(page_origin("file:///tmp/x"), None);
    }

    #[test]
    fn filled_debug_hides_secrets() {
        let f = Filled {
            id: "i".into(),
            username: Some("ann".into()),
            password: Some("hunter2".into()),
            code: Some("123456".into()),
            code_expires_in: Some(10),
        };
        let text = format!("{f:?}");
        assert!(
            !text.contains("hunter2") && !text.contains("123456"),
            "{text}"
        );
    }

    #[test]
    fn http_pins_are_ignored_https_pins_checked() {
        let s = Server::new("http://127.0.0.1:1", Some(&"ab".repeat(32))).unwrap();
        assert_eq!(s.pin(), None);
        let s = Server::new("https://127.0.0.1:1", Some(&"AB".repeat(32))).unwrap();
        assert_eq!(s.pin(), Some("ab".repeat(32).as_str()));
        assert!(Server::new("https://127.0.0.1:1", Some("nope")).is_err());
    }
}
