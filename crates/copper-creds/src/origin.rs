//! Where a saved login belongs (Mac `Fork/Credentials/Origin.swift`): an
//! origin — scheme, host and port, as a document's `location.origin` writes
//! it. What Copper fills without a person choosing (an agent's sign-in, a
//! custom field) goes only where that origin is. The list a person picks
//! from stays wider (the site and its subdomains): they see the address and
//! choose the row. Pure.

use url::Url;

use crate::credential::Credential;

/// `https://example.com`, `http://127.0.0.1:8795`: lowercased, no trailing
/// dot, the scheme's usual port left out. `None` without a scheme or host.
pub fn origin(scheme: &str, host: &str, port: Option<u16>) -> Option<String> {
    let scheme = scheme.to_lowercase();
    let mut host = host.to_lowercase();
    while host.ends_with('.') {
        host.pop();
    }
    if scheme.is_empty() || host.is_empty() {
        return None;
    }
    if host.contains(':') && !host.starts_with('[') {
        host = format!("[{host}]");
    }
    let usual = match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    match port {
        Some(p) if p != 0 && Some(p) != usual => Some(format!("{scheme}://{host}:{p}")),
        _ => Some(format!("{scheme}://{host}")),
    }
}

/// The origin of an address. `None` for one with no host (a file, `about:blank`).
pub fn origin_of(url: &str) -> Option<String> {
    let u = Url::parse(url.trim()).ok()?;
    origin(u.scheme(), u.host_str()?, u.port())
}

/// Whether a login's saved address (`site`: a URL, or a bare host as the
/// keyring keeps it) is exactly `origin`: not another subdomain —
/// `www.example.com` is not `example.com` unless the item lists both — not
/// another port, not http for https.
///
/// A bare host says no scheme or port, so it is https on its usual port (or
/// the port written after it). A bare loopback host (`localhost`,
/// `127.0.0.1`) owns nothing: any server on this computer answers there, on
/// any port, so it needs the origin it was saved from.
pub fn saved(site: &str, origin: &str) -> bool {
    let raw = site.trim();
    let Some(there) = origin_of(origin).filter(|_| !raw.is_empty()) else {
        return false;
    };
    if raw.contains("://") {
        return origin_of(raw).is_some_and(|mine| mine == there);
    }
    let Ok(u) = Url::parse(&format!("https://{raw}")) else {
        return false;
    };
    let Some(host) = u.host_str().filter(|h| !loopback(h)) else {
        return false;
    };
    self::origin("https", host, u.port()).is_some_and(|mine| mine == there)
}

/// `localhost`, `*.localhost`, 127.0.0.0/8 and `::1`: this computer.
pub fn loopback(host: &str) -> bool {
    let host = host.to_lowercase();
    let host = host.trim_matches(['[', ']']);
    // 127.0.0.0/8 as an address only: "127.example.com" is a name anyone can register.
    let octets: Vec<&str> = host.split('.').collect();
    let v4 = octets.len() == 4 && octets.iter().all(|o| o.parse::<u8>().is_ok());
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || (v4 && octets[0] == "127")
}

/// Whether an agent may fill `credential` into a document of `origin`: one
/// of its saved addresses is that origin (a Bitwarden address marked
/// "never" is not one of them).
pub fn owns(credential: &Credential, origin: &str) -> bool {
    credential.sites.iter().any(|s| saved(s, origin))
}

/// The keyring attributes for where a login was saved from (Mac
/// `Vault.place`): `protocol` and `port`, for an http(s) origin only.
pub fn place(origin: Option<&str>) -> Vec<(&'static str, String)> {
    let Some(u) = origin.and_then(|o| Url::parse(o).ok()) else {
        return Vec::new();
    };
    match u.scheme() {
        s @ ("http" | "https") => vec![
            ("protocol", s.to_string()),
            ("port", u.port_or_known_default().unwrap_or(0).to_string()),
        ],
        _ => Vec::new(),
    }
}

