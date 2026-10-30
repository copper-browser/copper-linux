//! The addons lane's self-test: `COPPER_SELFTEST=addons copper` in a
//! throwaway `COPPER_WORLD` (screenshots go to `COPPER_SELFTEST_OUT`,
//! default `/tmp/copper-addons-selftest`). Needs no cloud. Mirrors the
//! Mac's `Fork/Addons/AddonsBench.swift` (`./bench addons …`).
//!
//! 1. Nothing installed: Ctrl+E shows Copper's placeholder — light and dark.
//! 2. The hello fixture (`qa/addons/hello`, or `COPPER_ADDONS_DEV_DIR`) is
//!    packed in the served format, verified and staged: Settings › About
//!    says "Update available"; Install puts it in the slot without a
//!    relaunch.
//! 3. Through the page's own `window.copper`: `info`, `settings`,
//!    `storage.*`, `fetch` to a host the manifest doesn't list (`denied`),
//!    `openUrl` (a new foreground tab), `page.current`, `agent.state`
//!    (`available: false`), `ui.close`; the `visibility` event arrives.
//! 4. Screenshots with the pane open and closed — light and dark — and
//!    Settings › Extensions; Remove puts the placeholder back.
//!
//! Prints `PASS`/`FAIL` lines and exits non-zero on any failure.

use super::{core_glue, pane, policy};
use crate::shell::{GtkShell, Shell};
use copper_core::settings::Look;
use copper_engine::{PageView, World};
use gtk4::prelude::*;
use libadwaita::prelude::*;
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Start when `COPPER_SELFTEST=addons`.
pub fn maybe_start(shell: &Shell) {
    if std::env::var("COPPER_SELFTEST").ok().as_deref() != Some("addons") {
        return;
    }
    let shell = shell.clone();
    glib::timeout_add_local_once(Duration::from_millis(800), move || {
        glib::spawn_future_local(async move {
            let failures = run(&shell).await;
            println!("addons selftest: {} failure(s)", failures.len());
            for f in &failures {
                println!("  - {f}");
            }
            crate::app::set_exit(if failures.is_empty() { 0 } else { 1 });
            shell.shutdown();
            shell.app().quit();
        });
    });
}

/// A page for the stage while the slot is open (no network needed).
pub(super) const READING: &str = "data:text/html;charset=utf-8,%3C!doctype%20html%3E%3Ctitle%3EField%20notes%3C/title%3E\
%3Cmeta%20name=color-scheme%20content=%22light%20dark%22%3E\
%3Cbody%20style=%22font:17px/1.6%20Georgia,serif;max-width:34em;margin:9vh%20auto;padding:0%202em%22%3E\
%3Ch1%20style=%22font-weight:500%22%3EField%20notes%3C/h1%3E\
%3Cp%3EThe%20Agent%20sidebar%20sits%20to%20the%20right%20of%20the%20page,%20full%20height.%20\
Ctrl+E%20shows%20it%20or%20puts%20it%20away;%20its%20leading%20edge%20can%20be%20dragged%20wider.%3C/p%3E\
%3Cp%3EWhat%20it%20shows%20comes%20from%20your%20Copper%20Cloud:%20a%20small%20web%20app%20with%20its%20own%20\
storage%20and%20settings,%20updated%20without%20a%20new%20Copper.%3C/p%3E";

