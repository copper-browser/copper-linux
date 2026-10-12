//! Hosts, sites and vault URI matching (`Vault.registrable`, `Vault.host`,
//! `Credentials.matches` and friends). Pure.

use regex::RegexBuilder;
use url::Url;

/// `example.com` for `www.example.com` and `accounts.example.com`;
/// `bbc.co.uk` stays `bbc.co.uk`. The handful of two-part endings that matter
/// are listed; a full public suffix list would be a library for a corner.
pub fn registrable(host: &str) -> String {
    let lower = host.to_lowercase();
    let labels: Vec<&str> = lower.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    const SECONDS: [&str; 11] = [
        "co", "com", "org", "net", "gov", "gouv", "ac", "edu", "asso", "or", "ne",
    ];
    let n = labels.len();
    if SECONDS.contains(&labels[n - 2]) && labels[n - 1].chars().count() == 2 {
        return labels[n - 3..].join(".");
    }
    labels[n - 2..].join(".")
}

/// The site a typed or stored address belongs to: lowercased, no `www.`;
/// empty when there is no host (`Vault.host(of:)`).
pub fn host_of(text: &str) -> String {
    let mut value = text.trim().to_string();
    if !value.contains("://") {
        value = format!("https://{value}");
    }
    let Some(host) = Url::parse(&value).ok().and_then(|u| {
        u.host_str()
            .map(|h| h.trim_matches(['[', ']']).to_lowercase())
    }) else {
        return String::new();
    };
    match host.strip_prefix("www.") {
        Some(rest) => rest.to_string(),
        None => host,
    }
}

/// Lowercased, trimmed, without trailing dots.
pub fn normalized(host: &str) -> String {
    host.trim().to_lowercase().trim_end_matches('.').to_string()
}

/// The host of a vault URI (`https://x/y`, `x/y`, `x`), normalised.
pub fn uri_host(value: &str) -> Option<String> {
    let raw = value.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    if let Some(host) = Url::parse(&candidate)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_string()))
        .filter(|h| !h.is_empty())
    {
        return Some(normalized(&host));
    }
    Some(normalized(raw.split('/').next().unwrap_or(raw)))
}

fn scheme_of(uri: &str) -> String {
    match uri.find("://") {
        Some(at) => uri[..at].to_lowercase(),
        None => "https".into(),
    }
}

/// `localhost`, `*.localhost`, anything with a port or IPv6 colon, or a
/// dotted IPv4 address.
pub fn is_local_or_address(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") || host.contains(':') {
        return true;
    }
    let pieces: Vec<&str> = host.split('.').collect();
    pieces.len() == 4 && pieces.iter().all(|p| p.parse::<i64>().is_ok())
}

/// A stored website and its Bitwarden match rule (`Bitwarden.URI`).
/// `match_`: None/0 domain, 1 host, 2 starts-with, 3 exact, 4 regex, 5 never.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Uri {
    pub uri: String,
    pub match_: Option<i64>,
}

impl Uri {
    pub fn new(uri: impl Into<String>) -> Self {
        Uri {
            uri: uri.into(),
            match_: None,
        }
    }
}

/// Whether a saved website applies to a page on `host` (`Credentials.matches`).
pub fn matches(uri: &Uri, host: &str) -> bool {
    let target = normalized(host);
    let rule = uri.match_.unwrap_or(0);
    if rule == 5 {
        return false;
    }
    let target_url = format!("{}://{}", scheme_of(&uri.uri), target);
    if rule == 4 {
        return RegexBuilder::new(&uri.uri)
            .case_insensitive(true)
            .build()
            .is_ok_and(|re| re.is_match(&target_url));
    }
    let Some(stored) = uri_host(&uri.uri) else {
        return false;
    };
    match rule {
        1 => stored == target,
        2 => target_url
            .to_lowercase()
            .starts_with(&uri.uri.to_lowercase()),
        3 => {
            target_url.to_lowercase() == uri.uri.trim_matches('/').to_lowercase()
                || (is_local_or_address(&target) && stored == target)
        }
        _ => {
            if is_local_or_address(&target) || is_local_or_address(&stored) {
                return stored == target;
            }
            registrable(&stored) == registrable(&target)
        }
    }
}

