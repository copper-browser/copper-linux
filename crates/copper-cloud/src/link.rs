//! Link codes, pairing codes and the manual link (Mac: `Cloud.parseCode`,
//! `Cloud.link(address:key:fingerprint:)`, `Cloud.endpoint`).
//!
//! * A **link code** — `copper-cloud://HOST:PORT/#k=KEY&fp=HEX` — is what an
//!   instance's admin hands out (`copper-cloud link-code`, or a person's
//!   access key from the portal). It connects; signing in comes next.
//! * A **pairing code** — `copper-cloud://HOST:PORT/#p=cp_…&fp=HEX` — is minted
//!   by a Copper already signed in and links *and* signs in another one
//!   (`POST /v1/auth/pair`). A bare `cp_…` reads only against the instance
//!   this Copper is already linked to.
//! * A **sign-in link** — `copper-cloud://HOST:PORT/#sso=google&fp=HEX` — is
//!   an instance's onboarding link for company single sign-on: no secret in
//!   it, the identity provider's domain check is the gate. It links *and*
//!   signs in through the system browser ([`crate::sso`]). [`parse_code`]
//!   never returns one (its two kinds stay as they are); [`parse_join_code`]
//!   reads all three.
//! * A **deep link** — `copper://cloud/join?code=<percent-encoded code>` — is
//!   how a web page (the instance's `/start`) hands any of them to Copper.
//!   Every parser here unwraps it first.
//!
//! Spaces and line breaks a paste picked up are ignored; colons in the
//! fingerprint are allowed. `http://` (or `tls=off` in the fragment) is only
//! ever accepted for a loopback host, and is never pinned. A bad fingerprint
//! is refused rather than dropped: dropping it would silently mean "trust
//! whatever answers".

use copper_core::cloud::CloudLink;
use std::fmt;

/// Where an instance is: `https://host[:port]`, or `http://` for loopback.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    /// `https` (false only for a loopback development server).
    pub https: bool,
    /// Host name or IP literal, IPv6 without brackets.
    pub host: String,
    /// The port, when the code said one (kept as written, even 443).
    pub port: Option<u16>,
}

impl Endpoint {
    /// `https://host[:port]` — no trailing slash, the way the Mac writes
    /// `cloud.json`'s `link.url`.
    pub fn base(&self) -> String {
        format!(
            "{}://{}",
            if self.https { "https" } else { "http" },
            self.host_text()
        )
    }

    /// `host[:port]` as people read it, IPv6 in brackets (Mac: `hostText`).
    pub fn host_text(&self) -> String {
        let shown = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match self.port {
            Some(p) => format!("{shown}:{p}"),
            None => shown,
        }
    }

    /// The port a connection goes to.
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(if self.https { 443 } else { 80 })
    }

    /// `base` + `/path` (a leading `/` on `path` is optional).
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base(), path.trim_start_matches('/'))
    }

    /// The address is this machine (`127.0.0.1`, `localhost`, `::1`).
    pub fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.base())
    }
}

/// Where the instance is and how to know it is the one meant (Mac:
/// `Cloud.Link`).
#[derive(Clone, PartialEq, Eq)]
pub struct Link {
    pub endpoint: Endpoint,
    /// The instance key or access key every `/v1` request carries
    /// (`X-Copper-Instance`).
    pub key: String,
    /// SHA-256 of the server's leaf certificate (DER), lowercase hex. `None`
    /// only for loopback `http://`; an `https` link without one cannot be
    /// used until a fingerprint is confirmed (see `CloudClient::connect`).
    pub fingerprint: Option<String>,
}

impl fmt::Debug for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Link")
            .field("endpoint", &self.endpoint.base())
            .field("key", &"<redacted>")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

impl Link {
    /// `host:port` as people read it.
    pub fn host(&self) -> String {
        self.endpoint.host_text()
    }

    /// The fingerprint in pairs, `ab:cd:…`, for comparing by eye.
    pub fn fingerprint_pairs(&self) -> Option<String> {
        self.fingerprint.as_deref().map(pairs)
    }

    /// The code that would make this link again.
    pub fn code(&self) -> String {
        code_text(
            &self.endpoint,
            &format!("k={}", self.key),
            self.fingerprint.as_deref(),
        )
    }

