//! The cloud-ui lane's own self-test: `COPPER_SELFTEST=cloud-ui copper`
//! (in a throwaway `COPPER_WORLD`; screenshots go to `COPPER_SELFTEST_OUT`,
//! default `/tmp/copper-cloud-ui-selftest`). Needs no server:
//!
//! * Settings › Cloud, unlinked: a link code is read (host and fingerprint
//!   shown before anything is sent), a pairing code is told apart, a bare
//!   `cp_…` and junk are refused in words — light and dark;
//! * the Personal canvas opens in a tab at `copper://canvas/personal`, takes
//!   `canvas_apply`, reads back with `canvas_read`, and its history is on
//!   disk; `canvas_list`, `canvas_select`/`canvas_focus`, `canvas_screenshot`
//!   and a local `canvas_create` answer — light and dark;
//! * `cloud_history_delete` refuses while unlinked.
//!
//! Prints `PASS`/`FAIL` lines and exits non-zero on any failure.

use crate::shell::Shell;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

/// Start when `COPPER_SELFTEST=cloud-ui`.
pub fn maybe_start(shell: &Shell) {
    if std::env::var("COPPER_SELFTEST").ok().as_deref() != Some("cloud-ui") {
        return;
    }
    let shell = shell.clone();
    glib::timeout_add_local_once(Duration::from_millis(800), move || {
        glib::spawn_future_local(async move {
            let failures = run(&shell).await;
            println!("cloud-ui selftest: {} failure(s)", failures.len());
            for f in &failures {
                println!("  - {f}");
            }
            crate::app::set_exit(if failures.is_empty() { 0 } else { 1 });
            shell.shutdown();
            shell.app().quit();
        });
    });
}

struct Run {
    shell: Shell,
    out: PathBuf,
    failures: RefCell<Vec<String>>,
}

