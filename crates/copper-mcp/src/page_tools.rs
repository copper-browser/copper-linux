//! The page tools that reach past the page's script into the engine (Mac
//! `Fork/MCP/StateTools.swift`, `FileChooser.swift`, `NetLog.swift`,
//! `Permissions.swift`): files in (`browser_file_upload`) and out
//! (`browser_pdf_save`), what the page fetched (`browser_network_requests`),
//! its site's cookies and permissions. Playwright MCP's names and shapes
//! where it has them. Each works on the caller's own tab, never another's.

use std::path::{Path, PathBuf};
use std::time::Duration;

use copper_engine::{Cookie, GeoPosition, NetEntry, SameSite};
use copper_pagejs::page::{self as pj, DropFile};
use serde_json::{Map, Value};

use crate::drive::string_list;
use crate::host::{Content, Host, Page, TabId};
use crate::tools::{arg_bool, arg_f64, arg_str, js_value, number};
use crate::{Failure, Mcp};

/// The tools here, in the Mac's order (`StateTools.catalogue`, less the
/// browser-state ones).
pub const NAMES: [&str; 5] = [
    "browser_file_upload",
    "browser_pdf_save",
    "browser_network_requests",
    "browser_cookies",
    "browser_permissions",
];

impl<H: Host> Mcp<H> {
    /// One of [`NAMES`], on `id`.
    pub(crate) async fn page_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        id: &TabId,
        page: &H::Page,
    ) -> Result<Vec<Content>, Failure> {
        match name {
            "browser_file_upload" => self.upload(args, id, page).await,
            "browser_pdf_save" => self.pdf(args, id, page).await,
            "browser_network_requests" => {
                let log = page.network().ok_or("the tab has no page yet")?;
                let limit = arg_f64(args, "limit")
                    .map_or(100, |l| l as i64)
                    .clamp(1, copper_engine::NetLog::CAP as i64)
                    as usize;
                let include = arg_bool(args, "includeStatic").unwrap_or(false);
                let shown = net_lines(&log.entries(), include, limit);
                let body = if shown.is_empty() {
                    "No requests since this page loaded (or since an agent first used the tab)."
                        .to_string()
                } else {
                    shown.join("\n")
                };
                Ok(vec![Content::Text(format!(
                    "### Network requests\n{body}\n\n{}",
                    self.page_line(id)
                ))])
            }
            "browser_cookies" => self.cookies(args, id).await,
            "browser_permissions" => self.permissions(args, Some(id)),
            _ => Err(Failure(format!("Unknown tool {name}"))),
        }
    }

    // -- browser_file_upload -----------------------------------------------------

    async fn upload(
        &self,
        args: &Map<String, Value>,
        id: &TabId,
        page: &H::Page,
    ) -> Result<Vec<Content>, Failure> {
        let paths = string_list(args.get("paths"));
        if paths.is_empty() {
            let said = if self.answer_chooser(id, None) {
                "Cancelled the file chooser."
            } else if self.arm_upload(id, None) {
                "Disarmed: the next file chooser on this tab opens as usual."
            } else {
                "No file chooser is open on this tab, and none was armed."
            };
            return Ok(vec![Content::text(said)]);
        }
        if let Some(reference) = arg_str(args, "ref").filter(|r| !r.is_empty()) {
            return self.drop_files(&paths, reference, id, page).await;
        }
        let held = self.state.borrow().dialogs.held_chooser(id);
        let files = checked(&paths, held.is_some_and(|h| h.directories))?;
        let Some(held) = held else {
            self.arm_upload(id, Some(files.clone()));
            return Ok(vec![Content::Text(format!(
                "Armed: the next file chooser this tab opens (within {} min) gets {}. Now click the file input.",
                crate::dialogs::ARMED_FOR.as_secs() / 60,
                names(&files)
            ))]);
        };
        if files.len() > 1 && !held.multiple {
            return Err(Failure(format!(
                "this file input takes one file; {} given",
                files.len()
            )));
        }
        self.answer_chooser(id, Some(files.clone()));
        // The page hears `change` on its next turn.
        self.host.sleep(Duration::from_millis(150)).await;
        Ok(vec![Content::Text(format!(
            "Uploaded {}. {}",
            names(&files),
            self.page_line(id)
        ))])
    }

    /// Files dragged onto a drop zone, as from a file manager (`FileChooser.drop`).
    async fn drop_files(
        &self,
        paths: &[String],
        reference: &str,
        id: &TabId,
        page: &H::Page,
    ) -> Result<Vec<Content>, Failure> {
        let files = checked(paths, false)?;
        let mut dropped = Vec::new();
        let mut total = 0usize;
        for path in &files {
            let data =
                std::fs::read(path).map_err(|e| Failure(format!("{}: {e}", path.display())))?;
            total += data.len();
            if total > DROP_LIMIT {
                return Err(Failure(format!(
                    "more than {} MB to drop; drop fewer or smaller files",
                    DROP_LIMIT >> 20
                )));
            }
            use base64::Engine as _;
            dropped.push(DropFile {
                name: file_name(path),
                mime: mime(path),
                data: base64::engine::general_purpose::STANDARD.encode(&data),
            });
        }
        let result = js_value(page, &pj::drop(reference, &dropped)).await?;
        if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(Failure(format!(
                "{error}. Take a fresh browser_snapshot — refs change when the page does."
            )));
        }
        if result.get("accepted").and_then(Value::as_bool) != Some(true) {
            return Err(Failure(format!(
                "{reference} did not take the files: it cancelled no dragover, so it is not a drop zone. Drop on the element the page treats as the zone, or click its file input."
            )));
        }
        self.host.sleep(Duration::from_millis(150)).await;
        Ok(vec![Content::Text(format!(
            "Dropped {} on {reference}. {}",
            names(&files),
            self.page_line(id)
        ))])
    }

    // -- browser_pdf_save ----------------------------------------------------------

    async fn pdf(
        &self,
        args: &Map<String, Value>,
        id: &TabId,
        page: &H::Page,
    ) -> Result<Vec<Content>, Failure> {
        let title = self.host.tab(id).map(|t| t.title).unwrap_or_default();
        let folder = self.host.downloads_folder();
        // A fresh home has no Downloads yet; the first download makes it too.
        let _ = std::fs::create_dir_all(&folder);
        let path = destination(arg_str(args, "filename"), &title, &folder)?;
        page.print_pdf(&path)
            .await
            .map_err(|e| Failure(format!("couldn't save {}: {e}", path.display())))?;
        let pages = std::fs::read(&path)
            .map(|b| pdf_pages(&b))
            .map_err(|e| Failure(format!("couldn't save {}: {e}", path.display())))?;
        Ok(vec![Content::Text(format!(
            "Saved a {pages}-page PDF to {}. {}",
            path.display(),
            self.page_line(id)
        ))])
    }

    // -- browser_cookies -------------------------------------------------------------

    async fn cookies(
        &self,
        args: &Map<String, Value>,
        id: &TabId,
    ) -> Result<Vec<Content>, Failure> {
        let url = self.host.tab(id).and_then(|t| t.url).unwrap_or_default();
        let host = url::Url::parse(&url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_lowercase))
            .filter(|h| !h.is_empty())
            .ok_or("your tab has no site — navigate first")?;
        let site = host.strip_prefix("www.").unwrap_or(&host).to_string();
        let access = self.host.site_shared_with_agents(&url).await;
        let refusal = format!(
            "HttpOnly cookies are {site}'s sign-in — the user can share this site with agents in Settings › Passwords › Agent access"
        );
        let jar: Vec<Cookie> = self
            .host
            .cookie_jar(id)
            .await
            .map_err(Failure)?
            .into_iter()
            .filter(|c| cookie_matches(&c.domain, &host))
            .collect();
        let name = arg_str(args, "name").filter(|n| !n.is_empty());
        let path = arg_str(args, "path")
            .filter(|p| !p.is_empty())
            .unwrap_or("/");
        match arg_str(args, "action").unwrap_or("list") {
            "list" => {
                let mut shown: Vec<&Cookie> = jar
                    .iter()
                    .filter(|c| name.is_none_or(|n| c.name == n))
                    .collect();
                shown.sort_by(|a, b| (&a.domain, &a.name).cmp(&(&b.domain, &b.name)));
                if shown.is_empty() {
                    return Ok(vec![Content::Text(match name {
                        Some(n) => format!("No cookie {n} for {host}."),
                        None => format!("No cookies for {host}."),
                    })]);
                }
                let lines: Vec<String> = shown
                    .iter()
                    .map(|c| cookie_line(c, access, &refusal))
                    .collect();
                Ok(vec![Content::Text(format!(
                    "### Cookies for {host}\n{}",
                    lines.join("\n")
                ))])
            }
            "set" => {
                let name = name.ok_or("name required")?;
                let value = arg_str(args, "value").ok_or("value required")?;
                let domain = arg_str(args, "domain")
                    .filter(|d| !d.is_empty())
                    .unwrap_or(&host);
                if !may_scope(domain, &host) {
                    return Err(Failure(format!(
                        "{domain} is not {host} or a parent domain of it"
                    )));
                }
                let http_only = arg_bool(args, "httpOnly").unwrap_or(false);
                let replaces = jar.iter().any(|c| {
                    c.name == name
                        && c.path == path
                        && c.http_only
                        && same_domain(&c.domain, domain)
                });
                if (http_only || replaces) && !access {
                    return Err(Failure(format!("not set: {refusal}")));
                }
                let expires = number(args, "expires", 1e11)?.map(|e| e as i64);
                // No SameSite reads as Lax, as browsers take a cookie without
                // one (an insecure `None` is dropped by the jar).
                let same_site = match arg_str(args, "sameSite").map(str::to_lowercase).as_deref() {
                    Some("strict") => SameSite::Strict,
                    Some("none") => SameSite::None,
                    _ => SameSite::Lax,
                };
                let cookie = Cookie {
                    name: name.to_string(),
                    value: value.to_string(),
                    domain: domain.to_string(),
                    path: path.to_string(),
                    expires,
                    secure: arg_bool(args, "secure").unwrap_or(false),
                    http_only,
                    same_site,
                };
                self.host.set_cookie(id, cookie).await.map_err(Failure)?;
                Ok(vec![Content::Text(format!(
                    "Set {name} for {domain}{}.",
                    if http_only { " (HttpOnly)" } else { "" }
                ))])
            }
            "delete" => {
                let name = name.ok_or("name required")?;
                let domain = arg_str(args, "domain").filter(|d| !d.is_empty());
                let gone: Vec<Cookie> = jar
                    .into_iter()
                    .filter(|c| {
                        c.name == name
                            && domain.is_none_or(|d| same_domain(&c.domain, d))
                            && (args.get("path").is_none() || c.path == path)
                    })
                    .collect();
                if gone.is_empty() {
                    return Ok(vec![Content::Text(format!(
                        "No cookie {name} for {host}; nothing deleted."
                    ))]);
                }
                if gone.iter().any(|c| c.http_only) && !access {
                    return Err(Failure(format!("not deleted: {refusal}")));
                }
                let count = gone.len();
                for cookie in gone {
                    self.host.delete_cookie(id, cookie).await.map_err(Failure)?;
                }
                Ok(vec![Content::Text(format!(
                    "Deleted {count} cookie{} named {name}.",
                    if count == 1 { "" } else { "s" }
                ))])
            }
            _ => Err("action must be list, set or delete".into()),
        }
    }

    // -- browser_permissions ---------------------------------------------------------

    /// `browser_permissions` (`Permissions.call`). `tab`: the caller's tab,
    /// when the call names no `site`.
    pub(crate) fn permissions(
        &self,
        args: &Map<String, Value>,
        tab: Option<&TabId>,
    ) -> Result<Vec<Content>, Failure> {
        let grant = kinds(args.get("grant"))?;
        let revoke = kinds(args.get("revoke"))?;
        let named = arg_str(args, "site")
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        let site = match named {
            Some(named) => {
                let full = if named.contains("://") {
                    named.clone()
                } else {
                    format!("https://{named}")
                };
                url::Url::parse(&full)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| site_of(h, u.port())))
                    .unwrap_or(named)
            }
            None => {
                let url = tab
                    .and_then(|t| self.host.tab(t))
                    .and_then(|t| t.url)
                    .unwrap_or_else(|| "about:blank".into());
                url::Url::parse(&url)
                    .ok()
                    .and_then(|u| u.host_str().filter(|h| !h.is_empty()).map(|h| site_of(h, u.port())))
                    .ok_or_else(|| {
                        Failure(format!(
                            "Your tab has no site ({url}). Open one with browser_navigate, or pass site."
                        ))
                    })?
            }
        };
        if grant.iter().any(|k| k == "notifications") {
            return Err("Copper doesn't show web notifications, so it can't grant them: a site would think its messages arrive. Leave notifications out and grant the rest.".into());
        }
        if grant.iter().any(|k| k == "geolocation") {
            let lat = number(args, "latitude", 90.0)?;
            let lon = number(args, "longitude", 180.0)?;
            let (Some(latitude), Some(longitude)) = (lat, lon) else {
                return Err("geolocation needs latitude and longitude: Copper has no location of its own, so the page would only get \"position unavailable\". Call browser_permissions again with grant [\"geolocation\"], latitude and longitude (for example latitude 25.76, longitude -80.19). Nothing was granted.".into());
            };
            let accuracy = number(args, "accuracy", 1_000_000.0)?
                .unwrap_or(50.0)
                .max(1.0);
            self.host.set_geolocation(Some(GeoPosition {
                latitude,
                longitude,
                accuracy,
            }));
        }
        for kind in &grant {
            self.host.set_site_permission(&site, kind, true);
        }
        for kind in &revoke {
            self.host.set_site_permission(&site, kind, false);
        }
        let mut lines = vec![format!("### Permissions for {site}")];
        for kind in KINDS {
            lines.push(format!("- {kind}: {}", self.permission_state(kind, &site)));
        }
        Ok(vec![Content::Text(lines.join("\n"))])
    }

    fn permission_state(&self, kind: &str, site: &str) -> String {
        let granted = self.host.site_permission(site, kind);
        match kind {
            "geolocation" if granted => match self.host.geolocation() {
                Some(p) => format!("granted (at {}, {})", p.latitude, p.longitude),
                None => "granted (no position given)".into(),
            },
            // Linux asks the user where the Mac refuses (no Location Services there).
            "geolocation" => "ask".into(),
            "camera" | "microphone" if granted => "granted".into(),
            "camera" | "microphone" => "ask".into(),
            _ => "refused (Copper shows no web notifications)".into(),
        }
    }
}

