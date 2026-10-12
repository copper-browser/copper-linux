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

/// A link code (`#k=`), never a pairing code.
pub fn parse_link_code(text: &str) -> Option<Link> {
    match parse_code(text, None) {
        Some(Code::Link(link)) => Some(link),
        _ => None,
    }
}

/// A link code (`#k=`) or a pairing code (`#p=`), whichever was pasted. A
/// bare `cp_…` pairing code reads only with `linked_to`, the instance this
/// Copper already knows.
pub fn parse_code(text: &str, linked_to: Option<&Link>) -> Option<Code> {
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
}
