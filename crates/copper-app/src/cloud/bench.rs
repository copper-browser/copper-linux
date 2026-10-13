//! The cloud lane's test bench (Mac: `CloudBench.swift`, `bench cloud …`):
//! with `COPPER_CLOUD_BENCH=<socket path>` the app listens on a Unix socket
//! for one JSON object per line and answers each with one JSON line. It
//! drives the real Settings › Cloud page (typing into its fields, pressing
//! its buttons), makes local edits through the shell, waits for synced state
//! to show up, takes screenshots, and calls the canvas tools.
//!
//! For QA only: nothing listens unless the variable is set, and the socket
//! is created 0600.
//!
//! ```text
//! {"op":"connect","code":"copper-cloud://…"}   {"op":"signup","email":…,"password":…,"name":…}
//! {"op":"turn-on","domains":["spaces",…]}      {"op":"make-code"}
//! {"op":"space-add","name":"Work"}             {"op":"open","url":…,"saved":true,"pin":"W","space":"Work"}
//! {"op":"bookmark-add","url":…,"title":…}      {"op":"visit","url":…,"title":…}
//! {"op":"wait","what":"space","value":"Work","timeout":60}
//! {"op":"screenshot","path":"/tmp/a.png"}      {"op":"canvas","tool":"canvas_list","args":{}}
//! ```

use super::page::{Phase, current};
use super::{get, shell};
use crate::shell::{OpenTab, Shell};
use copper_cloud::Domain;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::time::{Duration, Instant};

/// Listen when `COPPER_CLOUD_BENCH` names a socket path.
pub fn start_if_asked(_shell: &Shell) {
    let Ok(path) = std::env::var("COPPER_CLOUD_BENCH") else {
        return;
    };
    if path.trim().is_empty() {
        return;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, %path, "cloud bench socket");
            return;
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    tracing::info!(%path, "cloud bench listening");
    std::thread::Builder::new()
        .name("cloud-bench".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || serve(stream));
            }
        })
        .ok();
}

fn serve(stream: UnixStream) {
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut out = stream;
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(req) => ask(req),
            Err(e) => json!({"error": format!("not JSON: {e}")}),
        };
        let mut text = answer.to_string();
        text.push('\n');
        if out.write_all(text.as_bytes()).is_err() {
            return;
        }
    }
}

/// Run one request on the main loop and wait for its answer.
fn ask(req: Value) -> Value {
    let (tx, rx) = std::sync::mpsc::channel();
    glib::MainContext::default().invoke(move || {
        glib::spawn_future_local(async move {
            let out = match run(req).await {
                Ok(v) => v,
                Err(e) => json!({"error": e}),
            };
            let _ = tx.send(out);
        });
    });
    rx.recv_timeout(Duration::from_secs(600))
        .unwrap_or_else(|_| json!({"error": "no answer in 600 s"}))
}

async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

fn s<'a>(req: &'a Value, key: &str) -> &'a str {
    req.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The Cloud page, opening Settings on it if it isn't up.
async fn page(shell: &Shell) -> Result<std::rc::Rc<super::page::CloudPage>, String> {
    if let Some(p) = current()
        && p_visible(&p)
    {
        return Ok(p);
    }
    super::open_settings(shell);
    sleep(400).await;
    current().ok_or_else(|| "the Cloud page didn't open".into())
}

fn p_visible(p: &super::page::CloudPage) -> bool {
    let _ = p;
    current_dialog().is_some()
}

fn current_dialog() -> Option<adw::Dialog> {
    let shell = shell()?;
    let dialog = shell.window().visible_dialog()?;
    Some(dialog)
}

/// Wait until `test` holds (polled every 200 ms), at most `secs`.
async fn until(secs: f64, mut test: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs_f64(secs);
    loop {
        if test() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(200).await;
    }
}

fn status() -> Value {
    let Some(c) = get() else {
        return json!({"cloud": null});
    };
    let st = c.client.status();
    json!({
        "linked": st.linked,
        "host": st.host,
        "fingerprint": st.fingerprint,
        "signedIn": st.signed_in,
        "account": st.account.as_ref().map(|a| json!({"email": a.email, "name": a.display_name, "device": a.device_id})),
        "reachable": st.reachable,
        "version": st.server_version,
        "device": st.device_name,
        "sync": c.sync.status().json(),
        "phase": format!("{:?}", current().map(|p| p.phase())),
    })
}

fn space_id(shell: &Shell, name: &str) -> Option<String> {
    shell
        .browser()
        .spaces()
        .find(|s| s.name == name || s.id.eq_ignore_ascii_case(name))
        .map(|s| s.id.clone())
}

fn page_error() -> Option<String> {
    current().and_then(|p| p.error_text())
}

