//! The `canvas_*` tools (Mac: `CanvasTools.swift`, `CanvasChatUI.swift`
//! `canvas_chat`). The signature of [`call`] is a fixed cross-lane contract:
//! each item is a text (a JSON string) or `{type:"image", data, mimeType}`.

use super::host::CanvasHost;
use super::registry::{self, is_personal, is_shared};
use crate::shell::Shell;
use copper_core::canvas::CanvasEntry;
use serde_json::{Map, Value, json};
use std::rc::Rc;

/// `tool` is the full tool name (e.g. `canvas_read`). Returns JSON content
/// items as the Mac does (a JSON value per content item; text items as
/// strings).
pub async fn call(
    shell: Shell,
    tool: String,
    args: Map<String, Value>,
) -> Result<Vec<Value>, String> {
    run(&shell, &tool, &args).await
}

/// JSON with sorted keys and unescaped slashes (the Mac's `.sortedKeys,
/// .withoutEscapingSlashes`).
pub fn json_text(v: &Value) -> String {
    fn walk(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), walk(&m[k]));
                }
                Value::Object(out)
            }
            Value::Array(a) => Value::Array(a.iter().map(walk).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&walk(v)).unwrap_or_else(|_| "{}".into())
}

fn s<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `id` (an id or a name), or `name`; nothing → the canvas in front, else Personal.
fn resolve(
    shell: &Shell,
    args: &Map<String, Value>,
    fallback_to_active: bool,
) -> Result<CanvasEntry, String> {
    for key in ["id", "name"] {
        if let Some(given) = s(args, key) {
            return registry::find(given)
                .ok_or_else(|| format!("no canvas “{given}” — canvas_list names them"));
        }
    }
    if fallback_to_active
        && let Some(id) = super::active(shell)
        && let Some(e) = registry::entry(&id)
    {
        return Ok(e);
    }
    Ok(registry::personal())
}

async fn open(shell: &Shell, e: &CanvasEntry, foreground: bool) -> Result<Rc<CanvasHost>, String> {
    super::open(shell, &e.id, foreground).await
}

fn summary(shell: &Shell, e: &CanvasEntry) -> Value {
    let hosts = super::host::hosts_of(&e.id);
    let live = hosts.iter().any(|h| h.room_open());
    let collaborators = hosts.iter().map(|h| h.collaborators()).max().unwrap_or(0);
    let mut out = json!({
        "id": e.id, "name": e.name, "kind": e.kind,
        "open": super::tab_showing(shell, &e.id).is_some(),
        "live": live, "collaborators": collaborators,
        "url": super::url_for(&e.id),
        "updatedAt": e.updated_at.to_string(),
    });
    if is_shared(e) {
        out["role"] = json!(e.role.clone().unwrap_or_else(|| "editor".into()));
        if let Some(m) = e.members {
            out["members"] = json!(m);
        }
        if let Some(o) = &e.owner {
            out["owner"] = json!(o);
        }
    }
    out
}

fn cloud_line() -> String {
    if registry::cloud_ready() {
        let personal = if registry::personal_syncs() {
            "Personal syncs with the account"
        } else {
            "Personal stays on this device (its sync is off)"
        };
        return format!("signed in — new canvases are shared and can take invites; {personal}");
    }
    if crate::cloud::get().is_some_and(|c| c.client.is_linked()) {
        return "linked, not signed in — canvases are local".into();
    }
    "not connected — canvases are local to this device".into()
}

/// Who an agent's ops are attributed to (`as`: a name or `{name, color}`).
fn caller(said: Option<&Value>) -> Value {
    let mut name = "Agent".to_string();
    let mut color: Option<String> = None;
    match said {
        Some(Value::String(t)) if !t.trim().is_empty() => {
            name = t.trim().chars().take(60).collect()
        }
        Some(Value::Object(o)) => {
            if let Some(n) = o
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
            {
                name = n.chars().take(60).collect();
            }
            if let Some(c) = o
                .get("color")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
            {
                color = Some(c.to_string());
            }
        }
        _ => {}
    }
    let id = format!("agent:{}", name.to_lowercase());
    let color = color.unwrap_or_else(|| super::stable_color(&name));
    json!({"id": id, "name": name, "color": color})
}