    /// As kept in `cloud.json`.
    pub fn to_saved(&self) -> CloudLink {
        CloudLink {
            url: self.endpoint.base(),
            key: self.key.clone(),
            fingerprint: self.fingerprint.clone(),
            extra: Default::default(),
        }
    }

    /// From `cloud.json`; `None` if what is kept is not usable.
    pub fn from_saved(saved: &CloudLink) -> Option<Link> {
        manual_link(&saved.url, &saved.key, saved.fingerprint.as_deref())
    }
}

/// A pairing code: where the instance is and the one-time credential (Mac:
/// `Cloud.PairingCode`). Never kept on disk.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub endpoint: Endpoint,
    /// `cp_…`, single use, ten minutes.
    pub code: String,
    /// As in a link code.
    pub fingerprint: Option<String>,
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingCode")
            .field("endpoint", &self.endpoint.base())
            .field("code", &"<redacted>")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
}

impl PairingCode {
    /// `host:port` as people read it.
    pub fn host(&self) -> String {
        self.endpoint.host_text()
    }

    /// The whole code, as the minting Copper shows it.
    pub fn link(&self) -> String {
        code_text(
            &self.endpoint,
            &format!("p={}", self.code),
            self.fingerprint.as_deref(),
        )
    }
}

/// What a pasted code turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Code {
    /// `#k=`: connects; signing in comes next.
    Link(Link),
    /// `#p=`: connects and signs in, in one step.
    Pairing(PairingCode),
}

/// A sign-in link (`#sso=<provider>`): where the instance is, which of its
/// identity providers to use, and the pin. Holds no secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsoLink {
    pub endpoint: Endpoint,
    /// The provider's id on the instance (`google`), lowercase.
    pub provider: String,
    /// As in a link code.
    pub fingerprint: Option<String>,
}

impl SsoLink {
    /// `host:port` as people read it.
    pub fn host(&self) -> String {
        self.endpoint.host_text()
    }

    /// The whole link, as the instance prints it.
    pub fn link(&self) -> String {
        code_text(
            &self.endpoint,
            &format!("sso={}", self.provider),
            self.fingerprint.as_deref(),
        )
    }
}

/// Anything that joins this Copper to an instance: a link code, a pairing
/// code or a sign-in link (Settings › Cloud's field, `copper cloud pair`, a
/// `copper://cloud/join` deep link).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinCode {
    /// `#k=`: connects; signing in comes next.
    Link(Link),
    /// `#p=`: connects and signs in, in one step.
    Pairing(PairingCode),
    /// `#sso=`: connects and signs in through the system browser.
    Sso(SsoLink),
}

impl From<Code> for JoinCode {
    fn from(code: Code) -> Self {
        match code {
            Code::Link(l) => JoinCode::Link(l),
            Code::Pairing(p) => JoinCode::Pairing(p),
        }
    }
}

/// A link code, a pairing code or a sign-in link, whichever was pasted (or
/// carried by a `copper://cloud/join` deep link). A bare `cp_…` reads only
/// with `linked_to`, as in [`parse_code`].
pub fn parse_join_code(text: &str, linked_to: Option<&Link>) -> Option<JoinCode> {
    if let Some(code) = parse_code(text, linked_to) {
        return Some(code.into());
    }
    parse_sso_link(text).map(JoinCode::Sso)
}

/// A sign-in link: `copper-cloud://HOST:PORT/#sso=<provider>[&fp=HEX]` (or
/// loopback `http://…/#sso=…`, or wrapped in a `copper://cloud/join` deep
/// link). The provider id is letters, digits, `-` and `_`, at most 64.
pub fn parse_sso_link(text: &str) -> Option<SsoLink> {
    let raw = unwrap_deep_link(text);
    let (address, fields) = split_code(&raw)?;
    if fields.get("k").is_some_and(|k| !k.is_empty())
        || fields.get("p").is_some_and(|p| !p.is_empty())
    {
        return None;
    }
    let provider = fields.get("sso")?.trim().to_ascii_lowercase();
    if provider.is_empty()
        || provider.len() > 64
        || !provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let (endpoint, pin) = endpoint(&address, fields.get("fp").map(String::as_str))?;
    Some(SsoLink {
        endpoint,
        provider,
        fingerprint: pin,
    })
}

