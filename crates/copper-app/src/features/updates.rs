//! Knowing when there is a newer Copper (Mac: `Updater.swift`,
//! `Fork/Updates.swift`, Settings › Updates).
//!
//! Every six hours (and on Check now) Copper reads
//! `releases/latest/download/copper-version.json` from the Linux repo's
//! GitHub releases; when it names a newer version it says
//! "Copper X is available" once, with a Download button that opens the
//! release page. It never replaces files: apt or Flatpak own them. Inside a
//! Flatpak it does not look at all — updates come through Flatpak.
//!
//! `COPPER_UPDATE_FEED` points the check elsewhere (tests); only then is
//! plain `http` accepted. What was last seen lives in `updates.json`.

use crate::features::format;
use crate::features::settings::general::flatpak;
use crate::features::settings::{button, group, line};
use crate::shell::Shell;
use copper_core::date::{SwiftDate, now_unix};
use copper_core::updates::{LatestRelease, UpdatesFile};
use gio::prelude::*;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Where the file lives.
pub const FEED: &str =
    "https://github.com/copper-browser/copper-linux/releases/latest/download/copper-version.json";
/// The page people download from.
pub const RELEASES: &str = "https://github.com/copper-browser/copper-linux/releases/latest";
/// How often to look.
const EVERY: f64 = 6.0 * 3600.0;

/// This build's version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The feed's file (`packaging/version-manifest.py`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: String,
    #[serde(default)]
    release_id: String,
    #[serde(default)]
    commit: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    latest_url: String,
}

#[derive(Default)]
struct State {
    file: UpdatesFile,
    checking: bool,
    error: Option<String>,
    view: Option<Rc<View>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn feed() -> String {
    std::env::var("COPPER_UPDATE_FEED")
        .ok()
        .filter(|f| !f.trim().is_empty())
        .unwrap_or_else(|| FEED.to_string())
}

/// At launch: read `updates.json`; look when due (a little after the window
/// is up), then every hour see whether six have passed.
pub fn start(shell: &Shell) {
    match shell.store().load::<UpdatesFile>() {
        Ok(l) => with(|s| s.file = l.value),
        Err(e) => tracing::warn!(error = %e, "updates.json"),
    }
    if flatpak() || STARTED.with(|s| s.replace(true)) {
        return;
    }
    // Test worlds and scripted runs only look when pointed at a feed.
    let world = std::env::var("COPPER_WORLD").is_ok_and(|w| !w.trim().is_empty());
    let selftest = std::env::var_os("COPPER_SELFTEST").is_some();
    if selftest || (world && std::env::var_os("COPPER_UPDATE_FEED").is_none()) {
        return;
    }
    let weak = shell.downgrade();
    let first = if std::env::var_os("COPPER_UPDATE_FEED").is_some() {
        2
    } else {
        20
    };
    glib::timeout_add_seconds_local_once(first, move || {
        if let Some(s) = weak.upgrade() {
            check_if_due(&s);
        }
    });
    let weak = shell.downgrade();
    glib::timeout_add_seconds_local(3600, move || {
        let Some(s) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        check_if_due(&s);
        glib::ControlFlow::Continue
    });
}

fn check_if_due(shell: &Shell) {
    let last = with(|s| s.file.checked_at.map(SwiftDate::unix));
    let due = last.is_none_or(|t| now_unix() - t >= EVERY)
        || std::env::var_os("COPPER_UPDATE_FEED").is_some();
    if due {
        check(shell, false);
    }
}

/// Read the feed now.
pub fn check(shell: &Shell, by_hand: bool) {
    if flatpak() || with(|s| std::mem::replace(&mut s.checking, true)) {
        return;
    }
    redraw(shell);
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        let result = fetch_manifest(&feed()).await;
        let Some(shell) = weak.upgrade() else { return };
        with(|s| s.checking = false);
        match result {
            Ok(m) => {
                with(|s| {
                    s.error = None;
                    s.file.checked_at = Some(SwiftDate::now());
                    let mut latest = LatestRelease {
                        version: m.version.clone(),
                        release_id: m.release_id.clone(),
                        commit: m.commit.clone(),
                        published_at: m.published_at.clone(),
                        notes: Some(m.notes.clone()).filter(|n| !n.trim().is_empty()),
                        ..Default::default()
                    };
                    if !m.latest_url.is_empty() {
                        latest
                            .extra
                            .insert("latestUrl".into(), m.latest_url.clone().into());
                    }
                    s.file.latest = Some(latest);
                });
                let newer = is_newer(&m.version, version());
                tracing::info!(latest = %m.version, current = version(), newer, "update check");
                if newer {
                    let first_time = with(|s| {
                        let first = s.file.announced_version.as_deref() != Some(m.version.as_str());
                        s.file.announced_version = Some(m.version.clone());
                        first
                    });
                    if first_time || by_hand {
                        announce(&shell, &m.version);
                    }
                } else if by_hand {
                    shell.toast("Copper is up to date");
                }
                save(&shell);
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check");
                with(|s| s.error = Some(e));
                if by_hand {
                    shell.toast("Couldn’t check for updates just now");
                }
            }
        }
        redraw(&shell);
    });
}