/// The options `canvas_read` hands the page's `read()` as they came
/// (canvas-perf contract B): the page reads the document, pages and narrows.
pub const READ_OPTIONS: [&str; 9] = [
    "full", "overview", "frame", "region", "ids", "types", "query", "limit", "cursor",
];

/// `canvas_read`'s arguments as the page's `read(opts)`: the known options
/// only (the tool's own `id`, `name` and `reason` stay here), `full` always
/// there.
pub fn read_opts(args: &Map<String, Value>) -> Value {
    let mut opts = Map::new();
    for key in READ_OPTIONS {
        match args.get(key) {
            None | Some(Value::Null) => {}
            Some(v) => {
                opts.insert(key.to_string(), v.clone());
            }
        }
    }
    if !opts.get("full").is_some_and(Value::is_boolean) {
        opts.insert("full".into(), json!(false));
    }
    Value::Object(opts)
}

fn ids(args: &Map<String, Value>) -> Vec<String> {
    args.get("shapeIds")
        .or_else(|| args.get("ids"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

async fn run(shell: &Shell, tool: &str, args: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let fail = |e: super::host::Failure| e.0;
    match tool {
        "canvas_list" => {
            let rows: Vec<Value> = registry::visible()
                .iter()
                .map(|e| summary(shell, e))
                .collect();
            Ok(vec![Value::String(json_text(
                &json!({"canvases": rows, "cloud": cloud_line()}),
            ))])
        }
        "canvas_open" => {
            let e = resolve(shell, args, false)?;
            let foreground = args
                .get("foreground")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let h = open(shell, &e, foreground).await?;
            h.wait_ready(super::host::READY_WITHIN)
                .await
                .map_err(fail)?;
            Ok(vec![Value::String(json_text(&summary(shell, &e)))])
        }
        "canvas_read" => {
            let e = resolve(shell, args, true)?;
            let h = open(shell, &e, false).await?;
            let read = h.read(&read_opts(args)).await.map_err(fail)?;
            Ok(vec![Value::String(json_text(&read))])
        }
        "canvas_apply" => {
            let ops = args
                .get("ops")
                .and_then(Value::as_array)
                .filter(|o| !o.is_empty())
                .ok_or("ops (a non-empty array) required")?;
            if ops.len() > 1000 {
                return Err("at most 1000 ops per call".into());
            }
            let e = resolve(shell, args, true)?;
            let h = open(shell, &e, false).await?;
            let result = h.apply(ops, &caller(args.get("as"))).await.map_err(fail)?;
            Ok(vec![Value::String(json_text(&result))])
        }
        "canvas_select" | "canvas_focus" => {
            let e = resolve(shell, args, true)?;
            let wanted = ids(args);
            let h = open(shell, &e, false).await?;
            let n = plural(wanted.len(), "shape", "shapes");
            if tool == "canvas_select" {
                h.select(&wanted).await.map_err(fail)?;
                return Ok(vec![Value::String(format!("Selected {n} on {}", e.name))]);
            }
            if wanted.is_empty() {
                return Err("shapeIds required".into());
            }
            h.zoom_to(&wanted).await.map_err(fail)?;
            Ok(vec![Value::String(format!("Showing {n} on {}", e.name))])
        }
        "canvas_create" => {
            let name = s(args, "name").ok_or("name required")?;
            let e = super::create(name).await?;
            Ok(vec![Value::String(json_text(&summary(shell, &e)))])
        }
        "canvas_invite" => {
            let key = s(args, "id").unwrap_or("");
            let e = registry::find(key)
                .ok_or_else(|| format!("no canvas {key} — canvas_list names them"))?;
            let email = s(args, "email").ok_or("email required")?.to_string();
            if is_personal(&e) {
                return Err("The Personal canvas can't be shared.".into());
            }
            if !registry::cloud_ready() {
                return Err("Sign in at Settings › Cloud to share canvases.".into());
            }
            let remote = e
                .remote_id
                .clone()
                .filter(|_| is_shared(&e))
                .ok_or_else(|| {
                    format!(
                        "“{}” is on this device only — make a new canvas while signed in to share.",
                        e.name
                    )
                })?;
            if !email.contains('@') || email.contains(' ') {
                return Err(format!("“{email}” isn't an email address."));
            }
            let api = crate::cloud::get()
                .ok_or("Copper Cloud is off")?
                .canvas
                .clone();
            let to = email.clone();
            let invite = crate::cloud::spawn(async move { api.invite(&remote, &to).await })
                .await
                .map_err(|_| "stopped".to_string())?
                .map_err(|f| format!("Couldn't invite {email} — {}", f.message))?;
            let who = invite
                .invitee
                .as_ref()
                .and_then(|p| p.display_name.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| email.clone());
            let said = match invite.nudged {
                Some(true) => format!("Reminded {who} about {}", e.name),
                Some(false) => format!(
                    "{who} was invited to {} moments ago — too soon to remind them",
                    e.name
                ),
                None if invite.status.as_deref() == Some("pending")
                    || invite.created_at.is_some() =>
                {
                    format!("Invited {who} to {}", e.name)
                }
                None => format!("{email} was already invited to {}", e.name),
            };
            Ok(vec![Value::String(said)])
        }
        "canvas_share_link" => {
            let e = resolve(shell, args, false)?;
            let remote = e
                .remote_id
                .clone()
                .filter(|_| is_shared(&e))
                .ok_or("This canvas is on this device only and can't be shared")?;
            let cloud = crate::cloud::get()
                .filter(|_| registry::cloud_ready())
                .ok_or("Sign in at Settings › Cloud to share canvases.")?;
            let host = cloud
                .client
                .link()
                .map(|l| l.host())
                .ok_or("Copper Cloud isn't linked")?;
            let token = share_token(&remote).await?;
            let q =
                |t: &str| url::form_urlencoded::byte_serialize(t.as_bytes()).collect::<String>();
            Ok(vec![Value::String(json_text(&json!({
                "link": format!("copper://canvas/join/{token}?cloud={}&n={}", q(&host), q(&e.name)),
                "webLink": format!("https://{host}/join/{token}"),
            })))])
        }
        "canvas_join" => {
            let text = s(args, "link").ok_or("link required")?;
            let link = super::JoinLink::parse(text)
                .ok_or_else(|| format!("not a canvas invite link: {text}"))?;
            if !link.copper {
                let linked = crate::cloud::get()
                    .and_then(|c| c.client.link())
                    .map(|l| super::JoinLink::normalize(&l.host()));
                if linked.as_deref() != Some(super::JoinLink::normalize(&link.cloud).as_str()) {
                    return Err(
                        "couldn't open that invite — it may be for another Copper Cloud".into(),
                    );
                }
            }
            let said = super::join(shell, &link, None).await?;
            Ok(vec![Value::String(said)])
        }
        "canvas_screenshot" => {
            let e = resolve(shell, args, true)?;
            let h = open(shell, &e, false).await?;
            let png = h.picture().await.map_err(fail)?;
            Ok(vec![
                json!({"type": "image", "data": glib::base64_encode(&png).to_string(), "mimeType": "image/png"}),
                Value::String(format!("Canvas · {}", e.name)),
            ])
        }
        "canvas_chat" => {
            let e = resolve(shell, args, true)?;
            if !(is_shared(&e) && e.remote_id.is_some() && registry::cloud_ready()) {
                return Err(if is_shared(&e) {
                    format!("Sign in to Copper Cloud to chat on “{}”", e.name)
                } else {
                    format!("“{}” has no chat — only shared canvases do", e.name)
                });
            }
            let text = s(args, "text").ok_or("text required")?;
            let mentions: Vec<String> = args
                .get("mentions")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let h = open(shell, &e, false).await?;
            let input = json!({"text": text, "mentions": mentions}).to_string();
            let answer = h.chat_send(&input).await.map_err(fail)?;
            if answer.get("ok").and_then(Value::as_bool) != Some(true) {
                return Err(answer
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("the message wasn't sent")
                    .to_string());
            }
            Ok(vec![Value::String(json_text(&json!({
                "sent": true, "id": answer.get("id").cloned().unwrap_or(json!("")),
                "canvas": e.name, "mentions": answer.get("mentions").cloned().unwrap_or(json!([])),
            })))])
        }
        other => Err(format!("Unknown tool {other}")),
    }
}

/// The share link's token for a canvas: kept per canvas (0600) so the same
/// link is handed out again, made on first use.
async fn share_token(remote: &str) -> Result<String, String> {
    let shell = super::shell().ok_or("no window")?;
    let path = shell
        .store()
        .paths()
        .data
        .join("canvas")
        .join("share-links.json");
    let mut saved: Map<String, Value> = std::fs::read(&path)
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    if let Some(t) = saved
        .get(remote)
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        return Ok(t.to_string());
    }
    let api = crate::cloud::get()
        .ok_or("Copper Cloud is off")?
        .canvas
        .clone();
    let id = remote.to_string();
    let made = crate::cloud::spawn(async move { api.create_share_link(&id).await })
        .await
        .map_err(|_| "stopped".to_string())?
        .map_err(|f| {
            if f.status == 404 {
                "This Copper Cloud is too old for invite links (canvas_invite takes an email.)"
                    .to_string()
            } else {
                format!("Couldn't make an invite link — {}", f.message)
            }
        })?;
    saved.insert(remote.to_string(), json!(made.token));
    let _ = copper_core::store::write_atomic(
        &path,
        Value::Object(saved).to_string().as_bytes(),
        true,
        false,
    );
    Ok(made.token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_json_and_caller() {
        assert_eq!(
            json_text(&json!({"b": "a/b", "a": 1})),
            r#"{"a":1,"b":"a/b"}"#
        );
        let c = caller(Some(&json!("Claude")));
        assert_eq!(c["id"], "agent:claude");
        assert_eq!(c["name"], "Claude");
        let d = caller(Some(&json!({"name": "Bot", "color": "#fff"})));
        assert_eq!(d["color"], "#fff");
        assert_eq!(caller(None)["name"], "Agent");
    }

    #[test]
    fn read_options_pass_through() {
        let args = json!({
            "id": "Roadmap", "reason": "why", "overview": true, "limit": 50,
            "region": {"x": 0, "y": 0, "w": 10, "h": 10}, "cursor": null, "bogus": 1,
        });
        let opts = read_opts(args.as_object().unwrap());
        assert_eq!(
            opts,
            json!({"full": false, "overview": true, "limit": 50, "region": {"x": 0, "y": 0, "w": 10, "h": 10}})
        );
        let full = read_opts(
            json!({"full": true, "types": ["frame"]})
                .as_object()
                .unwrap(),
        );
        assert_eq!(full, json!({"full": true, "types": ["frame"]}));
    }

    #[test]
    fn a_refused_read_is_a_tool_error() {
        use super::super::host::read_refused;
        let refused = json!({"error": "no shape f9 on this canvas — overview lists the frames"});
        assert_eq!(
            read_refused(refused.as_object().unwrap()).as_deref(),
            Some("no shape f9 on this canvas — overview lists the frames")
        );
        let shapes = json!({"error": "x", "shapes": []});
        let overview = json!({"error": "x", "overview": true});
        let fine = json!({"shapes": [], "total": 0});
        assert_eq!(read_refused(shapes.as_object().unwrap()), None);
        assert_eq!(read_refused(overview.as_object().unwrap()), None);
        assert_eq!(read_refused(fine.as_object().unwrap()), None);
    }

    #[test]
    fn assignees_anywhere_in_the_ops() {
        use super::super::host::mentions_key;
        let add = json!([{"op": "add", "shape": {"type": "checklist", "rows": ["a"], "assignees": {"a": ["Ann"]}}}]);
        let update = json!([{"op": "update", "id": "c1", "patch": {"assignees": {"a": null}}}]);
        let plain = json!([{"op": "add", "shape": {"type": "sticky", "text": "assignees"}}]);
        assert!(mentions_key(add.as_array().unwrap(), "assignees"));
        assert!(mentions_key(update.as_array().unwrap(), "assignees"));
        assert!(!mentions_key(plain.as_array().unwrap(), "assignees"));
    }
}
