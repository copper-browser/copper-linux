//! What the field turns typed text into (Mac: `Address.swift`, `Google.swift`).

use url::Url;

/// Schemes a tab can show itself. Anything else typed with a scheme is
/// refused rather than opening a blank tab.
const OURS: [&str; 7] = [
    "http",
    "https",
    "file",
    "about",
    "data",
    "copper",
    "copper-easel",
];

/// A place if the text can be one (`Address.url(from:)`): explicit schemes we
/// handle, `about:`/`data:`, or something that looks like a host (`a.b`,
/// `localhost`, an IPv4 address), with `http://` for local hosts and
/// `https://` otherwise. `None` means "search for it".
pub fn url_from_typed(typed: &str) -> Option<Url> {
    let text = typed.trim();
    if text.is_empty() || text.contains(' ') {
        return None;
    }
    if let Some(split) = text.find("://") {
        let scheme = text[..split].to_lowercase();
        if !OURS.contains(&scheme.as_str()) {
            return None;
        }
        return Url::parse(text).ok();
    }
    let lower = text.to_lowercase();
    if lower.starts_with("about:") || lower.starts_with("data:") {
        return Url::parse(text).ok();
    }
    let head: &str = text.split(['/', '?', '#']).next().unwrap_or(text);
    if head.contains('@') {
        return None; // an email address
    }
    let host = head.split(':').next().unwrap_or(head);
    if !looks_like_host(host) {
        return None;
    }
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host == "0.0.0.0"
        || host.starts_with("192.168.")
        || host.starts_with("10.");
    Url::parse(&format!(
        "{}{}",
        if local { "http://" } else { "https://" },
        text
    ))
    .ok()
}

fn looks_like_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() == 4 && labels.iter().all(|l| l.parse::<u8>().is_ok()) {
        return true;
    }
    if labels.len() < 2 {
        return false;
    }
    let ok = labels.iter().all(|l| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_alphanumeric() || c == '-')
    });
    if !ok {
        return false;
    }
    let tld = labels[labels.len() - 1];
    tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic)
}

/// What the tab says before the page has a title, and the history key
/// (lowercased): host without `www.`, plus the path unless it is `/`
/// (`Address.pretty`).
pub fn pretty(url: &Url) -> String {
    let Some(host) = url.host_str() else {
        return url.as_str().to_string();
    };
    let bare = host.strip_prefix("www.").unwrap_or(host);
    let path = url.path();
    if path.is_empty() || path == "/" {
        bare.to_string()
    } else {
        format!("{bare}{path}")
    }
}

/// [`pretty`] of a string, or the string itself if it does not parse.
pub fn pretty_str(url: &str) -> String {
    Url::parse(url)
        .map(|u| pretty(&u))
        .unwrap_or_else(|_| url.to_string())
}

/// The history key for a URL: [`pretty`], lowercased.
pub fn history_key(url: &Url) -> String {
    pretty(url).to_lowercase()
}

/// Lowercase, trim, drop `http(s)://` and `www.` — how typed text is compared
/// to keys.
pub fn strip_typed(typed: &str) -> String {
    let mut text = typed.trim().to_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = text.strip_prefix(scheme) {
            text = rest.to_string();
        }
    }
    if let Some(rest) = text.strip_prefix("www.") {
        text = rest.to_string();
    }
    text
}

/// Percent-encode everything but `A-Za-z0-9-._~` (Swift's alphanumerics plus
/// `-._~`; non-ASCII letters are encoded as UTF-8 bytes, which is what a
/// query string needs anyway).
pub fn query_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A search engine: a name and a URL template with `{q}` for the escaped
/// query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchEngine {
    /// Stable id stored in settings (`google`, `duckduckgo`, …).
    pub id: &'static str,
    pub name: &'static str,
    pub template: &'static str,
}

/// Built-in engines. The Mac only has Google; the rest are Linux additions
/// selected by the `search.engine` setting.
pub const ENGINES: [SearchEngine; 5] = [
    SearchEngine {
        id: "google",
        name: "Google",
        template: "https://www.google.com/search?q={q}",
    },
    SearchEngine {
        id: "duckduckgo",
        name: "DuckDuckGo",
        template: "https://duckduckgo.com/?q={q}",
    },
    SearchEngine {
        id: "bing",
        name: "Bing",
        template: "https://www.bing.com/search?q={q}",
    },
    SearchEngine {
        id: "kagi",
        name: "Kagi",
        template: "https://kagi.com/search?q={q}",
    },
    SearchEngine {
        id: "startpage",
        name: "Startpage",
        template: "https://www.startpage.com/do/search?q={q}",
    },
];

impl SearchEngine {
    /// The engine with this id, or Google.
    pub fn by_id(id: &str) -> &'static SearchEngine {
        ENGINES.iter().find(|e| e.id == id).unwrap_or(&ENGINES[0])
    }
    /// The results page for `text`, or `None` for blank text.
    pub fn url_for(&self, text: &str) -> Option<Url> {
        let words = text.trim();
        if words.is_empty() {
            return None;
        }
        Url::parse(&self.template.replace("{q}", &query_escape(words))).ok()
    }
}

/// A place if it can be one, else a search (`Google.destination(for:)`).
pub fn destination(typed: &str, engine: &SearchEngine) -> Option<Url> {
    url_from_typed(typed).or_else(|| engine.url_for(typed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text() {
        assert_eq!(
            url_from_typed("github.com").unwrap().as_str(),
            "https://github.com/"
        );
        assert_eq!(
            url_from_typed("localhost:3000/x").unwrap().as_str(),
            "http://localhost:3000/x"
        );
        assert_eq!(
            url_from_typed("192.168.1.2").unwrap().as_str(),
            "http://192.168.1.2/"
        );
        assert!(url_from_typed("hello world").is_none());
        assert!(url_from_typed("todo").is_none());
        assert!(url_from_typed("v1.2").is_none());
        assert!(url_from_typed("me@example.com").is_none());
        assert!(url_from_typed("mailto://x").is_none());
        assert_eq!(
            url_from_typed("about:blank").unwrap().as_str(),
            "about:blank"
        );
        let g = SearchEngine::by_id("google");
        assert_eq!(
            destination("a+b c", g).unwrap().as_str(),
            "https://www.google.com/search?q=a%2Bb%20c"
        );
    }

    #[test]
    fn pretty_keys() {
        let u = Url::parse("https://www.GitHub.com/").unwrap();
        assert_eq!(pretty(&u), "github.com");
        let u = Url::parse("https://github.com/Foo/bar?x=1").unwrap();
        assert_eq!(history_key(&u), "github.com/foo/bar");
        assert_eq!(strip_typed("  HTTPS://www.Git"), "git");
    }
}