fn save(shell: &Shell) {
    let file = with(|s| s.file.clone());
    shell.saver().schedule(file);
}

/// One line, with the way to it.
fn announce(shell: &Shell, v: &str) {
    let t = adw::Toast::new(&format!("Copper {v} is available"));
    t.set_button_label(Some("Download"));
    t.set_timeout(10);
    let weak = shell.downgrade();
    t.connect_button_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            open_release(&s);
        }
    });
    shell.ui().toasts.add_toast(t);
}

/// The release page for the newest version.
pub fn release_page() -> String {
    with(|s| {
        let l = s.file.latest.as_ref()?;
        if !l.release_id.is_empty() {
            return Some(format!(
                "https://github.com/copper-browser/copper-linux/releases/tag/{}",
                l.release_id
            ));
        }
        l.extra
            .get("latestUrl")
            .and_then(|v| v.as_str())
            .filter(|u| u.starts_with("https://"))
            .map(str::to_string)
    })
    .unwrap_or_else(|| RELEASES.to_string())
}

fn open_release(shell: &Shell) {
    let launcher = gtk::UriLauncher::new(&release_page());
    launcher.launch(Some(shell.window()), gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            tracing::warn!(error = %e, "release page");
        }
    });
}

/// A newer version is known.
pub fn available() -> Option<String> {
    with(|s| {
        s.file
            .latest
            .as_ref()
            .filter(|l| is_newer(&l.version, version()))
            .map(|l| l.version.clone())
    })
}

/// `a` is a newer version than `b` (semver order; a pre-release sorts
/// before its release).
pub fn is_newer(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> (Vec<u64>, Option<String>) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p.to_string())),
            None => (v, None),
        };
        let core = core.split('+').next().unwrap_or(core);
        let mut nums: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        nums.resize(3, 0);
        (nums, pre)
    }
    let (an, ap) = parse(a);
    let (bn, bp) = parse(b);
    match an.cmp(&bn) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (ap, bp) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => x > y,
            _ => false,
        },
    }
}

// MARK: - the request

async fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    let body = get(url).await?;
    serde_json::from_slice::<Manifest>(&body).map_err(|e| format!("The feed made no sense ({e})"))
}

