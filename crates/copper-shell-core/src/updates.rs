//! Knowing when there is a newer Copper (Mac: `Updater.swift`,
//! `Fork/Updates.swift`, Settings › About).
//!
//! Every six hours (and on Check now) Copper reads the platform's
//! `copper-version.json` ([`Channel::feed`], the repo's GitHub releases);
//! when it names a newer version it says "Copper X is available" once, with
//! a Download button that opens the release page. It never replaces files:
//! the package manager (or the installer) owns them. When something else
//! installs updates ([`Channel::managed`]: Flatpak) it does not look at all.
//!
//! `COPPER_UPDATE_FEED` points the check elsewhere (tests); only then is
//! plain `http` accepted. What was last seen lives in `updates.json`.
//!
//! The app supplies the socket, the announcement and Settings › About
//! ([`UpdatesApp`]; GTK: `copper-app`'s `features::updates`).

use crate::Platform;
use crate::main_loop::MainLoop;
use crate::shell::Shell;
use copper_core::date::{SwiftDate, now_unix};
use copper_core::updates::{LatestRelease, UpdatesFile};
use copper_engine::LocalFuture;
use serde::Deserialize;
use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// How often to look.
const EVERY: f64 = 6.0 * 3600.0;
/// The most of an answer [`UpdatesApp::exchange`] reads.
pub const MAX_ANSWER: usize = 4 * 1024 * 1024;

/// Where a platform's releases are published.
#[derive(Clone, Copy, Debug)]
pub struct Channel {
    /// Where the version file lives.
    pub feed: &'static str,
    /// The page people download from.
    pub releases: &'static str,
    /// A release's own page, before its tag (`…/releases/tag/`).
    pub tag_page: &'static str,
    /// The system in the `User-Agent` (`Copper/x (Linux)`).
    pub system: &'static str,
    /// Something else installs updates (Flatpak): Copper does not look.
    pub managed: fn() -> bool,
}

/// What the app gives the update check.
pub trait UpdatesApp<P: Platform>: 'static {
    /// Where to look.
    fn channel(&self) -> Channel;
    /// Send `request` (a whole HTTP/1.1 request, `Connection: close`) to
    /// `host:port` — over TLS when `tls` — and read the whole answer, at
    /// most [`MAX_ANSWER`] bytes (an `Err` past it). GTK's uses GIO's
    /// sockets and TLS, the same glib-networking WebKit uses.
    fn exchange(
        &self,
        host: String,
        port: u16,
        tls: bool,
        request: Vec<u8>,
    ) -> LocalFuture<Result<Vec<u8>, String>>;
    /// "Copper `version` is available", with the way to it ([`release_page`]).
    fn announce(&self, shell: &Shell<P>, version: &str);
    /// What the check knows changed (Settings › About redraws).
    fn changed(&self, shell: &Shell<P>);
}

/// The running app's version ([`copper_core::version::app`]: Copper for
/// Linux's or Copper for Windows', whichever binary set it).
pub fn version() -> &'static str {
    copper_core::version::app()
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

struct State<P: Platform> {
    file: UpdatesFile,
    checking: bool,
    error: Option<String>,
    app: Option<Rc<dyn UpdatesApp<P>>>,
    started: bool,
}

impl<P: Platform> Default for State<P> {
    fn default() -> Self {
        State {
            file: UpdatesFile::default(),
            checking: false,
            error: None,
            app: None,
            started: false,
        }
    }
}

thread_local! {
    /// The `State<P>` (one platform per process), made on first use.
    static STATE: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

fn with<P: Platform, R>(f: impl FnOnce(&mut State<P>) -> R) -> R {
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        if !slot.as_ref().is_some_and(|a| a.is::<State<P>>()) {
            *slot = Some(Box::new(State::<P>::default()));
        }
        let state = slot
            .as_mut()
            .and_then(|a| a.downcast_mut::<State<P>>())
            .expect("made just above");
        f(state)
    })
}

fn app<P: Platform>() -> Option<Rc<dyn UpdatesApp<P>>> {
    with::<P, _>(|s| s.app.clone())
}

fn managed<P: Platform>() -> bool {
    app::<P>().is_some_and(|a| (a.channel().managed)())
}

fn feed<P: Platform>() -> String {
    std::env::var("COPPER_UPDATE_FEED")
        .ok()
        .filter(|f| !f.trim().is_empty())
        .or_else(|| app::<P>().map(|a| a.channel().feed.to_string()))
        .unwrap_or_default()
}

