//! Canvas: a whiteboard in a tab (Mac: `Fork/Canvas/`, docs/canvas.md).
//!
//! * [`scheme`] serves `copper://canvas/<id>` — the bundled board page
//!   (`resources/canvas.html`) — into a tab's page;
//! * [`host`] is the bridge between that page and Copper (the message
//!   handler, the `window.copperCanvas` calls, the room relay);
//! * [`registry`] is `canvas/canvases.json`: Personal (always, local until
//!   its sync is chosen), local canvases, shared ones from Copper Cloud;
//! * [`store`] is each canvas's Yjs history on disk;
//! * [`mcp`] is the `canvas_*` tools.

pub mod chatprefs;
pub mod host;
pub mod mcp;
pub mod poster;
pub mod registry;
pub mod scheme;
pub mod store;

use crate::actions::{Command, register_action, register_command};
use crate::shell::{OpenTab, Shell, WeakShell};
use copper_core::TabId;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// The board page, bundled.
pub const CANVAS_HTML: &str = include_str!("resources/canvas.html");
/// The "Switching from Chrome" guide's ops (Flow's import lands it).
pub const SWITCHING_FROM_CHROME: &str = include_str!("resources/switching-from-chrome.json");

const PRESENCE: [&str; 9] = [
    "#E5484D", "#F76B15", "#D6A400", "#30A46C", "#12A594", "#0090FF", "#3E63DD", "#8E4EC6",
    "#D6409F",
];

thread_local! {
    static SHELL: RefCell<Option<WeakShell>> = const { RefCell::new(None) };
    static REFRESHING: Cell<bool> = const { Cell::new(false) };
    static AGAIN: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn shell() -> Option<Shell> {
    SHELL.with(|s| s.borrow().as_ref().and_then(WeakShell::upgrade))
}

/// A presence colour, the same every time for the same seed (Mac
/// `CanvasColors.stable`: FNV-1a over the lowercased UTF-8).
pub fn stable_color(seed: &str) -> String {
    let mut hash: u32 = 2_166_136_261;
    for b in seed.to_lowercase().bytes() {
        hash = (hash ^ b as u32).wrapping_mul(16_777_619);
    }
    PRESENCE[(hash % PRESENCE.len() as u32) as usize].to_string()
}

/// `copper://canvas/<id>`.
pub fn url_for(id: &str) -> String {
    let safe: String = url::form_urlencoded::byte_serialize(id.as_bytes())
        .collect::<String>()
        .replace('+', "%20");
    format!("copper://canvas/{safe}")
}

/// The canvas a `copper://canvas…` address names (`copper://canvas` alone
/// is Personal). Join links are not canvases.
pub fn id_from_url(text: &str) -> Option<String> {
    let u = url::Url::parse(text).ok()?;
    if !u.scheme().eq_ignore_ascii_case("copper") || !u.host_str()?.eq_ignore_ascii_case("canvas") {
        return None;
    }
    let path = u.path().trim_matches('/');
    if path.is_empty() {
        return Some(copper_core::canvas::PERSONAL_CANVAS_ID.into());
    }
    if path.eq_ignore_ascii_case("join") || path.to_ascii_lowercase().starts_with("join/") {
        return None;
    }
    let first = path.split('/').next().unwrap_or(path);
    let id = percent_decode(first);
    (!id.is_empty()).then_some(id)
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// A join link (`copper://canvas/join/<token>?cloud=…&n=…`, or the web form
/// on the linked cloud).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinLink {
    pub token: String,
    pub cloud: String,
    pub name: Option<String>,
    pub copper: bool,
}

impl JoinLink {
    pub fn parse(text: &str) -> Option<JoinLink> {
        let u = url::Url::parse(text.trim()).ok()?;
        let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() != 2 || !parts[0].eq_ignore_ascii_case("join") || parts[1].is_empty() {
            return None;
        }
        let query = |k: &str| {
            u.query_pairs()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.into_owned())
        };
        let (cloud, copper) = match u.scheme() {
            "copper" => {
                if !u.host_str()?.eq_ignore_ascii_case("canvas") {
                    return None;
                }
                (query("cloud").filter(|c| !c.is_empty())?, true)
            }
            "https" => {
                let host = u.host_str()?.to_string();
                let port = u.port().map(|p| format!(":{p}")).unwrap_or_default();
                (format!("{host}{port}"), false)
            }
            _ => return None,
        };
        Some(JoinLink {
            token: parts[1].to_string(),
            cloud,
            name: query("n"),
            copper,
        })
    }

    /// `host[:port]` compared as HTTPS does (443 is the same as none).
    pub fn normalize(cloud: &str) -> String {
        let mut h = cloud.trim().to_lowercase();
        if h.ends_with(":443") {
            h.truncate(h.len() - 4);
        }
        h
    }
}