impl Run {
    fn check(&self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.borrow_mut().push(what.into());
        }
    }

    async fn shot(&self, name: &str) {
        crate::app::settle(&self.shell.downgrade(), Duration::from_secs(5)).await;
        glib::timeout_future(Duration::from_millis(400)).await;
        let path = self.out.join(format!("{name}.png"));
        match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
            Ok(()) => println!("SHOT {}", path.display()),
            Err(e) => self.check(false, &format!("screenshot {name}: {e}")),
        }
    }

    async fn canvas(&self, tool: &str, args: Value) -> Result<String, String> {
        let args: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
        let items = crate::canvas::mcp::call(self.shell.clone(), tool.into(), args).await?;
        Ok(items
            .iter()
            .map(|i| match i {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

async fn run(shell: &Shell) -> Vec<String> {
    let out = std::env::var("COPPER_SELFTEST_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/copper-cloud-ui-selftest"));
    let _ = std::fs::create_dir_all(&out);
    let r = Rc::new(Run {
        shell: shell.clone(),
        out,
        failures: RefCell::default(),
    });
    let fp = "577920b797ef884344c9be3576103c6b1c0ca7776d72dff1f17a905435ffa1c4";
    let link = format!("copper-cloud://cloud.example.test:8443/#k=selftest-key&fp={fp}");
    let pairing = format!(
        "copper-cloud://cloud.example.test:8443/#p=cp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345&fp={fp}"
    );

    for look in [
        copper_core::settings::Look::Light,
        copper_core::settings::Look::Dark,
    ] {
        let dark = look == copper_core::settings::Look::Dark;
        let tag = if dark { "dark" } else { "light" };
        shell.set_look(look);
        super::open_settings(shell);
        glib::timeout_future(Duration::from_millis(500)).await;
        let Some(page) = super::page::current() else {
            r.check(false, "Settings › Cloud opens");
            break;
        };
        r.check(
            page.phase() == super::page::Phase::Unlinked,
            &format!("{tag}: unlinked page"),
        );
        r.shot(&format!("cloud-unlinked-{tag}")).await;
        page.type_code(&link);
        glib::timeout_future(Duration::from_millis(200)).await;
        let said = page.said_text().unwrap_or_default();
        r.check(
            said.contains("Link code")
                && said.contains("cloud.example.test:8443")
                && said.contains("57:79:20"),
            &format!("{tag}: link code read — host and fingerprint shown before sending"),
        );
        r.shot(&format!("cloud-link-code-{tag}")).await;
        page.type_code(&pairing);
        glib::timeout_future(Duration::from_millis(200)).await;
        let said = page.said_text().unwrap_or_default();
        r.check(
            said.contains("Pairing code"),
            &format!("{tag}: pairing code told apart"),
        );
        r.shot(&format!("cloud-pairing-code-{tag}")).await;
        page.type_code("cp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345");
        glib::timeout_future(Duration::from_millis(200)).await;
        r.check(
            page.error_text()
                .is_some_and(|e| e.contains("without its address")),
            &format!("{tag}: a bare pairing code is refused in words"),
        );
        page.type_code("hello");
        glib::timeout_future(Duration::from_millis(200)).await;
        r.check(
            page.error_text().is_some(),
            &format!("{tag}: junk is refused"),
        );
        r.shot(&format!("cloud-refused-{tag}")).await;
        if let Some(d) = shell.window().visible_dialog() {
            d.close();
        }
        glib::timeout_future(Duration::from_millis(300)).await;
    }

    // cloud_history_delete refuses while unlinked.
    let refused = super::mcp::history_delete(shell.clone(), Map::new()).await;
    r.check(
        refused.as_ref().is_err_and(|e| e.contains("isn't linked")),
        "cloud_history_delete refuses while unlinked",
    );

    // The Personal canvas, local.
    shell.set_look(copper_core::settings::Look::Light);
    match r
        .canvas("canvas_open", json!({"id": "personal", "foreground": true}))
        .await
    {
        Ok(t) => r.check(t.contains("\"kind\":\"personal\""), "canvas_open personal"),
        Err(e) => r.check(false, &format!("canvas_open personal: {e}")),
    }
    let url = shell
        .browser()
        .active()
        .map(|t| t.url.clone())
        .unwrap_or_default();
    r.check(
        url == "copper://canvas/personal",
        "the canvas tab's address is copper://canvas/personal",
    );
    let ops = json!([
        {"op": "add", "shape": {"type": "frame", "id": "f1", "x": -40, "y": -60, "w": 760, "h": 360, "title": "Linux self-test"}},
        {"op": "add", "shape": {"type": "sticky", "id": "s1", "x": 0, "y": 0, "text": "Personal stays on this device", "color": "yellow"}},
        {"op": "add", "shape": {"type": "sticky", "id": "s2", "x": 260, "y": 0, "text": "copper://canvas served by WebKitGTK", "color": "green"}},
        {"op": "add", "shape": {"type": "text", "id": "t1", "x": 520, "y": 40, "text": "canvas_apply → canvas_read"}},
        {"op": "connect", "from": "s1", "to": "s2", "label": "same board"}
    ]);
    match r
        .canvas(
            "canvas_apply",
            json!({"id": "personal", "ops": ops, "as": "self-test"}),
        )
        .await
    {
        Ok(t) => r.check(
            t.contains("\"applied\":5"),
            &format!("canvas_apply 5 ops ({t})"),
        ),
        Err(e) => r.check(false, &format!("canvas_apply: {e}")),
    }
    match r.canvas("canvas_read", json!({"id": "personal"})).await {
        Ok(t) => r.check(
            t.contains("Personal stays on this device") && t.contains("\"by\":\"self-test\""),
            "canvas_read reads it back, attributed",
        ),
        Err(e) => r.check(false, &format!("canvas_read: {e}")),
    }
    let _ = r
        .canvas(
            "canvas_focus",
            json!({"id": "personal", "shapeIds": ["f1"]}),
        )
        .await;
    match r
        .canvas(
            "canvas_select",
            json!({"id": "personal", "shapeIds": ["s2"]}),
        )
        .await
    {
        Ok(t) => r.check(t.starts_with("Selected 1 shape"), "canvas_select"),
        Err(e) => r.check(false, &format!("canvas_select: {e}")),
    }
    glib::timeout_future(Duration::from_millis(800)).await;
    r.shot("canvas-personal-light").await;
    shell.set_look(copper_core::settings::Look::Dark);
    glib::timeout_future(Duration::from_millis(800)).await;
    r.shot("canvas-personal-dark").await;
    match r
        .canvas("canvas_screenshot", json!({"id": "personal"}))
        .await
    {
        Ok(t) => r.check(t.contains("\"type\":\"image\""), "canvas_screenshot"),
        Err(e) => r.check(false, &format!("canvas_screenshot: {e}")),
    }
    match r
        .canvas("canvas_create", json!({"name": "Local board"}))
        .await
    {
        Ok(t) => r.check(
            t.contains("\"kind\":\"local\""),
            "canvas_create signed out makes a local canvas",
        ),
        Err(e) => r.check(false, &format!("canvas_create: {e}")),
    }
    match r.canvas("canvas_list", json!({})).await {
        Ok(t) => r.check(
            t.contains("Local board") && t.contains("not connected"),
            "canvas_list",
        ),
        Err(e) => r.check(false, &format!("canvas_list: {e}")),
    }
    match r
        .canvas(
            "canvas_invite",
            json!({"id": "personal", "email": "a@b.test"}),
        )
        .await
    {
        Err(e) => r.check(
            e.contains("Personal canvas can't be shared"),
            "canvas_invite refuses Personal",
        ),
        Ok(t) => r.check(false, &format!("canvas_invite should refuse: {t}")),
    }
    // History on disk.
    let log = shell
        .store()
        .paths()
        .data
        .join("canvas/personal/updates.log");
    let snap = shell
        .store()
        .paths()
        .data
        .join("canvas/personal/snapshot.bin");
    r.check(
        log.exists() || snap.exists(),
        "Personal's history is on disk (canvas/personal/)",
    );
    shell.set_look(copper_core::settings::Look::Light);
    r.failures.borrow().clone()
}