/// Back from those attributes (Mac `Vault.origin(of:host:)`). `None` when
/// there is no http(s) protocol: saved before they were kept.
pub fn placed(protocol: Option<&str>, port: Option<&str>, host: &str) -> Option<String> {
    let scheme = protocol.filter(|p| matches!(*p, "http" | "https"))?;
    origin(scheme, host, port.and_then(|p| p.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential::{AgentHint, CredentialId, Source};

    #[test]
    fn an_origin_is_scheme_host_and_port_as_location_origin_writes_it() {
        assert_eq!(
            origin_of("https://Example.com/login?x=1").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("https://example.com:443/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8795/c1/login.html").as_deref(),
            Some("http://127.0.0.1:8795")
        );
        assert_eq!(origin_of("about:blank"), None);
    }

    #[test]
    fn a_saved_address_owns_its_own_origin_and_no_other() {
        assert!(saved("https://example.com/login", "https://example.com"));
        assert!(saved("https://www.example.com/", "https://www.example.com"));
        assert!(saved("https://Example.com:443", "https://example.com"));
        // Not another port, scheme, subdomain (www. included) or site.
        assert!(!saved("https://www.example.com", "https://example.com"));
        assert!(!saved("https://example.com", "https://www.example.com"));
        assert!(!saved("www.example.com", "https://example.com"));
        assert!(!saved("http://127.0.0.1:8795", "http://127.0.0.1:8796"));
        assert!(!saved("https://example.com", "http://example.com"));
        assert!(!saved("https://example.com", "https://evil.example.com"));
        assert!(!saved(
            "https://accounts.example.com",
            "https://example.com"
        ));
        assert!(!saved(
            "https://example.com.evil.test",
            "https://example.com"
        ));
        // A bare host (a keyring login saved before scheme and port were
        // kept) is https on its usual port. A bare loopback host owns no
        // origin at all: it needs the scheme and port it was saved from.
        assert!(!saved("localhost", "http://localhost:4170"));
        assert!(!saved("127.0.0.1", "http://127.0.0.1:8795"));
        assert!(!saved("127.0.0.1", "https://127.0.0.1"));
        assert!(!saved("localhost:4170", "http://localhost:4170"));
        assert!(!saved("[::1]", "http://[::1]:8080"));
        assert!(saved(
            "http://localhost:4170/login",
            "http://localhost:4170"
        ));
        assert!(saved("example.com", "https://example.com"));
        assert!(saved("www.example.com", "https://www.example.com"));
        assert!(!saved("example.com", "https://www.example.com"));
        assert!(!saved("example.com", "http://example.com"));
        assert!(!saved("example.com", "https://example.com:8443"));
        assert!(!saved("example.com", "https://login.example.com"));
        assert!(saved("example.com:8443", "https://example.com:8443"));
        assert!(!saved("example.com:8443", "https://example.com"));
        assert!(!saved("example.com:8443", "http://example.com:8443"));
        assert!(!saved("", "https://example.com"));
        // Saved from an http page or another port, it says so, and only there.
        assert!(saved("http://example.com", "http://example.com"));
        assert!(!saved("http://example.com", "https://example.com"));
        assert!(!saved("http://127.0.0.1:8796", "http://127.0.0.1:8795"));
        assert!(loopback("[::1]") && loopback("app.localhost") && !loopback("127.example.com"));
    }

    fn login(sites: &[&str]) -> Credential {
        Credential {
            id: CredentialId::Keychain {
                host: "example.com".into(),
                user: "a".into(),
            },
            source: Source::Keychain,
            host: "example.com".into(),
            user: "a".into(),
            sites: sites.iter().map(|s| s.to_string()).collect(),
            name: "a".into(),
            has_totp: false,
            used: None,
            folder: None,
            agent_hint: AgentHint::None,
        }
    }

    #[test]
    fn an_item_fills_www_and_the_bare_site_only_when_it_lists_both() {
        assert!(!owns(
            &login(&["https://example.com"]),
            "https://www.example.com"
        ));
        assert!(owns(
            &login(&["https://example.com", "https://www.example.com"]),
            "https://www.example.com"
        ));
        assert!(!owns(&login(&["localhost"]), "http://localhost:8765"));
        assert!(owns(
            &login(&["http://localhost:8765"]),
            "http://localhost:8765"
        ));
    }

    #[test]
    fn a_keyring_login_keeps_the_scheme_and_port_it_was_saved_from() {
        let get = |attrs: &[(&str, String)], key: &str| {
            attrs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
        };
        let http = place(Some("http://example.com:8080"));
        assert_eq!(get(&http, "protocol").as_deref(), Some("http"));
        assert_eq!(get(&http, "port").as_deref(), Some("8080"));
        assert_eq!(
            placed(
                get(&http, "protocol").as_deref(),
                get(&http, "port").as_deref(),
                "example.com"
            )
            .as_deref(),
            Some("http://example.com:8080")
        );
        let https = place(Some("https://example.com"));
        assert_eq!(get(&https, "port").as_deref(), Some("443"));
        assert_eq!(
            placed(
                get(&https, "protocol").as_deref(),
                get(&https, "port").as_deref(),
                "example.com"
            )
            .as_deref(),
            Some("https://example.com")
        );
        assert!(place(None).is_empty() && place(Some("ftp://example.com")).is_empty());
        // An item saved before then (no protocol) has no origin: the bare host rule above.
        assert_eq!(placed(None, None, "example.com"), None);
    }
}