/// `browser_permissions`' kinds, in order.
pub const KINDS: [&str; 4] = ["geolocation", "camera", "microphone", "notifications"];

fn kinds(raw: Option<&Value>) -> Result<Vec<String>, Failure> {
    let list = match raw {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(s)) => vec![s.clone()],
        Some(v) => string_list(Some(v)),
    };
    let wrong: Vec<&str> = list
        .iter()
        .map(String::as_str)
        .filter(|k| !KINDS.contains(k))
        .collect();
    if !wrong.is_empty() {
        return Err(Failure(format!(
            "Unknown permission {}; use one of {}.",
            wrong.join(", "),
            KINDS.join(", ")
        )));
    }
    Ok(list)
}

/// A site as permissions know it: the host, with the port when the address
/// gave one (`CaptureChoices.site`).
pub fn site_of(host: &str, port: Option<u16>) -> String {
    let host = host.to_lowercase();
    match port {
        Some(p) if p > 0 => format!("{host}:{p}"),
        _ => host,
    }
}

// -- files ---------------------------------------------------------------------

/// How much a drop may carry.
pub const DROP_LIMIT: usize = 25 << 20;

/// Every path absolute (or `~`), existing, and a file unless folders are
/// taken (`FileChooser.checked`).
pub fn checked(paths: &[String], directories: bool) -> Result<Vec<PathBuf>, Failure> {
    paths
        .iter()
        .map(|raw| {
            let path = expand(raw);
            if !path.is_absolute() {
                return Err(Failure(format!("{raw}: give an absolute path")));
            }
            let meta =
                std::fs::metadata(&path).map_err(|_| Failure(format!("{raw}: no such file")))?;
            if meta.is_dir() && !directories {
                return Err(Failure(format!(
                    "{raw} is a folder; this input takes files"
                )));
            }
            Ok(path)
        })
        .collect()
}