async fn run(req: Value) -> Result<Value, String> {
    let shell = shell().ok_or("no window")?;
    let op = s(&req, "op").to_string();
    let timeout = req.get("timeout").and_then(Value::as_f64).unwrap_or(60.0);
    match op.as_str() {
        "status" => Ok(status()),
        "settings" => {
            page(&shell).await?;
            Ok(status())
        }
        "settings-close" => {
            if let Some(d) = current_dialog() {
                d.close();
            }
            sleep(300).await;
            Ok(json!({"closed": true}))
        }
        "type-code" => {
            let p = page(&shell).await?;
            p.type_code(s(&req, "code"));
            sleep(150).await;
            Ok(json!({"said": p.said_text(), "error": page_error()}))
        }
        "settings-scroll" => {
            let p = page(&shell).await?;
            p.scroll_to(req.get("fraction").and_then(Value::as_f64).unwrap_or(0.0));
            sleep(250).await;
            Ok(json!({"ok": true}))
        }
        "history-delete" => {
            let args: Map<String, Value> = req
                .get("args")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let text = super::mcp::history_delete(shell.clone(), args).await?;
            Ok(json!({"text": text}))
        }
        "canvas-eval" => {
            let h = crate::canvas::host::hosts_of(s(&req, "id"))
                .into_iter()
                .next()
                .ok_or("no host for that canvas")?;
            let v = h.eval_js(s(&req, "js")).await.map_err(|e| e.0)?;
            Ok(json!({"value": v}))
        }
        "canvas-hosts" => Ok(json!(
            crate::canvas::host::all()
                .iter()
                .map(|h| h.describe())
                .collect::<Vec<_>>()
        )),
        "connect" | "pair" => {
            let p = page(&shell).await?;
            if req.get("keep").and_then(Value::as_bool) != Some(true) {
                p.type_code(s(&req, "code"));
            }
            sleep(150).await;
            let said = p.said_text();
            let before = p.phase();
            p.press_connect();
            until(timeout, || {
                current().is_some_and(|p| p.phase() != before)
                    || page_error().is_some()
                    || current().is_some_and(|p| p.trust_shown().is_some())
            })
            .await;
            sleep(300).await;
            Ok(
                json!({"said": said, "error": page_error(), "trust": current().and_then(|p| p.trust_shown()), "status": status()}),
            )
        }
        "trust" => {
            let p = page(&shell).await?;
            let before = p.phase();
            p.press_trust();
            until(timeout, || {
                current().is_some_and(|p| p.phase() != before) || page_error().is_some()
            })
            .await;
            Ok(json!({"error": page_error(), "status": status()}))
        }
        "signup" | "signin" => {
            let p = page(&shell).await?;
            p.set_creating(op == "signup");
            sleep(100).await;
            let p = current().ok_or("page gone")?;
            p.fill_account(s(&req, "email"), s(&req, "password"), s(&req, "name"));
            p.press_account();
            until(timeout, || {
                current().is_some_and(|p| p.phase() != Phase::Linked) || page_error().is_some()
            })
            .await;
            Ok(json!({"error": page_error(), "status": status()}))
        }
        "turn-on" => {
            let p = page(&shell).await?;
            let domains: Vec<Domain> = match req.get("domains").and_then(Value::as_array) {
                Some(list) => list
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(Domain::parse)
                    .collect(),
                None => Domain::ALL.to_vec(),
            };
            p.pick(&domains);
            p.press_turn_on();
            until(timeout, || {
                current().is_some_and(|p| p.phase() == Phase::Syncing)
            })
            .await;
            Ok(status())
        }
        "make-code" => {
            let p = page(&shell).await?;
            p.make_code();
            until(timeout, || current().and_then(|p| p.code_text()).is_some()).await;
            Ok(json!({"code": current().and_then(|p| p.code_text())}))
        }
        "pairing" => Ok(json!({"card": current().map(|p| p.pairing_said())})),
        "rename-device" => {
            let c = get().ok_or("no cloud")?;
            let client = c.client.clone();
            let name = s(&req, "name").to_string();
            super::spawn(async move { client.rename_device(&name).await })
                .await
                .map_err(|e| e.to_string())?
                .map_err(|f| f.message)?;
            Ok(status())
        }
        "sync-now" => {
            let c = get().ok_or("no cloud")?;
            let sync = c.sync.clone();
            super::spawn(async move { sync.sync_now().await })
                .await
                .map_err(|e| e.to_string())?;
            Ok(status())
        }
        "space-add" => {
            let id = shell.add_space(s(&req, "name"));
            Ok(json!({"id": id}))
        }
        "space-rename" => {
            let id = space_id(&shell, s(&req, "name")).ok_or("no such space")?;
            shell.rename_space(&id, s(&req, "to"));
            Ok(json!({"id": id}))
        }
        "space-switch" => {
            let id = space_id(&shell, s(&req, "name")).ok_or("no such space")?;
            shell.switch_space(&id);
            Ok(json!({"id": id}))
        }
        "open" => {
            let space = match s(&req, "space") {
                "" => None,
                name => Some(space_id(&shell, name).ok_or("no such space")?),
            };
            let id = shell.open(
                s(&req, "url"),
                OpenTab {
                    foreground: req
                        .get("foreground")
                        .and_then(Value::as_bool)
                        .unwrap_or(true),
                    space,
                    at_end: true,
                    ..Default::default()
                },
            );
            if req.get("saved").and_then(Value::as_bool) == Some(true) {
                shell.set_saved(&id, true);
            }
            if let Some(letter) = req.get("pin").and_then(Value::as_str) {
                shell.with_browser(|b| b.pin(&id, Some(letter)));
            }
            Ok(json!({"id": id.0}))
        }
        "bookmark-add" => {
            let id = shell.bookmarks_mut().add(s(&req, "url"), s(&req, "title"));
            let list = shell.bookmarks().roots().to_vec();
            shell.saver().schedule(list);
            Ok(json!({"id": id}))
        }
        "visit" => {
            shell.history_mut().record(s(&req, "url"), s(&req, "title"));
            shell.history_changed();
            Ok(json!({"ok": true}))
        }
        "look" => {
            let look = match s(&req, "look") {
                "dark" => copper_core::settings::Look::Dark,
                "system" => copper_core::settings::Look::System,
                _ => copper_core::settings::Look::Light,
            };
            shell.set_look(look);
            Ok(json!({"look": shell.look().as_str()}))
        }
        "state" => Ok(state(&shell)),
        "wait" => {
            let what = s(&req, "what").to_string();
            let value = s(&req, "value").to_string();
            let ok = until(timeout, || holds(&what, &value)).await;
            Ok(json!({"ok": ok, "what": what, "value": value}))
        }
        "screenshot" => {
            crate::app::settle(&shell.downgrade(), Duration::from_secs(4)).await;
            let path = std::path::PathBuf::from(s(&req, "path"));
            crate::screenshot::save(shell.window().upcast_ref(), &path)
                .map_err(|e| e.to_string())?;
            Ok(json!({"path": path}))
        }
        "canvas" => {
            let tool = s(&req, "tool").to_string();
            let args: Map<String, Value> = req
                .get("args")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let items = crate::canvas::mcp::call(shell.clone(), tool, args).await?;
            Ok(json!({"items": items.into_iter().map(|i| match i {
                Value::Object(mut o) if o.get("type").and_then(Value::as_str) == Some("image") => {
                    let len = o.get("data").and_then(Value::as_str).map(str::len).unwrap_or(0);
                    o.insert("data".into(), json!(format!("<{len} base64 chars>")));
                    Value::Object(o)
                }
                other => other,
            }).collect::<Vec<_>>()}))
        }
        "select" => {
            let url = s(&req, "url");
            let id = shell
                .browser()
                .all_tabs()
                .find(|t| t.url.starts_with(url))
                .map(|t| t.id.clone())
                .ok_or("no such tab")?;
            shell.select(&id);
            Ok(json!({"id": id.0}))
        }
        "quit" => {
            shell.shutdown();
            shell.app().quit();
            Ok(json!({"quit": true}))
        }
        other => Err(format!("unknown op {other}")),
    }
}