/// `dark` or `light`, as the window wears it.
pub fn theme() -> String {
    if libadwaita::StyleManager::default().is_dark() {
        "dark".into()
    } else {
        "light".into()
    }
}

/// Which canvases are open / live, for the lists (recounted on changes).
pub fn recount() {
    for h in host::all() {
        h.status_now();
    }
}

pub fn register(shell: &Shell) {
    SHELL.with(|s| *s.borrow_mut() = Some(shell.downgrade()));
    scheme::install();
    scheme::register_host("canvas", Rc::new(serve));

    register_action(shell, "new-canvas", &["<Control><Shift>e"], |s| {
        new_canvas(s, None)
    });
    register_command(
        shell,
        Command::new("new-canvas", "New Canvas", "document-edit-symbolic", |s| {
            new_canvas(s, None)
        })
        .shortcut("Ctrl+Shift+E"),
    );
    register_command(
        shell,
        Command::new(
            "personal-canvas",
            "Personal Canvas",
            "document-edit-symbolic",
            |s| {
                show(s, copper_core::canvas::PERSONAL_CANVAS_ID, true);
            },
        ),
    );

    // Closed tabs lose their bridge.
    let mut events = shell.hub().subscribe();
    glib::spawn_future_local(async move {
        loop {
            match events.recv().await {
                Ok(crate::control::ShellEvent::TabClosed(id)) => host::drop_tab(&id),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
    watch_cloud();
    // The board follows the window's light or dark.
    libadwaita::StyleManager::default().connect_dark_notify(|_| {
        for h in host::all() {
            h.theme_changed();
        }
    });
}

/// Follow the cloud: sign-ins and sign-outs, and the event stream's
/// `canvas` events, re-read the list and move rooms.
fn watch_cloud() {
    let Some(cloud) = crate::cloud::get() else {
        return;
    };
    let mut status = cloud.client.watch_status();
    glib::spawn_future_local(async move {
        let mut was = (
            status.borrow().signed_in,
            status.borrow().account.clone().map(|a| a.user_id),
        );
        if was.0 {
            refresh_soon();
        }
        while status.changed().await.is_ok() {
            let now = (
                status.borrow().signed_in,
                status.borrow().account.clone().map(|a| a.user_id),
            );
            if now != was {
                was = now;
                cloud_changed();
                if was.0 {
                    refresh_soon();
                }
            }
        }
    });
    let mut events = cloud.client.subscribe();
    glib::spawn_future_local(async move {
        loop {
            match events.recv().await {
                Ok(e) if e.kind == "canvas" || e.kind == "resync" || e.kind == "open" => {
                    refresh_soon()
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => refresh_soon(),
                Err(_) => break,
            }
        }
    });
}

/// Signed in or out, another account, the Personal switch: pages whose room
/// is no longer right reload; the rest hear the new status.
pub fn cloud_changed() {
    for h in host::all() {
        h.cloud_changed();
    }
    recount();
}

/// Read the server's list soon (coalesced).
pub fn refresh_soon() {
    if REFRESHING.with(|r| r.get()) {
        AGAIN.with(|a| a.set(true));
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(150), || {
        glib::spawn_future_local(refresh());
    });
}

/// `GET /v1/canvases` for the account signed in now.
pub async fn refresh() {
    if REFRESHING.with(|r| r.replace(true)) {
        AGAIN.with(|a| a.set(true));
        return;
    }
    loop {
        AGAIN.with(|a| a.set(false));
        refresh_once().await;
        if !AGAIN.with(|a| a.get()) {
            break;
        }
    }
    REFRESHING.with(|r| r.set(false));
}

async fn refresh_once() {
    let Some(cloud) = crate::cloud::get() else {
        return;
    };
    if !registry::cloud_ready() {
        return;
    }
    let Some(account) = cloud
        .client
        .account()
        .map(|a| a.user_id.to_ascii_lowercase())
    else {
        return;
    };
    let api = cloud.canvas.clone();
    let rows = match crate::cloud::spawn(async move { api.list().await }).await {
        Ok(Ok(rows)) => rows,
        Ok(Err(f)) => {
            registry::set_problem(Some(f.message));
            return;
        }
        Err(_) => return,
    };
    let still = cloud
        .client
        .account()
        .map(|a| a.user_id.to_ascii_lowercase());
    if still.as_deref() != Some(account.as_str()) {
        AGAIN.with(|a| a.set(true));
        return;
    }
    let adopted = registry::adopt(&rows, &account);
    for id in &adopted.renamed {
        for h in host::hosts_of(id) {
            h.renamed();
        }
    }
    for id in &adopted.seen {
        for h in host::hosts_of(id) {
            h.share_now();
        }
    }
    for id in &adopted.gone {
        removed(id);
    }
    cloud_changed();
}

/// Deleted, left or revoked: its tabs close.
pub fn removed(id: &str) {
    let Some(shell) = shell() else { return };
    let tabs: Vec<TabId> = shell
        .browser()
        .all_tabs()
        .filter(|t| id_from_url(&t.url).as_deref() == Some(id))
        .map(|t| t.id.clone())
        .collect();
    for t in tabs {
        host::drop_tab(&t);
        shell.close_force(&t);
    }
}

/// The tab showing canvas `id`, if any.
pub fn tab_showing(shell: &Shell, id: &str) -> Option<TabId> {
    let b = shell.browser();
    let active = b.active_id();
    let mut hits: Vec<TabId> = b
        .all_tabs()
        .filter(|t| !t.private && id_from_url(&t.url).as_deref() == Some(id))
        .map(|t| t.id.clone())
        .collect();
    // The one in front wins.
    if let Some(a) = active
        && hits.contains(&a)
    {
        return Some(a);
    }
    hits.sort_by(|x, y| x.0.cmp(&y.0));
    hits.into_iter().next()
}

/// The canvas in front, if the active tab shows one.
pub fn active(shell: &Shell) -> Option<String> {
    let b = shell.browser();
    let t = b.active()?;
    id_from_url(&t.url)
}

/// Show canvas `id`: its tab if it has one (selected when `foreground`),
/// else a new one. The tab's page is made at once either way.
pub fn show(shell: &Shell, id: &str, foreground: bool) -> TabId {
    if let Some(t) = tab_showing(shell, id) {
        if foreground {
            shell.select(&t);
        }
        shell.ensure_page(&t);
        return t;
    }
    let name = registry::entry(id)
        .map(|e| e.name)
        .unwrap_or_else(|| "Canvas".into());
    let t = shell.open(
        &url_for(id),
        OpenTab {
            title: name,
            foreground,
            at_end: true,
            ..Default::default()
        },
    );
    shell.ensure_page(&t);
    t
}

/// The canvas's tab (made if there is none) and its host once the page has one.
pub async fn open(
    shell: &Shell,
    id: &str,
    foreground: bool,
) -> Result<Rc<host::CanvasHost>, String> {
    let name = registry::entry(id)
        .map(|e| e.name)
        .unwrap_or_else(|| id.to_string());
    let tab = show(shell, id, foreground);
    // The host comes when WebKit asks the tab's page for the canvas address
    // (`scheme`): first the tab needs a page (it may be asleep), then the
    // page has to start loading. Each may take a while on a busy machine;
    // neither may stall.
    let started = std::time::Instant::now();
    let mut stage = "building the tab's page";
    let mut since = started;
    let mut stamped: Option<std::time::Instant> = None;
    let mut reloaded = false;
    loop {
        if let Some(h) = host::for_tab(&tab)
            && h.canvas_id() == id
        {
            return Ok(h);
        }
        if shell.browser().tab(&tab).is_none() {
            return Err(format!("the canvas tab for {name} was closed"));
        }
        if stamped.is_none_or(|t| t.elapsed() >= host::STAMP_EVERY) {
            stamped = Some(std::time::Instant::now());
            host::keep_awake(shell, &tab);
        }
        let now = match shell.slot(&tab) {
            None => {
                shell.ensure_page(&tab);
                "building the tab's page"
            }
            Some(slot) if slot.crashed.get() && !reloaded => {
                reloaded = true;
                shell.reload(&tab);
                "loading the canvas page"
            }
            Some(_) => "loading the canvas page",
        };
        if now != stage {
            stage = now;
            since = std::time::Instant::now();
        }
        if since.elapsed() >= host::STALL {
            return Err(format!(
                "the canvas tab for {name} didn't open: still {stage} after {} s",
                since.elapsed().as_secs()
            ));
        }
        glib::timeout_future(Duration::from_millis(60)).await;
    }
}

/// "New Canvas": shared when signed in, on this device otherwise.
pub fn new_canvas(shell: &Shell, name: Option<String>) {
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        let name = name.unwrap_or_else(|| "Untitled canvas".into());
        match create(&name).await {
            Ok(e) => {
                if let Some(s) = weak.upgrade() {
                    show(&s, &e.id, true);
                }
            }
            Err(text) => {
                if let Some(s) = weak.upgrade() {
                    s.toast(&text);
                }
            }
        }
    });
}

/// Make a canvas: on the server when signed in, else local.
pub async fn create(name: &str) -> Result<copper_core::canvas::CanvasEntry, String> {
    let clean = registry::clean(name);
    let Some(cloud) = crate::cloud::get().filter(|_| registry::cloud_ready()) else {
        return Ok(registry::create_local(&clean));
    };
    let Some(account) = cloud
        .client
        .account()
        .map(|a| a.user_id.to_ascii_lowercase())
    else {
        return Ok(registry::create_local(&clean));
    };
    let api = cloud.canvas.clone();
    let made = clean.clone();
    let info = crate::cloud::spawn(async move { api.create(&made).await })
        .await
        .map_err(|_| "stopped".to_string())?
        .map_err(|f| f.message)?;
    let mut info = info;
    if info.role.is_empty() {
        info.role = "owner".into();
    }
    if info.member_count.is_none() {
        info.member_count = Some(1);
    }
    registry::upsert(&info, &account)
        .ok_or_else(|| "The server's answer had no canvas in it".into())
}

/// The board page for `copper://canvas/…`, with the tab's bridge attached.
fn serve(shell: &Shell, tab: &TabId, uri: &str) -> scheme::Answer {
    if let Some(link) = JoinLink::parse(uri) {
        let s = shell.clone();
        let t = tab.clone();
        glib::spawn_future_local(async move {
            let said = join(&s, &link, Some(&t)).await;
            if let Err(text) = said {
                s.toast(&text);
            }
        });
        let page = "<!doctype html><meta charset=utf-8><meta name=color-scheme content='light dark'><title>Canvas</title><body style='font:15px system-ui;padding:3em;opacity:.7'>Opening the canvas…</body>";
        return Ok((page.as_bytes().to_vec(), "text/html"));
    }
    let id = id_from_url(uri).ok_or("not a canvas address")?;
    if registry::entry(&id).is_none() {
        return Err(format!("no canvas {id}"));
    }
    host::CanvasHost::attach(shell, tab, &id);
    Ok((CANVAS_HTML.as_bytes().to_vec(), "text/html"))
}

/// Open the canvas a join link names (Mac `CanvasJoinFlow.join`). `in_tab`:
/// the tab that navigated to the link, which then shows the canvas.
pub async fn join(
    shell: &Shell,
    link: &JoinLink,
    in_tab: Option<&TabId>,
) -> Result<String, String> {
    let Some(cloud) = crate::cloud::get() else {
        return Err("Copper Cloud is off this run".into());
    };
    let Some(linked) = cloud.client.link() else {
        crate::cloud::open_settings(shell);
        return Err(format!(
            "This canvas is on {}. Connect to that Copper Cloud to open it.",
            link.cloud
        ));
    };
    if JoinLink::normalize(&linked.host()) != JoinLink::normalize(&link.cloud) {
        crate::cloud::open_settings(shell);
        return Err(format!(
            "This canvas is on {}. Connect to that Copper Cloud to open it.",
            link.cloud
        ));
    }
    if !cloud.client.is_signed_in() {
        crate::cloud::open_settings(shell);
        return Err("Sign in to Copper Cloud to open this canvas.".into());
    }
    let account = cloud
        .client
        .account()
        .map(|a| a.user_id.to_ascii_lowercase())
        .unwrap_or_default();
    let api = cloud.canvas.clone();
    let token = link.token.clone();
    let preview = {
        let api = api.clone();
        let token = token.clone();
        crate::cloud::spawn(async move { api.preview_link(&token).await })
            .await
            .map_err(|_| "stopped".to_string())?
    };
    let preview = match preview {
        Ok(v) => v,
        Err(f) if f.status == 404 => {
            return Err("This invite link no longer works — ask for a new one.".into());
        }
        Err(f) => return Err(f.message),
    };
    let member = preview
        .get("member")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let known = preview
        .get("canvas_id")
        .and_then(|v| v.as_str())
        .and_then(registry::entry);
    let entry = if member && let Some(e) = known {
        e
    } else {
        let info = crate::cloud::spawn(async move { api.join_link(&token).await })
            .await
            .map_err(|_| "stopped".to_string())?
            .map_err(|f| {
                if f.status == 404 {
                    "This invite link no longer works — ask for a new one.".to_string()
                } else {
                    f.message
                }
            })?;
        let e = registry::upsert(&info, &account).ok_or("The invite did not contain a canvas")?;
        refresh().await;
        e
    };
    match in_tab {
        Some(t) if shell.browser().tab(t).is_some() => shell.navigate(t, &url_for(&entry.id)),
        _ => {
            show(shell, &entry.id, true);
        }
    }
    Ok(format!("Joined “{}” from {}", entry.name, link.cloud))
}

/// Land the "Switching from Chrome" guide as a local canvas (Flow's import
/// calls this). Returns the canvas id.
pub async fn land_chrome_canvas(shell: &Shell) -> Option<String> {
    const NAME: &str = "Switching from Chrome";
    let existing = registry::visible()
        .into_iter()
        .find(|e| registry::is_local(e) && e.name.eq_ignore_ascii_case(NAME));
    let fresh = existing.is_none();
    let entry = existing.unwrap_or_else(|| registry::create_local(NAME));
    let host = open(shell, &entry.id, true).await.ok()?;
    host.wait_ready(host::READY_WITHIN).await.ok()?;
    if fresh {
        let ops: Vec<serde_json::Value> = serde_json::from_str(SWITCHING_FROM_CHROME).ok()?;
        let agent = serde_json::json!({"id": "agent:copper", "name": "Copper", "color": stable_color("Copper")});
        let out = host.apply(&ops, &agent).await.ok()?;
        if out["applied"].as_u64() != Some(ops.len() as u64) {
            tracing::warn!(target: "copper_app::canvas", "chrome guide applied partly: {out}");
        }
        let _ = host
            .zoom_to(&["sfc_header".to_string(), "sfc_f_map".to_string()])
            .await;
    }
    Some(entry.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(
            id_from_url("copper://canvas/personal").as_deref(),
            Some("personal")
        );
        assert_eq!(id_from_url("copper://canvas").as_deref(), Some("personal"));
        assert_eq!(id_from_url("copper://canvas/").as_deref(), Some("personal"));
        assert_eq!(
            id_from_url("copper://canvas/ab-12").as_deref(),
            Some("ab-12")
        );
        assert_eq!(id_from_url("copper://canvas/a%20b").as_deref(), Some("a b"));
        assert_eq!(id_from_url("copper://canvas/join/tok?cloud=x"), None);
        assert_eq!(id_from_url("https://canvas/x"), None);
        assert_eq!(url_for("a b"), "copper://canvas/a%20b");
        assert_eq!(id_from_url(&url_for("x/y")).as_deref(), Some("x/y"));
    }

    #[test]
    fn join_links() {
        let c =
            JoinLink::parse("copper://canvas/join/tok123?cloud=cloud.exowatt.com%3A8443&n=Roadmap")
                .unwrap();
        assert_eq!(c.token, "tok123");
        assert_eq!(c.cloud, "cloud.exowatt.com:8443");
        assert_eq!(c.name.as_deref(), Some("Roadmap"));
        assert!(c.copper);
        let w = JoinLink::parse("https://cloud.exowatt.com:8443/join/tok456").unwrap();
        assert_eq!(w.cloud, "cloud.exowatt.com:8443");
        assert!(!w.copper);
        assert!(JoinLink::parse("copper://canvas/personal").is_none());
        assert!(JoinLink::parse("copper://canvas/join/tok").is_none());
        assert!(JoinLink::parse("https://cloud.exowatt.com").is_none());
        assert_eq!(
            JoinLink::normalize(
                &JoinLink::parse("https://100.57.251.123/join/t")
                    .unwrap()
                    .cloud
            ),
            JoinLink::normalize("100.57.251.123:443")
        );
    }

    #[test]
    fn colours_are_stable() {
        assert_eq!(stable_color("Copper"), stable_color("copper"));
        assert!(PRESENCE.contains(&stable_color("x").as_str()));
    }
}