/// The fixture folder.
fn fixture() -> PathBuf {
    std::env::var_os("COPPER_ADDONS_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("qa/addons/hello"))
}

/// One self-test run: its PASS/FAIL lines, screenshots and page calls
/// (shared with the cloud lane, `selftest_cloud.rs`).
pub(super) struct Run {
    pub(super) shell: Shell,
    pub(super) out: PathBuf,
    pub(super) failures: RefCell<Vec<String>>,
}

impl Run {
    pub(super) fn check(&self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.borrow_mut().push(what.into());
        }
    }

    pub(super) async fn shot(&self, name: &str) {
        crate::app::settle(&self.shell.downgrade(), Duration::from_secs(5)).await;
        glib::timeout_future(Duration::from_millis(500)).await;
        let path = self.out.join(format!("{name}.png"));
        // A frame can be mid-relayout (just after a theme switch): try again.
        let mut last = None;
        for _ in 0..5 {
            match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
                Ok(()) => {
                    println!("SHOT {}", path.display());
                    return;
                }
                Err(e) => last = Some(e),
            }
            glib::timeout_future(Duration::from_millis(300)).await;
        }
        if let Some(e) = last {
            self.check(false, &format!("screenshot {name}: {e}"));
        }
    }

    pub(super) async fn look(&self, look: Look) {
        self.shell.set_look(look);
        glib::timeout_future(Duration::from_millis(400)).await;
    }

    /// `body` as an async function in the addon's page world.
    pub(super) async fn js(&self, body: &str) -> Result<Value, String> {
        let Some(v) = pane::view() else {
            return Err("no addon view".into());
        };
        let fut = v.page().call_async(body, &Map::new(), &World::Page);
        match glib::future_with_timeout(Duration::from_secs(20), fut).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("timed out".into()),
        }
    }

    /// Wait for `cond` (polled), at most `limit`.
    pub(super) async fn until(&self, limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if cond() {
                return true;
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
        cond()
    }

    pub(super) async fn until_js(&self, limit: Duration, body: &str) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if self.js(body).await.is_ok_and(|v| v == Value::Bool(true)) {
                return true;
            }
            glib::timeout_future(Duration::from_millis(200)).await;
        }
        false
    }
}