/// What the browser holds now, for the E2E script.
fn state(shell: &Shell) -> Value {
    let b = shell.browser();
    let spaces: Vec<Value> = b
        .spaces()
        .map(|s| json!({"id": s.id, "name": s.name, "hue": s.hue}))
        .collect();
    let tabs: Vec<Value> = b
        .all_tabs()
        .map(|t| json!({"url": t.url, "title": t.title, "space": t.space, "pin": t.pin, "saved": t.saved, "sleeping": t.sleeping}))
        .collect();
    drop(b);
    let bookmarks = shell.bookmarks().urls();
    let history: Vec<String> = shell.history().visits().map(|v| v.url.clone()).collect();
    let devices: Vec<Value> = get()
        .map(|c| {
            c.sync
                .other_devices()
                .iter()
                .map(|d| json!({"name": d.name, "tabs": d.tabs.iter().map(|t| t.url.clone()).collect::<Vec<_>>()}))
                .collect()
        })
        .unwrap_or_default();
    json!({
        "spaces": spaces, "tabs": tabs, "bookmarks": bookmarks, "history": history,
        "devices": devices, "look": shell.look().as_str(),
        "settings": super::adapter::cloud_settings(shell),
    })
}

fn holds(what: &str, value: &str) -> bool {
    let Some(shell) = shell() else { return false };
    match what {
        "space" => shell.browser().spaces().any(|s| s.name == value),
        "no-space" => !shell.browser().spaces().any(|s| s.name == value),
        "tab" => shell.browser().all_tabs().any(|t| t.url.starts_with(value)),
        "pin" => shell
            .browser()
            .pins()
            .iter()
            .any(|t| t.url.starts_with(value)),
        "bookmark" => shell
            .bookmarks()
            .urls()
            .iter()
            .any(|u| u.starts_with(value)),
        "history" => shell.history().visits().any(|v| v.url.starts_with(value)),
        "device-tab" => get().is_some_and(|c| {
            c.sync
                .other_devices()
                .iter()
                .any(|d| d.tabs.iter().any(|t| t.url.starts_with(value)))
        }),
        "look" => shell.look().as_str() == value,
        "phase" => current().is_some_and(|p| format!("{:?}", p.phase()) == value),
        "synced" => get().is_some_and(|c| {
            let st = c.sync.status();
            st.busy == 0 && st.last_sync.is_some()
        }),
        _ => false,
    }
}
