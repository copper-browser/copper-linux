//! Conversions between webkit6/soup/gdk values and copper-engine values,
//! plus small pure helpers (unit-tested without GTK).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use copper_engine::{
    Bitmap, Cookie, EngineError, Image, JsonValue, Result, SameSite, WebsiteDataKinds,
};
use gdk4::prelude::*;
use webkit6::{WebsiteDataTypes, javascriptcore, soup};

pub(crate) fn path_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| EngineError::InvalidArgument(format!("path is not UTF-8: {}", p.display())))
}

pub(crate) fn texture_to_image(tex: &gdk4::Texture) -> Image {
    Image {
        png: tex.save_to_png_bytes().to_vec(),
        width: tex.width().max(0) as u32,
        height: tex.height().max(0) as u32,
    }
}

/// The texture's pixels as straight-alpha RGBA8 (GDK converts whatever
/// format WebKit painted in).
pub(crate) fn texture_to_bitmap(tex: &gdk4::Texture) -> Bitmap {
    let mut down = gdk4::TextureDownloader::new(tex);
    down.set_format(gdk4::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = down.download_bytes();
    Bitmap {
        width: tex.width().max(0) as u32,
        height: tex.height().max(0) as u32,
        stride,
        rgba: bytes.to_vec(),
    }
}

/// JSON form of a JavaScriptCore value (`undefined`, functions, symbols → `null`).
pub(crate) fn js_to_json(v: &javascriptcore::Value) -> Result<JsonValue> {
    if v.is_undefined() {
        return Ok(JsonValue::Null);
    }
    match v.to_json(0) {
        Some(s) => serde_json::from_str(&s)
            .map_err(|e| EngineError::JavaScript(format!("result is not JSON ({e}): {s}"))),
        None => Ok(JsonValue::Null),
    }
}

pub(crate) fn js_error(e: glib::Error) -> EngineError {
    EngineError::JavaScript(e.message().to_owned())
}

pub(crate) fn cookie_from_soup(mut c: soup::Cookie) -> Cookie {
    Cookie {
        name: c.name().map(String::from).unwrap_or_default(),
        value: c.value().map(String::from).unwrap_or_default(),
        domain: c.domain().map(String::from).unwrap_or_default(),
        path: c.path().map(String::from).unwrap_or_else(|| "/".into()),
        expires: c.expires().map(|d| d.to_unix()),
        secure: c.is_secure(),
        http_only: c.is_http_only(),
        same_site: match c.same_site_policy() {
            soup::SameSitePolicy::None => SameSite::None,
            soup::SameSitePolicy::Strict => SameSite::Strict,
            _ => SameSite::Lax,
        },
    }
}

pub(crate) fn cookie_to_soup(c: &Cookie) -> Result<soup::Cookie> {
    if c.name.is_empty() && c.value.is_empty() {
        return Err(EngineError::InvalidArgument(
            "cookie has no name or value".into(),
        ));
    }
    if c.domain.is_empty() {
        return Err(EngineError::InvalidArgument(format!(
            "cookie {:?} has no domain",
            c.name
        )));
    }
    let path = if c.path.is_empty() { "/" } else { &c.path };
    let mut sc = soup::Cookie::new(&c.name, &c.value, &c.domain, path, -1);
    if let Some(ts) = c.expires {
        let dt = glib::DateTime::from_unix_utc(ts)
            .map_err(|e| EngineError::InvalidArgument(format!("cookie expiry {ts}: {e}")))?;
        sc.set_expires(&dt);
    }
    sc.set_secure(c.secure);
    sc.set_http_only(c.http_only);
    sc.set_same_site_policy(match c.same_site {
        SameSite::None => soup::SameSitePolicy::None,
        SameSite::Lax => soup::SameSitePolicy::Lax,
        SameSite::Strict => soup::SameSitePolicy::Strict,
    });
    Ok(sc)
}

pub(crate) fn data_types(kinds: WebsiteDataKinds) -> WebsiteDataTypes {
    let map = [
        (
            WebsiteDataKinds::MEMORY_CACHE,
            WebsiteDataTypes::MEMORY_CACHE,
        ),
        (WebsiteDataKinds::DISK_CACHE, WebsiteDataTypes::DISK_CACHE),
        (WebsiteDataKinds::COOKIES, WebsiteDataTypes::COOKIES),
        (
            WebsiteDataKinds::LOCAL_STORAGE,
            WebsiteDataTypes::LOCAL_STORAGE,
        ),
        (
            WebsiteDataKinds::SESSION_STORAGE,
            WebsiteDataTypes::SESSION_STORAGE,
        ),
        (
            WebsiteDataKinds::INDEXEDDB,
            WebsiteDataTypes::INDEXEDDB_DATABASES,
        ),
        (
            WebsiteDataKinds::SERVICE_WORKERS,
            WebsiteDataTypes::SERVICE_WORKER_REGISTRATIONS,
        ),
        (WebsiteDataKinds::DOM_CACHE, WebsiteDataTypes::DOM_CACHE),
        (WebsiteDataKinds::HSTS, WebsiteDataTypes::HSTS_CACHE),
        (
            WebsiteDataKinds::OFFLINE_APP_CACHE,
            WebsiteDataTypes::OFFLINE_APPLICATION_CACHE,
        ),
    ];
    let mut out = WebsiteDataTypes::empty();
    for (k, t) in map {
        if kinds.contains(k) {
            out |= t;
        }
    }
    out
}

/// Whether a website-data record name (WebKit groups data by registrable
/// domain, e.g. `example.com`) belongs to `domain` (a host or domain).
pub(crate) fn site_matches(record: &str, domain: &str) -> bool {
    let r = record.trim_start_matches('.').to_ascii_lowercase();
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    if r.is_empty() || d.is_empty() {
        return false;
    }
    r == d || r.ends_with(&format!(".{d}")) || d.ends_with(&format!(".{r}"))
}

/// A safe file name from a server-suggested one.
pub(crate) fn sanitize_filename(suggested: &str) -> String {
    let base = suggested.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_string();
    if cleaned.is_empty() {
        "download".into()
    } else {
        cleaned
    }
}

/// `dir/name`, or `dir/stem (n).ext` for the first `n` not on disk and not reserved.
pub(crate) fn unique_path(dir: &Path, suggested: &str, reserved: &HashSet<PathBuf>) -> PathBuf {
    let name = sanitize_filename(suggested);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let free = |p: &PathBuf| !reserved.contains(p) && std::fs::symlink_metadata(p).is_err();
    let first = dir.join(&name);
    if free(&first) {
        return first;
    }
    (1u32..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(free)
        .expect("an unused file name exists")
}

/// A plain JS identifier (used as a parameter name in generated code).
pub(crate) fn is_js_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Host of a URL, if it has one.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let uri = glib::Uri::parse(url, glib::UriFlags::NONE).ok()?;
    uri.host().map(|h| h.to_string()).filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites() {
        assert!(site_matches("example.com", "example.com"));
        assert!(site_matches("example.com", "www.example.com"));
        assert!(site_matches("a.example.com", "example.com"));
        assert!(site_matches("Example.COM", ".example.com"));
        assert!(!site_matches("example.com", "notexample.com"));
        assert!(!site_matches("ample.com", "example.com"));
        assert!(!site_matches("", "x"));
    }

    #[test]
    fn filenames() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a\\b\\c.txt"), "c.txt");
        assert_eq!(sanitize_filename(".bashrc"), "bashrc");
        assert_eq!(sanitize_filename(""), "download");
        assert_eq!(sanitize_filename("..."), "download");

        let dir = std::env::temp_dir().join(format!("copper-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut reserved = HashSet::new();
        let p = unique_path(&dir, "x.tar.gz", &reserved);
        assert_eq!(p, dir.join("x.tar.gz"));
        std::fs::write(&p, b"1").unwrap();
        let p2 = unique_path(&dir, "x.tar.gz", &reserved);
        assert_eq!(p2, dir.join("x.tar (1).gz"));
        reserved.insert(p2.clone());
        assert_eq!(
            unique_path(&dir, "x.tar.gz", &reserved),
            dir.join("x.tar (2).gz")
        );
        assert_eq!(unique_path(&dir, "noext", &reserved), dir.join("noext"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identifiers() {
        for ok in ["a", "_x", "$el", "selector2"] {
            assert!(is_js_identifier(ok), "{ok}");
        }
        for bad in ["", "2a", "a-b", "a b", "a;alert(1)", "é"] {
            assert!(!is_js_identifier(bad), "{bad}");
        }
    }

    #[test]
    fn hosts() {
        assert_eq!(
            url_host("http://127.0.0.1:8080/x").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            url_host("https://www.example.com/").as_deref(),
            Some("www.example.com")
        );
        assert_eq!(url_host("about:blank"), None);
    }

    #[test]
    fn kinds() {
        let t = data_types(WebsiteDataKinds::CACHES);
        assert_eq!(
            t,
            WebsiteDataTypes::MEMORY_CACHE | WebsiteDataTypes::DISK_CACHE
        );
        assert!(data_types(WebsiteDataKinds::ALL).contains(WebsiteDataTypes::COOKIES));
    }
}