async fn run(shell: &Shell) -> Vec<String> {
    let out = std::env::var_os("COPPER_SELFTEST_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/copper-addons-selftest"));
    let _ = std::fs::create_dir_all(&out);
    let r = Run {
        shell: shell.clone(),
        out,
        failures: RefCell::default(),
    };
    crate::app::settle(&shell.downgrade(), Duration::from_secs(10)).await;

    // The rules that need no window.
    r.check(
        policy::clamp_width(Some(100.0)) == 340 && policy::clamp_width(Some(9000.0)) == 620,
        "pane width is clamped to 340…620",
    );
    r.check(
        pane::width() == policy::clamp_width(shell.settings().number(pane::WIDTH_KEY)),
        &format!(
            "pane width comes from {} ({})",
            pane::WIDTH_KEY,
            pane::width()
        ),
    );

    // Something to read beside the slot (and no launch card over it).
    super::view::open_tab(shell, READING);
    crate::app::settle(&shell.downgrade(), Duration::from_secs(10)).await;

    // 1. The placeholder.
    let dev = core_glue::dev_dir().is_some();
    if !dev {
        r.check(
            core_glue::active().is_none(),
            "a fresh world has no addon in the slot",
        );
    }
    r.check(!pane::showing(), "the slot starts hidden");
    if !dev {
        r.look(Look::Light).await;
        super::toggle(shell);
        r.check(pane::showing(), "Ctrl+E shows the slot");
        r.check(
            pane::face().as_deref() == Some("placeholder"),
            "with no addon the slot shows Copper’s placeholder",
        );
        r.shot("linux-placeholder-light").await;
        r.look(Look::Dark).await;
        r.shot("linux-placeholder-dark").await;
        r.look(Look::Light).await;
        super::toggle(shell);
        r.check(!pane::showing(), "Ctrl+E again hides the slot");
    }

    // 2. Stage the fixture as a cloud update, then Install.
    if !dev {
        match core_glue::selftest_stage(&fixture()) {
            Ok(()) => r.check(true, "the hello bundle is verified and staged"),
            Err(e) => r.check(false, &format!("stage the hello bundle: {e}")),
        }
        super::changed(shell);
        let pending = core_glue::pending();
        r.check(
            pending
                .as_ref()
                .is_some_and(|p| p.id == "example.hello" && p.version == "1.0.0"),
            &format!("pending update is listed ({pending:?})"),
        );
        let dialog = crate::window::settings::dialog(shell);
        dialog.set_visible_page_name("about");
        r.shot("linux-update-available").await;
        dialog.set_visible_page_name(super::settings::PAGE_ID);
        r.shot("linux-settings-addons-pending").await;
        dialog.close();
        glib::timeout_future(Duration::from_millis(300)).await;
        super::updates::install(shell);
        let installed = r
            .until(Duration::from_secs(10), || {
                core_glue::active().is_some_and(|a| a.id == "example.hello")
            })
            .await;
        r.check(
            installed,
            "Install puts the addon in the slot (no relaunch)",
        );
        r.check(
            core_glue::pending().is_none(),
            "nothing is pending after Install",
        );
    }

    // 3. The bridge, from the page.
    super::show(shell);
    r.check(
        pane::face().as_deref() == Some("addon"),
        "the slot shows the addon",
    );
    let ready = r
        .until_js(
            Duration::from_secs(30),
            "return !!window.copper && window.__helloDone === true;",
        )
        .await;
    r.check(ready, "the addon page loaded and ran every call");
    if !ready {
        let why = r
            .js(
                "return {head: document.head.outerHTML.slice(0, 600), \
                 sheets: document.styleSheets.length, scripts: document.scripts.length, \
                 entries: performance.getEntries().map(e => e.name + ' ' + (e.responseStatus || '')), \
                 copper: typeof window.copper, done: window.__helloDone, results: window.__helloResults};",
            )
            .await;
        println!("  page: {why:?}");
    }
    let url = pane::view()
        .and_then(|v| v.page().url())
        .unwrap_or_default();
    r.check(
        url.starts_with("copper-addon://example.hello/"),
        &format!("the page is served from copper-addon:// ({url})"),
    );

    let info = r.js("return await copper.info();").await;
    r.check(
        info.as_ref().is_ok_and(|i| {
            i["bridge"] == json!(1)
                && i["platform"] == json!("linux")
                && i["slot"] == json!("agent-sidebar")
                && i["addon"]["id"] == json!("example.hello")
                && !i["capabilities"]
                    .as_array()
                    .is_some_and(|c| c.contains(&json!("agent")))
        }),
        &format!("info() {}", short(&info)),
    );
    let settings = r.js("return await copper.settings();").await;
    r.check(
        settings
            .as_ref()
            .is_ok_and(|s| s["greeting"] == json!("Hello from the addon’s settings")),
        &format!("settings() {}", short(&settings)),
    );
    let storage = r
        .js("await copper.storage.set('selftest', {n: 42, s: 'ü'});\
             const got = await copper.storage.get('selftest');\
             const keys = await copper.storage.keys();\
             await copper.storage.remove('selftest');\
             const gone = await copper.storage.get('selftest');\
             return {got, keys, gone};")
        .await;
    r.check(
        storage.as_ref().is_ok_and(|s| {
            s["got"] == json!({"n": 42, "s": "ü"})
                && s["keys"]
                    .as_array()
                    .is_some_and(|k| k.contains(&json!("selftest")))
                && s["gone"].is_null()
        }),
        &format!("storage set/get/keys/remove {}", short(&storage)),
    );
    let denied = r
        .js(
            "try { await copper.fetch('https://denied.example.org/'); return 'resolved'; }\
             catch (e) { return e.code; }",
        )
        .await;
    r.check(
        denied.as_ref().is_ok_and(|d| d == "denied"),
        &format!(
            "fetch to a host the manifest doesn't list is denied ({})",
            short(&denied)
        ),
    );
    let before = shell.browser().all_tabs().count();
    let opened = r
        .js("return await copper.openUrl('https://example.com/?from=hello-addon');")
        .await;
    glib::timeout_future(Duration::from_millis(500)).await;
    let active_url = shell
        .active_id()
        .and_then(|id| shell.tab_view(&id))
        .map(|t| t.url)
        .unwrap_or_default();
    r.check(
        opened.is_ok()
            && active_url.starts_with("https://example.com/")
            && shell.browser().all_tabs().count() == before + 1,
        &format!("openUrl() opens a new foreground tab ({active_url})"),
    );
    let page = r.js("return await copper.page.current();").await;
    r.check(
        page.as_ref().is_ok_and(|p| {
            p["url"]
                .as_str()
                .is_some_and(|u| u.starts_with("https://example.com/"))
        }),
        &format!("page.current() {}", short(&page)),
    );
    let page_event = r
        .until_js(
            Duration::from_secs(10),
            "return window.__helloEvents.some(e => e.name === 'page' && e.payload \
             && String(e.payload.url).startsWith('https://example.com/'));",
        )
        .await;
    r.check(page_event, "the page event follows the active tab");
    let agent = r.js("return await copper.agent.state();").await;
    r.check(
        agent.as_ref().is_ok_and(|a| a["available"] == json!(false)),
        &format!("agent.state() says unavailable {}", short(&agent)),
    );
    let identity = r.js("return await copper.identity();").await;
    r.check(
        identity.as_ref().is_ok_and(Value::is_null),
        &format!("identity() is null without a cloud {}", short(&identity)),
    );

    // Only the slot's own files, and only from this addon.
    let files = r
        .js(
            "const load = src => new Promise(ok => { const i = new Image(); \
               i.onload = () => ok('loaded'); i.onerror = () => ok('refused'); i.src = src; });\
             return {missing: await load('nope.png'), other: await load('copper-addon://example.other/icon.png'), \
                     up: await load('copper-addon://example.hello/..%2Fmanifest.json')};",
        )
        .await;
    r.check(
        files.as_ref().is_ok_and(|f| {
            f["missing"] == json!("refused")
                && f["other"] == json!("refused")
                && f["up"] == json!("refused")
        }),
        &format!(
            "copper-addon:// answers only the slot's own files {}",
            short(&files)
        ),
    );
    // The view never leaves the addon.
    let _ = r
        .js("setTimeout(() => { location.href = 'https://example.com/elsewhere'; }, 0); return true;")
        .await;
    glib::timeout_future(Duration::from_millis(800)).await;
    let url = pane::view()
        .and_then(|v| v.page().url())
        .unwrap_or_default();
    r.check(
        url.starts_with("copper-addon://example.hello/"),
        &format!("a navigation away from the addon is refused ({url})"),
    );

    // OAuth: the first address with the redirect prefix ends the sign-in.
    let oauth = r
        .js(
            "return await copper.oauth.authorize({url: 'https://example.com/authorize?code=abc&state=s', \
             redirectPrefix: 'https://example.com/authorize', timeoutMs: 20000});",
        )
        .await;
    r.check(
        oauth
            .as_ref()
            .is_ok_and(|o| o["url"] == json!("https://example.com/authorize?code=abc&state=s")),
        &format!(
            "oauth.authorize() resolves with the redirect {}",
            short(&oauth)
        ),
    );
    r.check(
        super::oauth::windows().is_empty(),
        "the sign-in window closes itself",
    );
    // Closing the window is `cancelled`.
    let _ = r
        .js(
            "window.__oauth = 'pending'; copper.oauth.authorize({url: 'https://example.com/?signin', \
             redirectPrefix: 'http://127.0.0.1:8765/callback', timeoutMs: 60000}) \
             .then(() => { window.__oauth = 'resolved'; }, e => { window.__oauth = e.code; }); return true;",
        )
        .await;
    let opened = r
        .until(Duration::from_secs(5), || {
            !super::oauth::windows().is_empty()
        })
        .await;
    r.check(
        opened,
        "oauth.authorize() opens a “Sign in — Hello addon” window",
    );
    if let Some(w) = super::oauth::windows().into_iter().next() {
        crate::app::settle(&shell.downgrade(), Duration::from_secs(10)).await;
        glib::timeout_future(Duration::from_millis(800)).await;
        let path = r.out.join("linux-oauth-window.png");
        match crate::screenshot::save(w.upcast_ref(), &path) {
            Ok(()) => println!("SHOT {}", path.display()),
            Err(e) => r.check(false, &format!("screenshot linux-oauth-window: {e}")),
        }
        w.close();
    }
    let cancelled = r
        .until_js(
            Duration::from_secs(5),
            "return window.__oauth === 'cancelled';",
        )
        .await;
    r.check(cancelled, "closing the sign-in window is `cancelled`");

    // Back to the addon (openUrl took the focus to the new tab).
    r.shot("linux-open-light").await;
    r.look(Look::Dark).await;
    let themed = r
        .until_js(
            Duration::from_secs(5),
            "return window.__helloEvents.some(e => e.name === 'theme' && e.payload && e.payload.dark === true);",
        )
        .await;
    r.check(themed, "the theme event follows the window to dark");
    r.shot("linux-open-dark").await;

    // ui.close and the visibility event.
    let closed = r.js("await copper.ui.close(); return true;").await;
    let hidden = r.until(Duration::from_secs(3), || !pane::showing()).await;
    r.check(closed.is_ok() && hidden, "ui.close() hides the slot");
    r.shot("linux-closed-dark").await;
    r.look(Look::Light).await;
    r.shot("linux-closed-light").await;
    super::show(shell);
    let vis = r
        .until_js(
            Duration::from_secs(5),
            "const v = window.__helloEvents.filter(e => e.name === 'visibility');\
             return v.some(e => e.payload.visible === false) && v.some(e => e.payload.visible === true);",
        )
        .await;
    r.check(vis, "visibility events arrive (hidden, then shown again)");
    let kept = r
        .js("return window.__helloDone === true && document.querySelectorAll('#results li').length > 5;")
        .await;
    r.check(
        kept.as_ref().is_ok_and(|v| v == &json!(true)),
        "the page was kept while hidden (no reload)",
    );

    // Settings › Extensions with the addon installed.
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name(super::settings::PAGE_ID);
    r.shot("linux-settings-addons").await;
    dialog.close();
    glib::timeout_future(Duration::from_millis(300)).await;

    // “Use in Agent sidebar” off and on again: the placeholder, then the addon.
    if !dev {
        core_glue::set_enabled(shell, "example.hello", false);
        let off = r
            .until(Duration::from_secs(5), || {
                core_glue::active().is_none() && pane::face().as_deref() == Some("placeholder")
            })
            .await;
        r.check(off, "turning the addon off here shows the placeholder");
        core_glue::set_enabled(shell, "example.hello", true);
        let on = r
            .until(Duration::from_secs(5), || {
                core_glue::active().is_some() && pane::face().as_deref() == Some("addon")
            })
            .await;
        let ran = on
            && r.until_js(
                Duration::from_secs(20),
                "return window.__helloDone === true;",
            )
            .await;
        r.check(ran, "turning it on again brings the addon back");
    }

    // Remove puts the placeholder back at once.
    if !dev {
        core_glue::remove(shell, "example.hello");
        let back = r
            .until(Duration::from_secs(5), || {
                core_glue::active().is_none() && pane::face().as_deref() == Some("placeholder")
            })
            .await;
        r.check(back, "Remove puts the placeholder back");
    }
    r.failures.into_inner()
}

pub(super) fn short(v: &Result<Value, String>) -> String {
    let s = match v {
        Ok(v) => v.to_string(),
        Err(e) => format!("error: {e}"),
    };
    if s.chars().count() > 200 {
        format!("{}…", s.chars().take(200).collect::<String>())
    } else {
        s
    }
}