fn expand(raw: &str) -> PathBuf {
    match raw.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            let home = copper_core::platform::home_dir().unwrap_or_default();
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(raw),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn names(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|f| file_name(f))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A dropped file's type, as its extension says; empty when unknown.
fn mime(path: &Path) -> String {
    let full = copper_core::addons::scheme::mime_for(&path.to_string_lossy());
    let bare = full.split(';').next().unwrap_or("");
    if bare == "application/octet-stream" {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            return "application/pdf".into();
        }
        return String::new();
    }
    bare.to_string()
}

/// Where a PDF goes (`StateTools.destination`): an absolute (or `~`) path as
/// given; a bare name, or nothing (the title), in `folder`, numbered rather
/// than overwriting.
pub fn destination(given: Option<&str>, title: &str, folder: &Path) -> Result<PathBuf, Failure> {
    let raw = given.unwrap_or("").trim();
    let expanded = expand(raw);
    if !raw.is_empty() && (expanded.is_absolute() || raw.starts_with('~')) {
        let parent = expanded.parent().unwrap_or(Path::new("/"));
        if !parent.is_dir() {
            return Err(Failure(format!("no folder {}", parent.display())));
        }
        return Ok(expanded);
    }
    let base = if raw.is_empty() { title } else { raw };
    let clean: String = base
        .chars()
        .map(|c| {
            if matches!(c, '/' | ':' | '\\' | '\n') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let mut name = if clean.trim().is_empty() {
        "page".to_string()
    } else {
        clean
    };
    if !name.to_lowercase().ends_with(".pdf") {
        name.push_str(".pdf");
    }
    let stem = name[..name.len() - 4].to_string();
    let mut path = folder.join(&name);
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem} {n}.pdf"));
        n += 1;
    }
    Ok(path)
}