/// The code a `copper://cloud/join?code=<percent-encoded code>` deep link
/// carries (`copper:cloud/join?…` and `copper:///cloud/join?…` too). `None`
/// for anything else, including a `copper://` address that isn't a join.
pub fn deep_link_code(text: &str) -> Option<String> {
    let raw = text.trim();
    let rest = strip_prefix_ignore_case(raw, "copper:")?;
    let rest = rest.trim_start_matches('/');
    let (path, query) = rest.split_once('?')?;
    if !path
        .trim_end_matches('/')
        .eq_ignore_ascii_case("cloud/join")
    {
        return None;
    }
    // Everything after `code=` is the code: a code that wasn't encoded
    // (`…/#p=…&fp=…` pasted as is) still reads whole, and a field after an
    // encoded one only adds a field the code's parser ignores.
    let at = query
        .split('&')
        .scan(0usize, |offset, part| {
            let here = *offset;
            *offset += part.len() + 1;
            Some((here, part))
        })
        .find(|(_, part)| part.len() >= 5 && part[..5].eq_ignore_ascii_case("code="))
        .map(|(at, _)| at)?;
    let value = &query[at + 5..];
    let value = value.replace('+', "%20");
    let code = percent_decode(&value).unwrap_or(value);
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    (!code.is_empty()).then_some(code)
}

/// `text` itself, or the code inside it when it is a deep link.
fn unwrap_deep_link(text: &str) -> String {
    deep_link_code(text).unwrap_or_else(|| text.to_string())
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    (text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix))
        .then(|| &text[prefix.len()..])
}

/// The address part (`copper-cloud://` rewritten to `https://`, or `http://`
/// with `tls=off`) and the fragment's fields, keys lowercased, values
/// percent-decoded, the last of a repeated key winning. Spaces and line
/// breaks are dropped first.
fn split_code(text: &str) -> Option<(String, std::collections::BTreeMap<String, String>)> {
    let raw: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let (head, fragment) = raw.split_once('#')?;
    let mut fields = std::collections::BTreeMap::new();
    for pair in fragment.split('&').filter(|p| !p.is_empty()) {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let value = percent_decode(v).unwrap_or_else(|| v.to_string());
        fields.insert(k.to_lowercase(), value);
    }
    let plain = matches!(
        fields.get("tls").map(|t| t.to_lowercase()).as_deref(),
        Some("off" | "none" | "plain" | "http")
    );
    let mut address = head.to_string();
    if address.len() >= 15 && address[..15].eq_ignore_ascii_case("copper-cloud://") {
        address = format!(
            "{}{}",
            if plain { "http://" } else { "https://" },
            &address[15..]
        );
    }
    Some((address, fields))
}

/// A link code (`#k=`), never a pairing code.
pub fn parse_link_code(text: &str) -> Option<Link> {
    match parse_code(text, None) {
        Some(Code::Link(link)) => Some(link),
        _ => None,
    }
}