/// At launch: read `updates.json`; look when due (a little after the window
/// is up), then every hour see whether six have passed.
pub fn start<P: Platform>(shell: &Shell<P>, app: Rc<dyn UpdatesApp<P>>) {
    let managed = (app.channel().managed)();
    with::<P, _>(|s| s.app = Some(app));
    match shell.store().load::<UpdatesFile>() {
        Ok(l) => with::<P, _>(|s| s.file = l.value),
        Err(e) => tracing::warn!(error = %e, "updates.json"),
    }
    if managed || with::<P, _>(|s| std::mem::replace(&mut s.started, true)) {
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
    P::Loop::spawn_local(async move {
        P::Loop::sleep(Duration::from_secs(first)).await;
        if let Some(s) = weak.upgrade() {
            check_if_due(&s);
        }
    });
    let weak = shell.downgrade();
    P::Loop::spawn_local(async move {
        loop {
            P::Loop::sleep(Duration::from_secs(3600)).await;
            let Some(s) = weak.upgrade() else { break };
            check_if_due(&s);
        }
    });
}

fn check_if_due<P: Platform>(shell: &Shell<P>) {
    let last = with::<P, _>(|s| s.file.checked_at.map(SwiftDate::unix));
    let due = last.is_none_or(|t| now_unix() - t >= EVERY)
        || std::env::var_os("COPPER_UPDATE_FEED").is_some();
    if due {
        check(shell, false);
    }
}

/// Read the feed now.
pub fn check<P: Platform>(shell: &Shell<P>, by_hand: bool) {
    if managed::<P>() || with::<P, _>(|s| std::mem::replace(&mut s.checking, true)) {
        return;
    }
    changed(shell);
    let weak = shell.downgrade();
    P::Loop::spawn_local(async move {
        let result = fetch_manifest::<P>(&feed::<P>()).await;
        let Some(shell) = weak.upgrade() else { return };
        with::<P, _>(|s| s.checking = false);
        match result {
            Ok(m) => {
                with::<P, _>(|s| {
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
                    let first_time = with::<P, _>(|s| {
                        let first = s.file.announced_version.as_deref() != Some(m.version.as_str());
                        s.file.announced_version = Some(m.version.clone());
                        first
                    });
                    if (first_time || by_hand)
                        && let Some(a) = app::<P>()
                    {
                        a.announce(&shell, &m.version);
                    }
                } else if by_hand {
                    shell.toast("Copper is up to date");
                }
                save(&shell);
            }
            Err(e) => {
                tracing::warn!(error = %e, "update check");
                with::<P, _>(|s| s.error = Some(e));
                if by_hand {
                    shell.toast("Couldn’t check for updates just now");
                }
            }
        }
        changed(&shell);
    });
}

fn changed<P: Platform>(shell: &Shell<P>) {
    if let Some(a) = app::<P>() {
        a.changed(shell);
    }
}

fn save<P: Platform>(shell: &Shell<P>) {
    let file = with::<P, _>(|s| s.file.clone());
    shell.saver().schedule(file);
}

/// The release page for the newest version.
pub fn release_page<P: Platform>() -> String {
    let channel = app::<P>().map(|a| a.channel());
    with::<P, _>(|s| {
        let l = s.file.latest.as_ref()?;
        if !l.release_id.is_empty() {
            return Some(format!("{}{}", channel?.tag_page, l.release_id));
        }
        l.extra
            .get("latestUrl")
            .and_then(|v| v.as_str())
            .filter(|u| u.starts_with("https://"))
            .map(str::to_string)
    })
    .or_else(|| channel.map(|c| c.releases.to_string()))
    .unwrap_or_default()
}

/// A newer version is known.
pub fn available<P: Platform>() -> Option<String> {
    with::<P, _>(|s| {
        s.file
            .latest
            .as_ref()
            .filter(|l| is_newer(&l.version, version()))
            .map(|l| l.version.clone())
    })
}

/// What the check knows, for Settings › About.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// A check is under way.
    pub checking: bool,
    /// Why the last check failed.
    pub error: Option<String>,
    /// What the feed last said.
    pub latest: Option<LatestRelease>,
    /// When it last answered (Unix seconds).
    pub checked: Option<f64>,
}

/// What the check knows now.
pub fn status<P: Platform>() -> Status {
    with::<P, _>(|s| Status {
        checking: s.checking,
        error: s.error.clone(),
        latest: s.file.latest.clone(),
        checked: s.file.checked_at.map(SwiftDate::unix),
    })
}

/// Wait (polling, never blocking) until a check has answered (tests).
pub async fn wait_checked<P: Platform>(limit: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if with::<P, _>(|s| !s.checking && (s.file.checked_at.is_some() || s.error.is_some())) {
            return true;
        }
        P::Loop::sleep(Duration::from_millis(100)).await;
    }
    false
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

async fn fetch_manifest<P: Platform>(url: &str) -> Result<Manifest, String> {
    let body = get::<P>(url).await?;
    serde_json::from_slice::<Manifest>(&body).map_err(|e| format!("The feed made no sense ({e})"))
}

/// GET `url`, following redirects, over the app's sockets
/// ([`UpdatesApp::exchange`]).
pub async fn get<P: Platform>(url: &str) -> Result<Vec<u8>, String> {
    let app = app::<P>().ok_or("Updates are not set up")?;
    let overridden = std::env::var_os("COPPER_UPDATE_FEED").is_some();
    let system = app.channel().system;
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
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Copper/{} ({system})\r\nAccept: application/json, */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
            version()
        );
        let raw = app
            .exchange(host, port, https, request.into_bytes())
            .await?;
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