/// GET `url` (following redirects) with GIO's own sockets and TLS — the
/// same glib-networking WebKit uses, so nothing extra is needed.
pub async fn get(url: &str) -> Result<Vec<u8>, String> {
    let overridden = std::env::var_os("COPPER_UPDATE_FEED").is_some();
    let mut url = url.to_string();
    for _ in 0..6 {
        let u = url::Url::parse(&url).map_err(|e| e.to_string())?;
        let https = match u.scheme() {
            "https" => true,
            "http" if overridden => false,
            other => return Err(format!("Won’t fetch a {other} address")),
        };
        let host = u.host_str().ok_or("No host")?.to_string();
        let port = u
            .port_or_known_default()
            .unwrap_or(if https { 443 } else { 80 });
        let path = match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        };
        let client = gio::SocketClient::new();
        client.set_tls(https);
        client.set_timeout(20);
        let conn = client
            .connect_to_host_future(&host, port)
            .await
            .map_err(|e| e.message().to_string())?;
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Copper/{} (Linux)\r\nAccept: application/json, */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
            version()
        );
        conn.output_stream()
            .write_all_future(request.into_bytes(), glib::Priority::DEFAULT)
            .await
            .map_err(|(_, e)| e.message().to_string())?;
        let input = conn.input_stream();
        let mut raw = Vec::new();
        loop {
            let chunk = input
                .read_bytes_future(16 * 1024, glib::Priority::DEFAULT)
                .await
                .map_err(|e| e.message().to_string())?;
            if chunk.is_empty() {
                break;
            }
            raw.extend_from_slice(&chunk);
            if raw.len() > 4 * 1024 * 1024 {
                return Err("The answer was too big".into());
            }
        }
        let _ = conn.close_future(glib::Priority::DEFAULT).await;
        let reply = parse_reply(&raw)?;
        match reply.status {
            200 => return Ok(reply.body),
            301 | 302 | 303 | 307 | 308 => {
                let to = reply.location.ok_or("A redirect to nowhere")?;
                url = u.join(&to).map_err(|e| e.to_string())?.to_string();
            }
            404 => return Err("No release has been published yet".into()),
            s => return Err(format!("The server said {s}")),
        }
    }
    Err("Too many redirects".into())
}

struct Reply {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
}

fn parse_reply(raw: &[u8]) -> Result<Reply, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("An answer without headers")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.split("\r\n");
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or("A malformed answer")?;
    let mut location = None;
    let mut chunked = false;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "location" {
            location = Some(v.to_string());
        } else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }
    if chunked {
        body = dechunk(&body)?;
    }
    Ok(Reply {
        status,
        location,
        body,
    })
}

fn dechunk(mut b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = b
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("A broken chunk")?;
        let size_text = String::from_utf8_lossy(&b[..eol]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| "A broken chunk size")?;
        b = &b[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if b.len() < size {
            return Err("A short chunk".into());
        }
        out.extend_from_slice(&b[..size]);
        b = b.get(size + 2..).unwrap_or(&[]);
    }
}

// MARK: - Settings › Updates

struct View {
    group: glib::WeakRef<adw::PreferencesGroup>,
    rows: RefCell<Vec<gtk::Widget>>,
    note: glib::WeakRef<gtk::Label>,
}

fn redraw(shell: &Shell) {
    if let Some(v) = with(|s| s.view.clone()) {
        v.draw(shell);
    }
}

pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let g = group(Some("This Copper"));
    page.add(&g);
    let note = gtk::Label::new(None);
    note.add_css_class("copper-settings-note");
    note.set_xalign(0.0);
    note.set_wrap(true);
    let notes = group(None);
    notes.add(&note);
    page.add(&notes);
    let view = Rc::new(View {
        group: g.downgrade(),
        rows: RefCell::default(),
        note: note.downgrade(),
    });
    view.draw(shell);
    with(|s| s.view = Some(view));
    page
}