/// A link code (`#k=`) or a pairing code (`#p=`), whichever was pasted. A
/// bare `cp_…` pairing code reads only with `linked_to`, the instance this
/// Copper already knows. A `copper://cloud/join` deep link reads as the
/// code it carries. A sign-in link (`#sso=`) is neither: see
/// [`parse_join_code`].
pub fn parse_code(text: &str, linked_to: Option<&Link>) -> Option<Code> {
    let text = unwrap_deep_link(text);
    let raw: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(current) = linked_to
        && is_bare_pairing_code(&raw)
    {
        return Some(Code::Pairing(PairingCode {
            endpoint: current.endpoint.clone(),
            code: raw,
            fingerprint: current.fingerprint.clone(),
        }));
    }
    let (head, fragment) = raw.split_once('#')?;
    let mut fields: Vec<(String, String)> = Vec::new();
    for pair in fragment.split('&').filter(|p| !p.is_empty()) {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        let value = percent_decode(v).unwrap_or_else(|| v.to_string());
        let key = k.to_lowercase();
        // The last one wins, as a Swift dictionary assignment does.
        fields.retain(|(name, _)| *name != key);
        fields.push((key, value));
    }
    let field = |name: &str| {
        fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let plain = matches!(
        field("tls").map(str::to_lowercase).as_deref(),
        Some("off" | "none" | "plain" | "http")
    );
    let mut address = head.to_string();
    if address.len() >= 15 && address[..15].eq_ignore_ascii_case("copper-cloud://") {
        address = format!(
            "{}{}",
            if plain { "http://" } else { "https://" },
            &address[15..]
        );
    }
    if let Some(key) = field("k").or_else(|| field("key"))
        && !key.is_empty()
    {
        return manual_link(&address, key, field("fp")).map(Code::Link);
    }
    if let Some(code) = field("p").or_else(|| field("pair"))
        && !code.is_empty()
        && !code
            .chars()
            .any(|c| c.is_whitespace() || c == '/' || c == '?')
    {
        let (endpoint, pin) = endpoint(&address, field("fp"))?;
        return Some(Code::Pairing(PairingCode {
            endpoint,
            code: code.to_string(),
            fingerprint: pin,
        }));
    }
    None
}

/// `cp_` and base64url: a pairing code on its own, without the address.
pub fn is_bare_pairing_code(text: &str) -> bool {
    let raw = text.trim();
    let Some(rest) = raw.strip_prefix("cp_") else {
        return false;
    };
    raw.chars().count() >= 19
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A link from the advanced fields: an address (`host:port` or a URL), the
/// key, and an optional fingerprint (Mac: `Cloud.link(address:key:fingerprint:)`).
/// `None` when one of them is not usable.
pub fn manual_link(address: &str, key: &str, fingerprint: Option<&str>) -> Option<Link> {
    let key = key.trim();
    if key.is_empty() || key.chars().any(char::is_whitespace) {
        return None;
    }
    let (endpoint, pin) = endpoint(address, fingerprint)?;
    Some(Link {
        endpoint,
        key: key.to_string(),
        fingerprint: pin,
    })
}

/// The address and pin both kinds of code share (Mac: `Cloud.endpoint`):
/// `https://host[:port]` (or loopback `http://`, never pinned) and the
/// fingerprint as lowercase hex. `None` when either is unusable.
pub fn endpoint(address: &str, fingerprint: Option<&str>) -> Option<(Endpoint, Option<String>)> {
    let mut address = address.trim().to_string();
    if address.is_empty() {
        return None;
    }
    if !address.contains("://") {
        address = format!("https://{address}");
    }
    let (scheme, rest) = address.split_once("://")?;
    let https = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // user:password@ is not part of where the instance is.
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let (host, port) = split_host_port(authority)?;
    if host.is_empty() || url::Host::parse(&bracket(&host)).is_err() {
        return None;
    }
    // Plain HTTP only to this machine: a development server with `tls off`.
    if !https && !is_loopback_host(&host) {
        return None;
    }
    let mut pin = None;
    if let Some(fp) = fingerprint
        && !fp.trim().is_empty()
    {
        pin = Some(normalize_fingerprint(fp)?);
    }
    Some((
        Endpoint { https, host, port },
        if https { pin } else { None },
    ))
}

/// `AB:CD:…` or plain hex, any case, spaces ignored → 64 lowercase hex.
pub fn normalize_fingerprint(raw: &str) -> Option<String> {
    let hex: String = raw
        .chars()
        .filter(|c| *c != ':' && !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// A fingerprint in pairs, `ab:cd:…`, for comparing by eye.
pub fn pairs(hex: &str) -> String {
    hex.as_bytes()
        .chunks(2)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(":")
}

/// `copper-cloud://host:port/#<fragment>[&fp=…]`, or the loopback `http://`
/// form (Mac: `Cloud.codeText`).
pub fn code_text(endpoint: &Endpoint, fragment: &str, fingerprint: Option<&str>) -> String {
    let mut fragment = fragment.to_string();
    if let Some(fp) = fingerprint {
        fragment.push_str("&fp=");
        fragment.push_str(fp);
    }
    if !endpoint.https {
        return format!("{}/#{fragment}", endpoint.base());
    }
    format!("copper-cloud://{}/#{fragment}", endpoint.host_text())
}

fn is_loopback_host(host: &str) -> bool {
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

fn bracket(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// `host`, `host:port`, `[v6]`, `[v6]:port`. A port that isn't a number in
/// 1…65535 makes the whole address unusable.
fn split_host_port(authority: &str) -> Option<(String, Option<u16>)> {
    let port = |text: &str| -> Option<Option<u16>> {
        if text.is_empty() {
            return Some(None);
        }
        match text.parse::<u16>() {
            Ok(0) | Err(_) => None,
            Ok(p) => Some(Some(p)),
        }
    };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let p = match after.strip_prefix(':') {
            Some(text) => port(text)?,
            None if after.is_empty() => None,
            None => return None,
        };
        return Some((host.to_string(), p));
    }
    match authority.matches(':').count() {
        0 => Some((authority.to_string(), None)),
        1 => {
            let (host, text) = authority.split_once(':')?;
            Some((host.to_string(), port(text)?))
        }
        // A bare IPv6 literal.
        _ => Some((authority.to_string(), None)),
    }
}

fn percent_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_string());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    const KEY: &str = "ck_Zm9vYmFyYmF6cXV4cXV1eGNvcmdlZ3JhdWx0Z2FycGx5";

    #[test]
    fn link_code_round_trip() {
        let text = format!("copper-cloud://203.0.113.10:443/#k={KEY}&fp={FP}");
        let link = parse_link_code(&text).expect("link");
        assert_eq!(link.endpoint.base(), "https://203.0.113.10:443");
        assert_eq!(link.key, KEY);
        assert_eq!(link.fingerprint.as_deref(), Some(FP));
        assert_eq!(link.code(), text);
        assert_eq!(link.host(), "203.0.113.10:443");
        assert_eq!(link.fingerprint_pairs().unwrap().len(), 64 + 31);
        assert!(link.fingerprint_pairs().unwrap().starts_with("9f:86:d0"));
    }

    #[test]
    fn paste_noise_colons_and_case() {
        let upper = pairs(FP).to_uppercase();
        let text = format!(" Copper-Cloud://cloud.example.com:8443/\n#k={KEY}&fp={upper}  ");
        let link = parse_link_code(&text).expect("link");
        assert_eq!(link.endpoint.base(), "https://cloud.example.com:8443");
        assert_eq!(link.fingerprint.as_deref(), Some(FP));
        // `key=` is the long spelling.
        let link = parse_link_code(&format!("copper-cloud://h:1/#key={KEY}")).unwrap();
        assert_eq!(link.key, KEY);
        assert_eq!(link.fingerprint, None);
        // No port: kept as no port.
        let link = parse_link_code(&format!("copper-cloud://h/#k={KEY}&fp={FP}")).unwrap();
        assert_eq!(link.endpoint.base(), "https://h");
        assert_eq!(link.endpoint.effective_port(), 443);
    }

    #[test]
    fn bad_fingerprint_is_refused_not_dropped() {
        assert!(parse_link_code(&format!("copper-cloud://h:1/#k={KEY}&fp=abc")).is_none());
        assert!(
            parse_link_code(&format!("copper-cloud://h:1/#k={KEY}&fp={}g", &FP[..63])).is_none()
        );
        assert!(manual_link("h:1", KEY, Some("zz")).is_none());
        // Empty fp is "none given".
        assert!(manual_link("h:1", KEY, Some("  ")).is_some());
    }

    #[test]
    fn plain_http_only_for_loopback_and_never_pinned() {
        let link = parse_link_code(&format!(
            "copper-cloud://127.0.0.1:8080/#k={KEY}&tls=off&fp={FP}"
        ))
        .unwrap();
        assert_eq!(link.endpoint.base(), "http://127.0.0.1:8080");
        assert_eq!(link.fingerprint, None);
        assert_eq!(link.code(), format!("http://127.0.0.1:8080/#k={KEY}"));
        assert!(
            parse_link_code(&format!("copper-cloud://10.0.0.1:8080/#k={KEY}&tls=off")).is_none()
        );
        assert!(parse_link_code(&format!("http://example.com/#k={KEY}")).is_none());
        let link = parse_link_code(&format!("http://localhost:8443/#k={KEY}")).unwrap();
        assert!(!link.endpoint.https);
    }

    #[test]
    fn pairing_codes() {
        let text = format!(
            "copper-cloud://cloud.example.com:443/#p=cp_Q2bxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx&fp={FP}"
        );
        let Some(Code::Pairing(code)) = parse_code(&text, None) else {
            panic!("pairing")
        };
        assert_eq!(code.code, "cp_Q2bxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx");
        assert_eq!(code.fingerprint.as_deref(), Some(FP));
        assert_eq!(code.link(), text);
        assert!(parse_link_code(&text).is_none());
        // Bare: only with a link to read it against.
        let bare = "cp_Q2bxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        assert!(is_bare_pairing_code(bare));
        assert!(!is_bare_pairing_code("cp_short"));
        assert!(!is_bare_pairing_code("cp_has/slash_xxxxxxxxxxxx"));
        assert!(parse_code(bare, None).is_none());
        let linked = parse_link_code(&format!("copper-cloud://h:9/#k={KEY}&fp={FP}")).unwrap();
        let Some(Code::Pairing(code)) = parse_code(bare, Some(&linked)) else {
            panic!("bare pairing")
        };
        assert_eq!(code.endpoint, linked.endpoint);
        assert_eq!(code.fingerprint.as_deref(), Some(FP));
        // `pair=` spelling, and a code with a slash is not one.
        assert!(matches!(
            parse_code(&format!("copper-cloud://h:9/#pair=cp_abc&fp={FP}"), None),
            Some(Code::Pairing(_))
        ));
        assert!(parse_code("copper-cloud://h:9/#p=cp_a/b", None).is_none());
    }

    #[test]
    fn ipv6_and_bad_addresses() {
        let link = parse_link_code(&format!(
            "copper-cloud://[2001:db8::1]:8443/#k={KEY}&fp={FP}"
        ))
        .unwrap();
        assert_eq!(link.endpoint.host, "2001:db8::1");
        assert_eq!(link.endpoint.base(), "https://[2001:db8::1]:8443");
        assert_eq!(
            link.code(),
            format!("copper-cloud://[2001:db8::1]:8443/#k={KEY}&fp={FP}")
        );
        assert!(parse_link_code(&format!("copper-cloud://h:0/#k={KEY}")).is_none());
        assert!(parse_link_code(&format!("copper-cloud://h:99999/#k={KEY}")).is_none());
        assert!(parse_link_code(&format!("copper-cloud:///#k={KEY}")).is_none());
        assert!(parse_link_code(&format!("ftp://h/#k={KEY}")).is_none());
        assert!(parse_link_code("copper-cloud://h:1/#fp=00").is_none());
        assert!(parse_link_code("no fragment").is_none());
        // The manual form: an address and a key.
        let link = manual_link("cloud.example.com:8443/some/path", KEY, Some(FP)).unwrap();
        assert_eq!(link.endpoint.base(), "https://cloud.example.com:8443");
        assert!(manual_link("h", "a key", None).is_none());
    }

    #[test]
    fn saved_round_trip() {
        let link =
            parse_link_code(&format!("copper-cloud://h.example:8443/#k={KEY}&fp={FP}")).unwrap();
        let saved = link.to_saved();
        assert_eq!(saved.url, "https://h.example:8443");
        assert_eq!(Link::from_saved(&saved), Some(link));
    }

    #[test]
    fn percent_encoded_fields() {
        let link = parse_link_code(&format!("copper-cloud://h:1/#k=ck%5Fabc&fp={FP}")).unwrap();
        assert_eq!(link.key, "ck_abc");
    }

    #[test]
    fn sso_links() {
        let text = format!("copper-cloud://cloud.example.com:8443/#sso=google&fp={FP}");
        let sso = parse_sso_link(&text).expect("sso link");
        assert_eq!(sso.endpoint.base(), "https://cloud.example.com:8443");
        assert_eq!(sso.provider, "google");
        assert_eq!(sso.fingerprint.as_deref(), Some(FP));
        assert_eq!(sso.link(), text);
        assert_eq!(sso.host(), "cloud.example.com:8443");
        // Not a link code or a pairing code: the old parser stays as it was.
        assert!(parse_code(&text, None).is_none());
        assert!(parse_link_code(&text).is_none());
        assert_eq!(parse_join_code(&text, None), Some(JoinCode::Sso(sso)));
        // ACME: no pin. Paste noise, case, colons in the pin.
        let sso = parse_sso_link("  Copper-Cloud://copper.example.com/\n#SSO=Google ").unwrap();
        assert_eq!(sso.provider, "google");
        assert_eq!(sso.fingerprint, None);
        assert_eq!(sso.link(), "copper-cloud://copper.example.com/#sso=google");
        let sso = parse_sso_link(&format!(
            "copper-cloud://h:1/#sso=corp-okta_2&fp={}",
            pairs(FP).to_uppercase()
        ))
        .unwrap();
        assert_eq!(sso.provider, "corp-okta_2");
        assert_eq!(sso.fingerprint.as_deref(), Some(FP));
        // Loopback development server, never pinned.
        let sso = parse_sso_link(&format!("http://127.0.0.1:8080/#sso=google&fp={FP}")).unwrap();
        assert!(!sso.endpoint.https);
        assert_eq!(sso.fingerprint, None);
        assert_eq!(sso.link(), "http://127.0.0.1:8080/#sso=google");
    }

    #[test]
    fn bad_sso_links() {
        for text in [
            "copper-cloud://h:1/#sso=".to_string(),
            "copper-cloud://h:1/#sso=goo/gle".to_string(),
            "copper-cloud://h:1/#sso=google&fp=abc".to_string(),
            format!("copper-cloud://h:1/#sso={}", "a".repeat(65)),
            "copper-cloud://10.0.0.1:1/#sso=google&tls=off".to_string(),
            "copper-cloud://h:0/#sso=google".to_string(),
            "copper-cloud://h:1/sso=google".to_string(),
        ] {
            assert!(parse_sso_link(&text).is_none(), "{text}");
        }
        // A key or a pairing code beside it: that code wins.
        let both = format!("copper-cloud://h:1/#k={KEY}&sso=google");
        assert!(parse_sso_link(&both).is_none());
        assert!(matches!(
            parse_join_code(&both, None),
            Some(JoinCode::Link(_))
        ));
        assert!(matches!(
            parse_join_code("copper-cloud://h:1/#p=cp_abc&sso=google", None),
            Some(JoinCode::Pairing(_))
        ));
    }

    #[test]
    fn deep_links() {
        let sso = format!("copper-cloud://cloud.example.com:8443/#sso=google&fp={FP}");
        let encoded = sso
            .replace(':', "%3A")
            .replace('/', "%2F")
            .replace('#', "%23")
            .replace('=', "%3D")
            .replace('&', "%26");
        let deep = format!("copper://cloud/join?code={encoded}");
        assert_eq!(deep_link_code(&deep).as_deref(), Some(sso.as_str()));
        assert!(matches!(
            parse_join_code(&deep, None),
            Some(JoinCode::Sso(s)) if s.provider == "google" && s.fingerprint.as_deref() == Some(FP)
        ));
        // Other spellings of the same thing; a code that wasn't encoded.
        for form in [
            format!("COPPER://cloud/join/?code={encoded}"),
            format!("copper:cloud/join?code={encoded}"),
            format!("copper:///cloud/join?from=start&code={encoded}"),
            format!("copper://cloud/join?code={sso}"),
        ] {
            assert_eq!(
                deep_link_code(&form).as_deref(),
                Some(sso.as_str()),
                "{form}"
            );
        }
        // Pairing and link codes come through the same door.
        let pairing = format!("copper-cloud://h:9/#p=cp_Q2bxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx&fp={FP}");
        let deep = format!(
            "copper://cloud/join?code={}",
            pairing.replace('#', "%23").replace('&', "%26")
        );
        assert!(matches!(parse_code(&deep, None), Some(Code::Pairing(_))));
        let link = format!("copper-cloud://h:9/#k={KEY}&fp={FP}");
        let deep = format!("copper://cloud/join?code={}", link.replace('#', "%23"));
        assert_eq!(parse_link_code(&deep).map(|l| l.key), Some(KEY.to_string()));
        // Not joins.
        for text in [
            "copper://canvas/personal",
            "copper://cloud/join",
            "copper://cloud/join?code=",
            "copper://cloud/settings?code=x",
            "https://cloud/join?code=x",
        ] {
            assert_eq!(deep_link_code(text), None, "{text}");
        }
    }
}