/// Pages in a PDF: its `/Type /Page` objects.
/// shortcut: a byte scan, right for the uncompressed page objects cairo and
/// Chromium write; a real parser if an engine ever packs them in streams.
pub fn pdf_pages(bytes: &[u8]) -> usize {
    let mut count = 0;
    for needle in [&b"/Type /Page"[..], &b"/Type/Page"[..]] {
        let mut at = 0;
        while let Some(i) = find(&bytes[at..], needle) {
            let end = at + i + needle.len();
            if bytes.get(end) != Some(&b's') {
                count += 1;
            }
            at = end;
        }
    }
    count.max(1)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

// -- requests ------------------------------------------------------------------

/// Kinds left out when they came back fine, unless `includeStatic`.
pub const STATIC_KINDS: [&str; 9] = [
    "image",
    "font",
    "media",
    "script",
    "stylesheet",
    "object",
    "manifest",
    "xslt",
    "other",
];

/// Playwright's lines: `[GET] url => [200] (kind)`, newest `limit`
/// (`NetLog.lines`).
pub fn net_lines(entries: &[NetEntry], include_static: bool, limit: usize) -> Vec<String> {
    let kept: Vec<&NetEntry> = entries
        .iter()
        .filter(|e| {
            include_static
                || !STATIC_KINDS.contains(&e.kind.as_str())
                || e.error.is_some()
                || e.status.unwrap_or(0) >= 400
        })
        .collect();
    kept[kept.len().saturating_sub(limit)..]
        .iter()
        .map(|e| {
            let outcome = match (&e.error, e.status) {
                (Some(why), _) => format!("failed: {why}"),
                (None, Some(status)) => format!("[{status}]"),
                (None, None) => "pending".into(),
            };
            format!("[{}] {} => {outcome} ({})", e.method, e.url, e.kind)
        })
        .collect()
}

// -- cookies -------------------------------------------------------------------

fn bare(domain: &str) -> String {
    let d = domain.to_lowercase();
    d.strip_prefix('.').map(str::to_string).unwrap_or(d)
}

/// A cookie for `domain` is sent to `host`: the host itself or a subdomain
/// of it (`CookieRules.matches`).
pub fn cookie_matches(domain: &str, host: &str) -> bool {
    let (d, h) = (bare(domain), host.to_lowercase());
    !d.is_empty() && (h == d || h.ends_with(&format!(".{d}")))
}

/// An agent may set a cookie for `domain` from `host`: the host, or a
/// parent domain with a dot in it; an IP address or a dotless host only for
/// itself (`CookieRules.mayScope`).
pub fn may_scope(domain: &str, host: &str) -> bool {
    let (d, h) = (bare(domain), host.to_lowercase());
    if d == h {
        return true;
    }
    let ip = h.chars().all(|c| c.is_ascii_digit() || c == '.') || h.contains(':');
    !ip && d.contains('.') && h.ends_with(&format!(".{d}"))
}

fn same_domain(a: &str, b: &str) -> bool {
    bare(a) == bare(b)
}

fn cookie_line(c: &Cookie, access: bool, refusal: &str) -> String {
    let value = if c.http_only && !access {
        format!("(hidden — {refusal})")
    } else {
        c.value.clone()
    };
    let mut flags = vec![format!("domain={}", c.domain), format!("path={}", c.path)];
    if c.http_only {
        flags.push("HttpOnly".into());
    }
    if c.secure {
        flags.push("Secure".into());
    }
    match c.same_site {
        SameSite::Lax => flags.push("SameSite=lax".into()),
        SameSite::Strict => flags.push("SameSite=strict".into()),
        SameSite::None => {}
    }
    flags.push(
        c.expires
            .map_or("session".into(), |e| format!("expires={e}")),
    );
    format!("- {}={value}; {}", c.name, flags.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hc-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn upload_takes_only_absolute_paths_that_exist() {
        let folder = temp("upload");
        let file = folder.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let f = file.to_string_lossy().to_string();
        assert_eq!(
            checked(std::slice::from_ref(&f), false).unwrap(),
            vec![file.clone()]
        );
        assert_eq!(
            checked(&["a.txt".into()], false).unwrap_err().0,
            "a.txt: give an absolute path"
        );
        let missing = folder.join("nope.txt").to_string_lossy().to_string();
        assert!(
            checked(&[missing], false)
                .unwrap_err()
                .0
                .ends_with("no such file")
        );
        let dir = folder.to_string_lossy().to_string();
        assert!(
            checked(std::slice::from_ref(&dir), false)
                .unwrap_err()
                .0
                .ends_with("is a folder; this input takes files")
        );
        assert_eq!(checked(&[dir], true).unwrap(), vec![folder.clone()]);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn cookies_belong_to_their_site() {
        assert!(cookie_matches("example.com", "example.com"));
        assert!(cookie_matches(".example.com", "www.example.com"));
        assert!(!cookie_matches("example.com", "notexample.com"));
        assert!(!cookie_matches("", "example.com"));
        assert!(may_scope("example.com", "www.example.com"));
        assert!(may_scope("www.example.com", "www.example.com"));
        assert!(!may_scope("com", "www.example.com"));
        assert!(!may_scope("other.com", "www.example.com"));
        assert!(may_scope("127.0.0.1", "127.0.0.1"));
        assert!(!may_scope("0.0.1", "127.0.0.1"));
    }

    #[test]
    fn pdf_goes_to_downloads_numbered_or_where_told() {
        let folder = temp("pdf");
        let first = destination(None, "A/B: c", &folder).unwrap();
        assert_eq!(first, folder.join("A-B- c.pdf"));
        std::fs::write(&first, "x").unwrap();
        assert_eq!(
            destination(None, "A/B: c", &folder).unwrap(),
            folder.join("A-B- c 2.pdf")
        );
        assert_eq!(
            destination(Some("Report"), "t", &folder).unwrap(),
            folder.join("Report.pdf")
        );
        assert_eq!(
            destination(Some(""), "  ", &folder).unwrap(),
            folder.join("page.pdf")
        );
        let abs = folder.join("x.pdf");
        assert_eq!(
            destination(Some(&abs.to_string_lossy()), "t", &folder).unwrap(),
            abs
        );
        let nowhere = folder.join("no/such/x.pdf");
        assert!(
            destination(Some(&nowhere.to_string_lossy()), "t", &folder)
                .unwrap_err()
                .0
                .starts_with("no folder ")
        );
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn pdf_pages_are_counted() {
        let pdf = b"<< /Type /Pages /Count 2 >> << /Type /Page >> << /Type/Page /Parent 1 0 R >>";
        assert_eq!(pdf_pages(pdf), 2);
        assert_eq!(pdf_pages(b"not a pdf"), 1);
    }

    #[test]
    fn requests_are_listed_like_playwright() {
        let e = |method: &str, url: &str, kind: &str, status: Option<u16>, error: Option<&str>| {
            let mut n = NetEntry::new(method, url, kind);
            n.status = status;
            n.error = error.map(str::to_string);
            n
        };
        let entries = vec![
            e("GET", "https://a.test/", "document", Some(200), None),
            e("GET", "https://a.test/a.png", "image", Some(200), None),
            e("GET", "https://a.test/b.png", "image", Some(404), None),
            e("POST", "https://a.test/api", "fetch", None, None),
            e(
                "GET",
                "https://a.test/x.js",
                "script",
                None,
                Some("offline"),
            ),
        ];
        assert_eq!(
            net_lines(&entries, false, 100),
            vec![
                "[GET] https://a.test/ => [200] (document)",
                "[GET] https://a.test/b.png => [404] (image)",
                "[POST] https://a.test/api => pending (fetch)",
                "[GET] https://a.test/x.js => failed: offline (script)",
            ]
        );
        assert_eq!(net_lines(&entries, true, 2).len(), 2);
        assert_eq!(net_lines(&entries, true, 100).len(), 5);
    }

    #[test]
    fn numbers_are_finite_and_bounded() {
        let args: Map<String, Value> =
            serde_json::from_str(r#"{"a": 12, "b": 1e100, "c": "x", "d": true}"#).unwrap();
        assert_eq!(number(&args, "a", 90.0).unwrap(), Some(12.0));
        assert_eq!(number(&args, "z", 90.0).unwrap(), None);
        assert_eq!(
            number(&args, "b", 90.0).unwrap_err().0,
            "b must be a number from -90 to 90; got 1e+100"
        );
        assert!(number(&args, "c", 90.0).is_err());
        assert!(number(&args, "d", 90.0).is_err());
    }

    #[test]
    fn sites_and_kinds() {
        assert_eq!(site_of("Example.COM", None), "example.com");
        assert_eq!(site_of("localhost", Some(8080)), "localhost:8080");
        assert_eq!(
            kinds(Some(&Value::String("camera".into()))).unwrap(),
            vec!["camera"]
        );
        assert_eq!(
            kinds(Some(&serde_json::json!(["camera", "midi"])))
                .unwrap_err()
                .0,
            "Unknown permission midi; use one of geolocation, camera, microphone, notifications."
        );
    }
}