impl View {
    fn draw(&self, shell: &Shell) {
        let (Some(g), Some(note)) = (self.group.upgrade(), self.note.upgrade()) else {
            return;
        };
        for r in self.rows.borrow_mut().drain(..) {
            if let Some(row) = r.downcast_ref::<adw::PreferencesRow>() {
                g.remove(row);
            }
        }
        let mut rows: Vec<gtk::Widget> = Vec::new();
        if flatpak() {
            let r = line(
                &format!("You have Copper {}", version()),
                "Updates come through Flatpak — your software centre, or flatpak update, installs new versions",
            );
            rows.push(r.upcast());
            note.set_text("This Copper is a Flatpak, so it doesn’t look for updates itself.");
        } else {
            let (checking, error, latest, checked) = with(|s| {
                (
                    s.checking,
                    s.error.clone(),
                    s.file.latest.clone(),
                    s.file.checked_at.map(SwiftDate::unix),
                )
            });
            let newer = latest.as_ref().filter(|l| is_newer(&l.version, version()));
            let status = if checking {
                "Checking…".to_string()
            } else if let Some(l) = newer {
                let mut s = format!("Copper {} is available", l.version);
                if let Some(d) = published(&l.published_at) {
                    s.push_str(&format!(" · {d}"));
                }
                if error.is_some() {
                    s.push_str(" · couldn’t check again just now");
                }
                s
            } else if let Some(e) = &error {
                e.clone()
            } else if latest.is_none() {
                "Not checked yet — Copper looks every six hours on its own".to_string()
            } else {
                "Up to date".to_string()
            };
            let r = line(&format!("You have Copper {}", version()), &status);
            let b = button(if checking { "Checking…" } else { "Check now" }, false);
            b.set_sensitive(!checking);
            let weak = shell.downgrade();
            b.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    check(&s, true);
                }
            });
            r.add_suffix(&b);
            rows.push(r.upcast());
            if let Some(l) = newer {
                let r = line(
                    &format!("Copper {} is available", l.version),
                    "The .deb and the Flatpak are on the release page. Your package manager installs it — Copper never replaces itself.",
                );
                let d = button("Download", true);
                let weak = shell.downgrade();
                d.connect_clicked(move |_| {
                    if let Some(s) = weak.upgrade() {
                        open_release(&s);
                    }
                });
                r.add_suffix(&d);
                rows.push(r.upcast());
                if let Some(n) = l.notes.as_deref().filter(|n| !n.trim().is_empty()) {
                    rows.push(line(&format!("What’s new in {}", l.version), n).upcast());
                }
            }
            let mut text = "Updates come from github.com/copper-browser/copper-linux".to_string();
            if let Some(t) = checked {
                text.push_str(&format!(" · last checked {}", format::ago(t, now_unix())));
            }
            note.set_text(&text);
        }
        for r in &rows {
            if let Some(row) = r.downcast_ref::<adw::PreferencesRow>() {
                g.add(row);
            }
        }
        *self.rows.borrow_mut() = rows;
    }
}

/// `Oct 12, 2026` from the feed's RFC 3339 time.
fn published(raw: &str) -> Option<String> {
    let t = glib::DateTime::from_iso8601(raw, None).ok()?;
    t.format("%b %-d, %Y").ok().map(|s| s.to_string())
}

/// Check every so often without blocking (tests).
#[allow(dead_code)]
pub async fn wait_checked(limit: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if !with(|s| s.checking) && with(|s| s.file.checked_at.is_some() || s.error.is_some()) {
            return true;
        }
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order() {
        assert!(is_newer("0.1.0", "0.0.1"));
        assert!(is_newer("v1.2.10", "1.2.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("0.1.0", "0.1.0-rc.1"));
        assert!(!is_newer("0.1.0-rc.1", "0.1.0"));
        assert!(is_newer("0.1.0-rc.2", "0.1.0-rc.1"));
    }

    #[test]
    fn replies_parse() {
        let r = parse_reply(b"HTTP/1.1 302 Found\r\nLocation: https://x.test/a\r\n\r\n").unwrap();
        assert_eq!(
            (r.status, r.location.as_deref()),
            (302, Some("https://x.test/a"))
        );
        let r = parse_reply(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.body, b"{\"a\":1}");
        let m: Manifest = serde_json::from_str(
            r#"{"schemaVersion":1,"version":"0.2.0","releaseId":"v0.2.0","assets":[]}"#,
        )
        .unwrap();
        assert_eq!(m.version, "0.2.0");
    }
}
