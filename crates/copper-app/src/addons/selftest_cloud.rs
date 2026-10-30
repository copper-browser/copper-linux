//! The addons lane against a real Copper Cloud: `COPPER_SELFTEST=addons-cloud
//! copper` in a throwaway `COPPER_WORLD` that is already linked and signed in
//! (`qa/addons-cloud-e2e.sh` writes its `cloud.json`). Nothing is staged by
//! hand: the addon arrives through the app's own sync, over the network.
//! Screenshots go to `COPPER_SELFTEST_OUT` (default
//! `/tmp/copper-addons-cloud-e2e`).
//!
//! 1. Within 90 s the cloud sync stages `COPPER_E2E_EXPECT_ID` at
//!    `COPPER_E2E_EXPECT_VERSION`; Settings › About says "Update available —
//!    <name> <version> for the Agent sidebar" and its **Install** button is
//!    pressed.
//! 2. Ctrl+E shows the addon page; `copper.info()` reports the version and
//!    `copper.identity()` the signed-in cloud account
//!    (`COPPER_E2E_EXPECT_EMAIL`, else the account in `cloud.json`).
//! 3. `COPPER_E2E_PANE_SCRIPT`, if set: a file holding the body of an async
//!    function run in the addon page, which drives the addon's own UI and
//!    returns `{ok: boolean, steps: [...]}`. It may call `await shot(name)` for
//!    a screenshot (`cloud-pane-<name>.png`) and `await sleep(ms)`. Limit
//!    `COPPER_E2E_PANE_TIMEOUT` seconds (default 120).
//! 4. `COPPER_E2E_UPDATE_TO`, if set: the lane touches `update-wanted` in the
//!    output folder and waits for the file `COPPER_E2E_UPDATE_READY` (the
//!    harness publishes that version, then touches it). Then Settings ›
//!    Updates' addon **Check now**, "Update available" for the new version,
//!    **Install**, and the page reports the new version — the same process,
//!    no relaunch. `COPPER_E2E_UPDATE_PANE_SCRIPT` then runs like step 3.
//! 5. Settings › Extensions.
//!
//! Prints `PASS`/`FAIL` lines and exits non-zero on any failure.

use super::selftest::{READING, Run, short};
use super::{core_glue, pane};
use crate::shell::{GtkShell, Shell};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::json;
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The `COPPER_SELFTEST` value that starts this lane.
const LANE: &str = "addons-cloud";

/// How long the cloud sync may take to stage a version.
const STAGE_LIMIT: Duration = Duration::from_secs(90);

/// Start when `COPPER_SELFTEST=addons-cloud`.
pub fn maybe_start(shell: &Shell) {
    if std::env::var("COPPER_SELFTEST").ok().as_deref() != Some(LANE) {
        return;
    }
    let shell = shell.clone();
    glib::timeout_add_local_once(Duration::from_millis(800), move || {
        glib::spawn_future_local(async move {
            let failures = run(&shell).await;
            println!("addons cloud e2e: {} failure(s)", failures.len());
            for f in &failures {
                println!("  - {f}");
            }
            crate::app::set_exit(if failures.is_empty() { 0 } else { 1 });
            shell.shutdown();
            shell.app().quit();
        });
    });
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn secs(name: &str, default: u64) -> Duration {
    Duration::from_secs(env(name).and_then(|v| v.parse().ok()).unwrap_or(default))
}

/// Every descendant of `root` that is a `T`, in tree order.
fn descendants<T: IsA<gtk::Widget>>(root: &gtk::Widget) -> Vec<T> {
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if let Ok(t) = w.clone().downcast::<T>() {
            out.push(t);
        }
        let mut kids = Vec::new();
        let mut c = w.first_child();
        while let Some(k) = c {
            c = k.next_sibling();
            kids.push(k);
        }
        stack.extend(kids.into_iter().rev());
    }
    out
}

/// The settings page named `name` (its id) in `dialog`.
fn settings_page(dialog: &adw::PreferencesDialog, name: &str) -> Option<gtk::Widget> {
    descendants::<adw::PreferencesPage>(dialog.upcast_ref())
        .into_iter()
        .find(|p| PreferencesPageExt::name(p).as_deref() == Some(name))
        .map(|p| p.upcast())
}

/// The row titled `title` on settings page `page`.
fn row(dialog: &adw::PreferencesDialog, page: &str, title: &str) -> Option<adw::ActionRow> {
    let page = settings_page(dialog, page)?;
    descendants::<adw::ActionRow>(&page)
        .into_iter()
        .find(|r| r.title() == title)
}