/// Sites the person asked never to be asked about (`passwords.never`): the
/// host itself or its registrable domain (`Vault.isNever`).
pub fn is_never<'a>(never: impl IntoIterator<Item = &'a str>, host: &str) -> bool {
    let site = registrable(host);
    never.into_iter().any(|n| n == host || n == site)
}

/// The scheme a new vault item's website gets: plain http for a dev server
/// on this machine or the LAN (`OnePassword.scheme(for:)`).
pub fn scheme_for(host: &str) -> &'static str {
    let value = host.to_lowercase();
    if value == "localhost" || value.contains(':') {
        return "http";
    }
    if value.ends_with(".localhost") || value.ends_with(".local") {
        return "http";
    }
    let pieces: Vec<&str> = value.split('.').collect();
    let numeric = pieces.len() == 4 && pieces.iter().all(|p| p.parse::<u8>().is_ok());
    if numeric { "http" } else { "https" }
}

/// Lowercased, without a leading `www.`.
pub fn bare(host: &str) -> String {
    let lower = host.to_lowercase();
    match lower.strip_prefix("www.") {
        Some(rest) => rest.to_string(),
        None => lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable("www.example.com"), "example.com");
        assert_eq!(registrable("accounts.example.com"), "example.com");
        assert_eq!(registrable("news.bbc.co.uk"), "bbc.co.uk");
        assert_eq!(registrable("example.com"), "example.com");
        assert_eq!(registrable("a.b.example.org"), "example.org");
        assert_eq!(registrable("x.gouv.fr"), "x.gouv.fr");
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://www.Example.com/login"), "example.com");
        assert_eq!(host_of("example.com/x"), "example.com");
        assert_eq!(host_of("  "), "");
        assert_eq!(host_of("http://localhost:8765/"), "localhost");
        assert_eq!(
            uri_host("https://Accounts.Example.com./x").as_deref(),
            Some("accounts.example.com")
        );
        assert_eq!(
            uri_host("example.org/login").as_deref(),
            Some("example.org")
        );
        assert_eq!(uri_host(""), None);
    }

    #[test]
    fn match_rules() {
        let m = |u: &str, r: Option<i64>, h: &str| {
            matches(
                &Uri {
                    uri: u.into(),
                    match_: r,
                },
                h,
            )
        };
        assert!(m("https://example.com", None, "accounts.example.com"));
        assert!(!m("https://example.com", None, "example.org"));
        assert!(m("https://example.com", Some(1), "example.com"));
        assert!(!m("https://example.com", Some(1), "www.example.com"));
        assert!(m("https://exa", Some(2), "example.com"));
        assert!(m("https://example.com/", Some(3), "example.com"));
        // The scheme is read off the pattern up to `://`, as on macOS.
        assert!(m(r"https://.*\.example\.com$", Some(4), "a.example.com"));
        assert!(!m(r"https://.*\.example\.com$", Some(4), "a.example.org"));
        assert!(!m("https://example.com", Some(5), "example.com"));
        assert!(m("http://localhost:8765/", None, "localhost"));
        assert!(!m("http://localhost", None, "other.localhost"));
        assert!(m("http://192.168.1.4", None, "192.168.1.4"));
    }

    #[test]
    fn never_and_schemes() {
        assert!(is_never(["example.com"], "accounts.example.com"));
        assert!(is_never(["accounts.example.com"], "accounts.example.com"));
        assert!(!is_never(["example.com"], "example.org"));
        assert_eq!(scheme_for("localhost"), "http");
        assert_eq!(scheme_for("dev.local"), "http");
        assert_eq!(scheme_for("10.0.0.2"), "http");
        assert_eq!(scheme_for("example.com"), "https");
    }
}