/// The button labelled `label` in `row`.
fn row_button(row: &adw::ActionRow, label: &str) -> Option<gtk::Button> {
    descendants::<gtk::Button>(row.upcast_ref())
        .into_iter()
        .find(|b| b.label().as_deref() == Some(label))
}

/// A file name part from a name a script asked for.
fn clean(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    if s.is_empty() { "shot".into() } else { s }
}

async fn run(shell: &Shell) -> Vec<String> {
    let out = std::env::var_os("COPPER_SELFTEST_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/copper-addons-cloud-e2e"));
    let _ = std::fs::create_dir_all(&out);
    let r = Run {
        shell: shell.clone(),
        out,
        failures: RefCell::default(),
    };
    let (Some(id), Some(version)) = (
        env("COPPER_E2E_EXPECT_ID"),
        env("COPPER_E2E_EXPECT_VERSION"),
    ) else {
        r.check(
            false,
            "COPPER_E2E_EXPECT_ID and COPPER_E2E_EXPECT_VERSION are set",
        );
        return r.failures.into_inner();
    };
    crate::app::settle(&shell.downgrade(), Duration::from_secs(10)).await;

    // 0. The world is linked and signed in (cloud.json, as the harness wrote it).
    let account = crate::cloud::get().and_then(|c| c.client.account());
    r.check(
        account.is_some(),
        "the world is linked and signed in to a Copper Cloud",
    );
    let email = env("COPPER_E2E_EXPECT_EMAIL")
        .or_else(|| account.as_ref().map(|a| a.email.clone()))
        .unwrap_or_default();
    let linked = r
        .until(Duration::from_secs(15), || core_glue::status().linked)
        .await;
    r.check(linked, "the addon sync sees the link and the account");
    if account.is_none() || !linked {
        return r.failures.into_inner();
    }

    // Something to read beside the slot (and no launch card over it).
    super::view::open_tab(shell, READING);
    crate::app::settle(&shell.downgrade(), Duration::from_secs(10)).await;

    // 1. The cloud sync stages the expected version, on its own.
    let started = Instant::now();
    let staged = staged(&r, &id, &version).await;
    r.check(
        staged,
        &format!(
            "the cloud sync staged {id} {version} ({:.1} s; {:?})",
            started.elapsed().as_secs_f32(),
            core_glue::status()
        ),
    );
    if !staged {
        return r.failures.into_inner();
    }
    if !install_from_updates(&r, &id, &version, "cloud-update-available").await {
        return r.failures.into_inner();
    }

    // 2. Ctrl+E, and the page.
    if pane::showing() {
        super::hide(shell);
    }
    let _ = WidgetExt::activate_action(shell.window(), &format!("win.{}", super::ACTION), None);
    r.check(pane::showing(), "Ctrl+E shows the slot");
    r.check(
        pane::face().as_deref() == Some("addon"),
        "the slot shows the addon",
    );
    let loaded = r
        .until_js(
            Duration::from_secs(30),
            &format!(
                "return location.href.startsWith({}) && document.readyState === 'complete' && !!window.copper;",
                json!(format!("copper-addon://{id}/"))
            ),
        )
        .await;
    r.check(loaded, &format!("the page loads from copper-addon://{id}/"));
    let info = r.js("return await copper.info();").await;
    r.check(
        info.as_ref().is_ok_and(|i| {
            i["platform"] == json!("linux")
                && i["slot"] == json!(super::SLOT)
                && i["addon"]["id"] == json!(id)
                && i["addon"]["version"] == json!(version)
        }),
        &format!("info() reports {id} {version} {}", short(&info)),
    );
    let identity = r.js("return await copper.identity();").await;
    r.check(
        identity
            .as_ref()
            .is_ok_and(|i| !email.is_empty() && i["email"] == json!(email)),
        &format!("identity() is the cloud account {}", short(&identity)),
    );
    r.shot("cloud-pane").await;

    // 3. The addon's own flow.
    if let Some(script) = env("COPPER_E2E_PANE_SCRIPT") {
        pane_script(&r, &script, "cloud-pane-script").await;
    }

    // 4. A newer version, while this one runs.
    if let Some(next) = env("COPPER_E2E_UPDATE_TO") {
        update(&r, &id, &next).await;
    }

    // 5. Settings › Extensions.
    let listed = core_glue::list();
    let want = env("COPPER_E2E_UPDATE_TO").unwrap_or(version);
    r.check(
        listed
            .iter()
            .any(|l| l.id == id && l.version == want && l.installed && l.enabled),
        &format!("Settings › Extensions lists {id} {want}, installed and on"),
    );
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name(super::settings::PAGE_ID);
    r.shot("cloud-settings-addons").await;
    dialog.close();
    glib::timeout_future(Duration::from_millis(300)).await;
    r.failures.into_inner()
}

/// Wait for the sync to stage `id` at `version`.
async fn staged(r: &Run, id: &str, version: &str) -> bool {
    r.until(STAGE_LIMIT, || {
        core_glue::pending().is_some_and(|p| p.id == id && p.version == version)
    })
    .await
}

/// Settings › About: check the row, screenshot it, press its Install, and
/// wait for `id` `version` to run.
async fn install_from_updates(r: &Run, id: &str, version: &str, shot: &str) -> bool {
    let shell = &r.shell;
    let Some(p) = core_glue::pending() else {
        r.check(false, "an update is pending");
        return false;
    };
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name("about");
    crate::app::settle(&shell.downgrade(), Duration::from_secs(5)).await;
    let want = format!("{} {} for the {}", p.name, version, super::SLOT_NAME);
    let found = row(&dialog, "about", "Update available");
    let text = found
        .as_ref()
        .map(|r| format!("{} — {}", r.title(), r.subtitle().unwrap_or_default()))
        .unwrap_or_default();
    r.check(
        found
            .as_ref()
            .is_some_and(|r| r.subtitle().as_deref() == Some(want.as_str())),
        &format!("Settings › About says “{text}”"),
    );
    r.shot(shot).await;
    let Some(button) = found.as_ref().and_then(|r| row_button(r, "Install")) else {
        r.check(false, "the Update available row has an Install button");
        dialog.close();
        return false;
    };
    button.emit_clicked();
    let installed = r
        .until(Duration::from_secs(30), || {
            core_glue::active().is_some_and(|a| a.id == id && a.version == version)
        })
        .await;
    r.check(
        installed,
        &format!("Install puts {id} {version} in the slot (no relaunch)"),
    );
    r.check(
        core_glue::pending().is_none(),
        "nothing is pending after Install",
    );
    dialog.close();
    glib::timeout_future(Duration::from_millis(300)).await;
    installed
}

/// Run the script at `path` in the addon page (see the module docs) and
/// wait for its answer.
async fn pane_script(r: &Run, path: &str, shot: &str) {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) => {
            r.check(false, &format!("read the pane script {path}: {e}"));
            return;
        }
    };
    let start = format!(
        "const e = window.__copperE2E = {{done: false, result: null, want: [], taken: []}};\n\
         const sleep = ms => new Promise(ok => setTimeout(ok, ms));\n\
         const shot = name => new Promise(ok => {{\n\
           name = String(name); e.want.push(name); const t0 = Date.now();\n\
           const t = setInterval(() => {{ if (e.taken.includes(name) || Date.now() - t0 > 20000) {{ clearInterval(t); ok(); }} }}, 100);\n\
         }});\n\
         (async () => {{\n{body}\n}})().then(\n\
           v => {{ e.result = v === undefined ? null : v; e.done = true; }},\n\
           err => {{ e.result = {{ok: false, error: String((err && err.stack) || err)}}; e.done = true; }});\n\
         return true;"
    );
    if let Err(e) = r.js(&start).await {
        r.check(false, &format!("start the pane script: {e}"));
        return;
    }
    let limit = secs("COPPER_E2E_PANE_TIMEOUT", 120);
    let began = Instant::now();
    let mut oauth_shot = false;
    let result = loop {
        if began.elapsed() > limit {
            break None;
        }
        // The sign-in window, if the script opened one (it may close at once).
        if !oauth_shot && let Some(w) = super::oauth::windows().into_iter().next() {
            oauth_shot = true;
            glib::timeout_future(Duration::from_millis(400)).await;
            let path = r.out.join(format!("{shot}-oauth-window.png"));
            match crate::screenshot::save(w.upcast_ref(), &path) {
                Ok(()) => println!("SHOT {}", path.display()),
                Err(e) => println!("NOTE the sign-in window closed before a screenshot ({e})"),
            }
        }
        let state = r
            .js("const e = window.__copperE2E; if (!e) return null;\
                 return {done: e.done, want: e.want.filter(n => !e.taken.includes(n)), result: e.done ? e.result : null};")
            .await;
        let Ok(state) = state else {
            glib::timeout_future(Duration::from_millis(300)).await;
            continue;
        };
        if state.is_null() {
            r.check(false, "the pane script is running (the page reloaded?)");
            return;
        }
        for name in state["want"].as_array().into_iter().flatten() {
            let name = name.as_str().unwrap_or("shot").to_string();
            r.shot(&format!("cloud-pane-{}", clean(&name))).await;
            let _ = r
                .js(&format!(
                    "window.__copperE2E.taken.push({}); return true;",
                    json!(name)
                ))
                .await;
        }
        if state["done"] == json!(true) {
            break Some(state["result"].clone());
        }
        glib::timeout_future(Duration::from_millis(250)).await;
    };
    let Some(result) = result else {
        r.check(
            false,
            &format!("the pane script finished within {} s", limit.as_secs()),
        );
        r.shot(shot).await;
        return;
    };
    for step in result["steps"].as_array().into_iter().flatten() {
        match step.as_str() {
            Some(s) => println!("  step: {s}"),
            None => println!("  step: {step}"),
        }
    }
    let ok = result["ok"] == json!(true);
    let why = result
        .get("error")
        .map(|e| format!(" — {e}"))
        .unwrap_or_default();
    r.check(
        ok,
        &format!(
            "the pane script ({}) passed in {:.1} s{why}",
            PathBuf::from(path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default(),
            began.elapsed().as_secs_f32()
        ),
    );
    r.shot(shot).await;
}

/// Step 4: a newer version arrives while the old one runs.
async fn update(r: &Run, id: &str, next: &str) {
    let shell = &r.shell;
    let Some(ready) = env("COPPER_E2E_UPDATE_READY").map(PathBuf::from) else {
        r.check(
            false,
            "COPPER_E2E_UPDATE_READY is set with COPPER_E2E_UPDATE_TO",
        );
        return;
    };
    let _ = std::fs::write(r.out.join("update-wanted"), next);
    let limit = secs("COPPER_E2E_UPDATE_WAIT", 300);
    let came = r.until(limit, || ready.exists()).await;
    r.check(came, &format!("the harness published {id} {next}"));
    if !came {
        return;
    }

    // Settings › About › Check for addon updates › Check now. The cloud's
    // event stream may have asked already: let any such check finish and
    // land in an earlier second, so the press is seen to run one of its own.
    let _ = r
        .until(Duration::from_secs(30), || !core_glue::status().checking)
        .await;
    glib::timeout_future(Duration::from_millis(1100)).await;
    let early = core_glue::pending().is_some_and(|p| p.version == next);
    let before = core_glue::checked_at();
    let dialog = crate::window::settings::dialog(shell);
    dialog.set_visible_page_name("about");
    crate::app::settle(&shell.downgrade(), Duration::from_secs(5)).await;
    let button = row(&dialog, "about", "Check for addon updates")
        .and_then(|r| row_button(&r, "Check now"))
        .filter(|b| b.is_sensitive());
    if let Some(b) = &button {
        b.emit_clicked();
    }
    let checked = button.is_some()
        && r.until(Duration::from_secs(30), || {
            core_glue::checked_at() > before && !core_glue::status().checking
        })
        .await;
    r.check(
        checked,
        &format!(
            "Settings › About › Check now runs a cloud check{}",
            if early {
                " (the cloud's event stream had already staged the version)"
            } else {
                ""
            }
        ),
    );
    dialog.close();
    glib::timeout_future(Duration::from_millis(300)).await;
    let started = Instant::now();
    let staged = staged(r, id, next).await;
    r.check(
        staged,
        &format!(
            "the cloud sync staged {id} {next} ({:.1} s)",
            started.elapsed().as_secs_f32()
        ),
    );
    if !staged {
        return;
    }
    let before = pane::view().map(|v| std::rc::Rc::as_ptr(&v) as usize);
    if !install_from_updates(r, id, next, "cloud-update2-available").await {
        return;
    }
    if !pane::showing() {
        super::show(shell);
    }
    let after = pane::view().map(|v| std::rc::Rc::as_ptr(&v) as usize);
    r.check(
        before.is_some() && before == after,
        "the same addon view reloads in place",
    );
    let updated = r
        .until_js(
            Duration::from_secs(30),
            &format!(
                "if (!window.copper || document.readyState !== 'complete') return false;\
                 const i = await copper.info(); return i.addon.version === {};",
                json!(next)
            ),
        )
        .await;
    let info = r.js("return await copper.info();").await;
    r.check(
        updated,
        &format!(
            "the page reports {id} {next} without a relaunch {}",
            short(&info)
        ),
    );
    r.shot("cloud-pane-updated").await;
    if let Some(script) = env("COPPER_E2E_UPDATE_PANE_SCRIPT") {
        pane_script(r, &script, "cloud-pane-updated-script").await;
    }
}
