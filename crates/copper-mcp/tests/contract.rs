//! The MCP contract against a fake browser: the catalogue is the Mac's,
//! the transport refuses what the Mac refuses with the same bytes, and a
//! scripted agent session drives tabs, pages and input through `Host`.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use copper_core::intelligence::IntelligenceKeys;
use copper_engine::{Image, InputEvent, SnapshotRegion};
use copper_mcp::client;
use copper_mcp::config::Endpoint;
use copper_mcp::host::{GroupInfo, Host, IntelligenceSource, Page, TabId, TabInfo};
use copper_mcp::server::{self, Body, RequestCtx};
use copper_mcp::transport::{self, TransportConfig};
use copper_mcp::{Mcp, Settings, Who};
use copper_pagejs::Script;
use serde_json::{Map, Value, json};

const TOKEN: &str = "t0k-for-tests";

// MARK: - the fake browser

#[derive(Default)]
struct World {
    tabs: Vec<TabInfo>,
    active: Option<TabId>,
    /// (tab short id, script file, args) for every page call.
    calls: Vec<(String, String, Value)>,
    inputs: Vec<(String, InputEvent)>,
    announced: Vec<String>,
    opened_foreground: Vec<bool>,
    released: Vec<TabId>,
    /// The fake page's state for Jev: clicked once → marker m2.
    clicked: bool,
    jev_endpoint: Option<String>,
}

#[derive(Clone)]
struct FakeHost(Rc<RefCell<World>>);

#[derive(Clone)]
struct FakePage {
    tab: TabId,
    world: Rc<RefCell<World>>,
}

fn invoked(body: &str) -> Option<(Script, Value)> {
    for script in Script::ALL {
        let marker = format!("return ({})(", script.function());
        if let Some(at) = body.find(&marker) {
            let rest = &body[at + marker.len()..];
            let args = rest.trim_end().trim_end_matches(';').trim_end_matches(')');
            let value = serde_json::from_str(args).unwrap_or(Value::Null);
            return Some((*script, value));
        }
    }
    None
}

impl Page for FakePage {
    fn call_async(
        &self,
        body: &str,
        _world: Option<&str>,
    ) -> impl Future<Output = Result<Value, String>> {
        let found = invoked(body);
        let world = self.world.clone();
        let tab = self.tab.clone();
        async move {
            let Some((script, args)) = found else {
                return Err("JavaScript: unknown script".to_string());
            };
            world
                .borrow_mut()
                .calls
                .push((tab.short(), script.file().to_string(), args.clone()));
            Ok(match script {
                Script::Snapshot => {
                    json!("- button \"Search\" [ref=e1]\n- textbox \"Query\" [ref=e2]")
                }
                Script::Rect => match args["target"].as_str() {
                    Some("ref:e1") => json!({"x": 10, "y": 20, "width": 100, "height": 30}),
                    Some("ref:e2") => json!({"x": 0, "y": 100, "width": 200, "height": 20}),
                    _ => json!({"error": "not found"}),
                },
                Script::Focus | Script::SetValue | Script::Submit => json!("ok"),
                Script::HasText => json!(true),
                Script::Text => json!("Example Domain"),
                Script::JevObserve => {
                    let w = world.borrow();
                    let url = w
                        .tabs
                        .iter()
                        .find(|t| t.id == tab)
                        .and_then(|t| t.url.clone())
                        .unwrap_or_default();
                    json!({
                        "url": url, "title": "Example Domain", "w": 1280, "h": 800,
                        "text": "Example Domain Search", "scroll": {"y": 0, "height": 800},
                        "actions": [{"id": "a1", "kind": "click", "label": "Search", "node": 1,
                                     "role": "button", "rect": {"x": 10, "y": 20, "w": 100, "h": 30}}],
                        "marker": if w.clicked { "m2" } else { "m1" },
                        "page_key": "k", "guards": {"1": "g"}, "omitted_actions": 0,
                    })
                }
                Script::JevMarker => json!(if world.borrow().clicked { "m2" } else { "m1" }),
                Script::JevCheck => json!(["k", "g"]),
                Script::JevResolve => json!({"x": 60, "y": 35}),
                Script::JevSettle => json!(true),
                _ => Value::Null,
            })
        }
    }

    async fn snapshot(&self, _region: SnapshotRegion) -> Result<Image, String> {
        Err("no pixels in the fake".to_string())
    }

    fn input(&self, event: InputEvent) -> impl Future<Output = Result<(), String>> {
        let mut w = self.world.borrow_mut();
        if matches!(event, InputEvent::Click { .. }) {
            w.clicked = true;
        }
        w.inputs.push((self.tab.short(), event));
        async { Ok(()) }
    }

    fn zoom(&self) -> f64 {
        1.0
    }

    fn view_size(&self) -> (f64, f64) {
        (1280.0, 800.0)
    }
}

impl FakeHost {
    fn new() -> Self {
        let mut world = World::default();
        for (i, (title, url)) in [
            ("Home", "https://home.test/"),
            ("Mail", "https://mail.test/"),
        ]
        .into_iter()
        .enumerate()
        {
            world.tabs.push(TabInfo {
                id: TabId(format!("0000000{i}-AAAA-BBBB-CCCC-DDDDDDDDDDDD")),
                index: Some(i),
                title: title.into(),
                url: Some(url.into()),
                pinned: i == 0,
                group: (i == 1).then(|| "Work".to_string()),
                selected: i == 0,
                ..TabInfo::default()
            });
        }
        world.active = Some(world.tabs[0].id.clone());
        FakeHost(Rc::new(RefCell::new(world)))
    }

    fn set(&self, id: &TabId, url: &str, title: &str) {
        let mut w = self.0.borrow_mut();
        if let Some(t) = w.tabs.iter_mut().find(|t| &t.id == id) {
            t.url = Some(url.into());
            t.title = title.into();
        }
    }
}

impl Host for FakeHost {
    type Page = FakePage;

    fn tabs(&self) -> Vec<TabInfo> {
        self.0.borrow().tabs.clone()
    }

    fn active_tab(&self) -> Option<TabId> {
        self.0.borrow().active.clone()
    }

    fn open_tab(&self, url: &str, foreground: bool) -> TabId {
        let mut w = self.0.borrow_mut();
        let id = TabId(format!(
            "{:08X}-1111-2222-3333-444444444444",
            0xABC0 + w.tabs.len()
        ));
        let index = w.tabs.len();
        w.tabs.push(TabInfo {
            id: id.clone(),
            index: Some(index),
            title: if url == "about:blank" {
                String::new()
            } else {
                "Example Domain".into()
            },
            url: Some(url.into()),
            ..TabInfo::default()
        });
        w.opened_foreground.push(foreground);
        if foreground {
            w.active = Some(id.clone());
        }
        id
    }

    fn show_tab(&self, id: &TabId) {
        let mut w = self.0.borrow_mut();
        w.active = Some(id.clone());
        for t in &mut w.tabs {
            t.selected = &t.id == id;
        }
    }

    fn close_tab(&self, id: &TabId) {
        self.0.borrow_mut().tabs.retain(|t| &t.id != id);
    }

    fn prepare_tab(&self, _id: &TabId) -> bool {
        false
    }

    fn release_tab(&self, id: &TabId) {
        self.0.borrow_mut().released.push(id.clone());
    }

    fn page(&self, id: &TabId) -> Option<FakePage> {
        self.tab(id).map(|_| FakePage {
            tab: id.clone(),
            world: self.0.clone(),
        })
    }

    fn navigate(&self, id: &TabId, url: &str) {
        self.set(id, url, "Example Domain");
    }

    fn go_back(&self, _id: &TabId) {}

    fn go_forward(&self, _id: &TabId) {}

    fn resize_window(&self, _width: f64, _height: f64) -> Result<(), String> {
        Ok(())
    }

    fn groups(&self) -> Vec<GroupInfo> {
        vec![GroupInfo {
            name: "Work".into(),
            members: vec!["Mail".into()],
        }]
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        tokio::time::sleep(duration)
    }

    fn announce(&self, text: &str) {
        self.0.borrow_mut().announced.push(text.to_string());
    }

    fn intelligence(&self) -> IntelligenceSource {
        let endpoint = self.0.borrow().jev_endpoint.clone();
        let keys: IntelligenceKeys = serde_json::from_value(json!({
            "jevKey": endpoint.as_ref().map(|_| "ts-test"),
            "jevEndpoint": endpoint,
        }))
        .unwrap();
        IntelligenceSource {
            local: keys,
            cloud: None,
        }
    }
}

// MARK: - plumbing

struct Running {
    server: transport::Server,
    host: FakeHost,
    mcp: Rc<Mcp<FakeHost>>,
}

fn start(jev: bool) -> Running {
    let host = FakeHost::new();
    let mcp = Mcp::new(
        host.clone(),
        Settings {
            jev,
            announces: true,
            version: "1.0.test".into(),
        },
    );
    let (tx, rx) = server::channel();
    let server = transport::start(
        TransportConfig {
            port: 0,
            token: Arc::new(RwLock::new(TOKEN.to_string())),
            version: "1.0.test".into(),
            headless: false,
        },
        tx,
    )
    .expect("bind a loopback port");
    mcp.set_runtime(server.runtime());
    tokio::task::spawn_local(mcp.clone().serve(rx));
    Running { server, host, mcp }
}

async fn http(
    port: u16,
    method: &str,
    headers: Vec<(&'static str, String)>,
    body: Value,
) -> client::Response {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).unwrap()
    };
    let method = method.to_string();
    tokio::task::spawn_blocking(move || {
        client::request(
            port,
            &method,
            "/mcp",
            &headers,
            &bytes,
            Duration::from_secs(30),
        )
    })
    .await
    .unwrap()
    .expect("the server answers")
}

fn authed() -> Vec<(&'static str, String)> {
    vec![
        ("Content-Type", "application/json".into()),
        ("Accept", "application/json, text/event-stream".into()),
        ("Authorization", format!("Bearer {TOKEN}")),
    ]
}

async fn rpc(port: u16, id: u64, method: &str, params: Value) -> Value {
    let r = http(
        port,
        "POST",
        authed(),
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    )
    .await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.header("content-type"), Some("application/json"));
    serde_json::from_slice(&r.body).unwrap()
}

async fn tool(port: u16, name: &str, arguments: Value) -> (String, bool) {
    let reply = rpc(
        port,
        9,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
    .await;
    let result = &reply["result"];
    let text = result["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    (text, result["isError"].as_bool().unwrap())
}

fn mac_tools() -> BTreeMap<String, Value> {
    let raw: Value = serde_json::from_str(copper_mcp::catalogue::MAC_TOOLS_LIST).unwrap();
    raw["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t.clone()))
        .collect()
}

fn local() -> tokio::task::LocalSet {
    tokio::task::LocalSet::new()
}

// MARK: - the contract

#[tokio::test]
async fn tools_list_is_the_mac_catalogue_minus_voice() {
    local()
        .run_until(async {
            let r = start(true);
            let reply = rpc(r.server.port, 2, "tools/list", json!({})).await;
            let ours = reply["result"]["tools"].as_array().unwrap().clone();
            let mac = mac_tools();
            assert_eq!(ours.len(), mac.len() - 2);
            for tool in &ours {
                let name = tool["name"].as_str().unwrap();
                assert_eq!(tool, &mac[name], "{name} differs from the Mac's");
            }
            for name in mac.keys() {
                let offered = ours.iter().any(|t| t["name"] == name.as_str());
                assert_eq!(offered, !name.starts_with("voice_"), "{name}");
            }
            assert!(reply["result"].get("nextCursor").is_none());

            // The Mac's wire bytes: same escaping, so the same length when
            // every tool is there (the fixture's 45 came to 32921 bytes;
            // 36024 with canvas_read's paging options, 2026-10-16; 36366 with
            // the integrated Mac build's canvas_read text, jev_run/jev_step
            // kept at the options Linux implements).
            let all: Vec<Value> = mac.values().cloned().collect();
            let wire = copper_pagejs::render::compact_json(
                &json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": all}}),
            );
            assert_eq!(wire.len(), 36366);
        })
        .await;
}

#[tokio::test]
async fn jev_tools_only_in_jev_mode() {
    local()
        .run_until(async {
            let r = start(false);
            let reply = rpc(r.server.port, 2, "tools/list", json!({})).await;
            let names: Vec<&str> = reply["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect();
            assert!(!names.iter().any(|n| n.starts_with("jev_")));
            assert_eq!(names.first(), Some(&"browser_tabs"));
            let (text, error) = tool(r.server.port, "jev_observe", json!({})).await;
            assert!(error);
            assert_eq!(text, "Jev mode is off — Settings › Agents › Jev mode");
        })
        .await;
}

#[tokio::test]
async fn refusals_have_the_mac_shape() {
    local()
        .run_until(async {
            let r = start(false);
            let port = r.server.port;
            let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});

            let bogus = vec![("Authorization", "Bearer nope".to_string())];
            let res = http(port, "POST", bogus.clone(), ping.clone()).await;
            assert_eq!(res.status, 401);
            assert_eq!(
                String::from_utf8(res.body.clone()).unwrap(),
                r#"{"error":"bearer token required — Settings › Agents"}"#
            );
            let names: Vec<&str> = res.headers.iter().map(|(n, _)| n.as_str()).collect();
            assert_eq!(
                names,
                [
                    "content-type",
                    "content-length",
                    "cache-control",
                    "connection"
                ]
            );
            assert_eq!(res.header("cache-control"), Some("no-store"));
            assert_eq!(res.header("connection"), Some("keep-alive"));

            let none = http(port, "POST", vec![], ping.clone()).await;
            assert_eq!(none.status, 401);

            let mut evil = authed();
            evil.push(("Origin", "https://evil.test".into()));
            let res = http(port, "POST", evil, ping.clone()).await;
            assert_eq!(res.status, 403);
            assert_eq!(
                String::from_utf8(res.body).unwrap(),
                r#"{"error":"forbidden origin"}"#
            );

            let mut both = bogus.clone();
            both.push(("Origin", "https://evil.test".into()));
            let res = http(port, "POST", both, ping.clone()).await;
            assert_eq!(res.status, 403, "Origin is checked before the token");

            let mut local_origin = authed();
            local_origin.push(("Origin", "http://localhost:5173".into()));
            assert_eq!(
                http(port, "POST", local_origin, ping.clone()).await.status,
                200
            );

            let res = http(port, "GET", authed(), Value::Null).await;
            assert_eq!(res.status, 405);
            assert_eq!(
                String::from_utf8(res.body).unwrap(),
                r#"{"error":"no event stream — POST JSON-RPC messages here"}"#
            );
            assert_eq!(
                http(port, "DELETE", authed(), Value::Null).await.status,
                200
            );

            let parse = tokio::task::spawn_blocking(move || {
                client::request(
                    port,
                    "POST",
                    "/mcp",
                    &authed(),
                    b"{nope",
                    Duration::from_secs(5),
                )
            })
            .await
            .unwrap()
            .unwrap();
            assert_eq!(parse.status, 400);
            assert_eq!(
                String::from_utf8(parse.body).unwrap(),
                r#"{"error":{"code":-32700,"message":"Parse error"},"id":null,"jsonrpc":"2.0"}"#
            );

            let health = tokio::task::spawn_blocking(move || {
                client::request(port, "GET", "/health", &[], &[], Duration::from_secs(5))
            })
            .await
            .unwrap()
            .unwrap();
            assert_eq!(health.status, 200, "health needs no token");
            let h: Value = serde_json::from_slice(&health.body).unwrap();
            assert_eq!(h["mcp"], "/mcp");
            assert_eq!(h["port"], port);

            let unknown = rpc(port, 5, "frobnicate", json!({})).await;
            assert_eq!(unknown["error"]["code"], -32601);
            assert_eq!(unknown["error"]["message"], "Method not found: frobnicate");
        })
        .await;
}

#[tokio::test]
async fn initialize_session_and_notifications() {
    local()
        .run_until(async {
            let r = start(true);
            let port = r.server.port;
            let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "1"}}});
            let res = http(port, "POST", authed(), init).await;
            assert_eq!(res.status, 200);
            let sid = res.header("mcp-session-id").expect("a session id").to_string();
            assert_eq!(sid.len(), 36);
            assert_eq!(sid, sid.to_lowercase());
            let body: Value = serde_json::from_slice(&res.body).unwrap();
            let result = &body["result"];
            assert_eq!(result["protocolVersion"], "2025-06-18");
            assert_eq!(result["serverInfo"], json!({"name": "copper", "version": "1.0.test"}));
            assert_eq!(result["capabilities"], json!({"tools": {"listChanged": false}, "resources": {}, "prompts": {}}));
            let mac: Value = serde_json::from_str(include_str!("../../../fixtures/mac/mcp-initialize.json")).unwrap();
            assert!(mac["result"]["instructions"]
                .as_str()
                .unwrap()
                .starts_with(result["instructions"].as_str().unwrap()));

            let mut with_session = authed();
            with_session.push(("Mcp-Session-Id", sid.clone()));
            let note = http(port, "POST", with_session.clone(), json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
            assert_eq!(note.status, 202);
            assert!(note.body.is_empty());
            assert_eq!(note.header("content-length"), Some("0"));

            // A later call from that session is Claude Code's hand.
            let call = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "browser_tabs", "arguments": {"action": "list"}}});
            let res = http(port, "POST", with_session, call).await;
            assert_eq!(res.status, 200);
            assert!(res.header("mcp-session-id").is_none());
            assert!(r.host.0.borrow().announced.contains(&"Agent · browser_tabs".to_string()));

            let unknown_version = rpc(port, 4, "initialize", json!({"protocolVersion": "1999-01-01"})).await;
            assert_eq!(unknown_version["result"]["protocolVersion"], "2025-06-18");
        })
        .await;
}

#[tokio::test]
async fn scripted_session_tabs_navigate_snapshot_click_type() {
    local()
        .run_until(async {
            let r = start(false);
            let port = r.server.port;

            let (list, error) = tool(port, "browser_tabs", json!({"action": "list"})).await;
            assert!(!error);
            assert_eq!(
                list,
                "### Open tabs\n- 0: (on screen, yours) [Home] (https://home.test/) {tab 00000000} (pinned)\n- 1: [Mail] (https://mail.test/) {tab 00000001} [Work]"
            );

            let (opened, error) = tool(port, "browser_tabs", json!({"action": "new", "url": "example.com"})).await;
            assert!(!error, "{opened}");
            assert!(opened.starts_with("Opened in the background — the user's tab did not change; your calls now land on it. {tab 0000abc2}\n### Page\n- URL: https://example.com/\n- Title: Example Domain\n- Tab: 0000abc2 (background — not on the user's screen)"), "{opened}");
            assert!(opened.contains("- 2: (yours) [Example Domain] (https://example.com/) {tab 0000abc2}"), "{opened}");
            assert_eq!(r.host.0.borrow().opened_foreground, [false]);
            assert_eq!(r.host.active_tab().unwrap().short(), "00000000", "the user's tab did not change");

            let (nav, error) = tool(port, "browser_navigate", json!({"url": "https://example.org/a"})).await;
            assert!(!error);
            assert_eq!(nav, "### Page\n- URL: https://example.org/a\n- Title: Example Domain\n- Tab: 0000abc2 (background — not on the user's screen)");

            let (snap, error) = tool(port, "browser_snapshot", json!({"interactive": true})).await;
            assert!(!error);
            assert!(snap.ends_with("\n\n- button \"Search\" [ref=e1]\n- textbox \"Query\" [ref=e2]"), "{snap}");
            {
                let w = r.host.0.borrow();
                let (tab, file, args) = w.calls.iter().find(|c| c.1 == "page/snapshot.js").unwrap();
                assert_eq!(tab, "0000abc2", "the call landed on the agent's own tab");
                assert_eq!(file, "page/snapshot.js");
                assert_eq!(args, &json!({"selector": "", "interactive": true, "limit": 600}));
            }

            let (clicked, error) = tool(port, "browser_click", json!({"ref": "e1", "element": "Search", "reason": "search"})).await;
            assert!(!error, "{clicked}");
            assert!(clicked.starts_with("Clicked Search. ### Page\n- URL: https://example.org/a"), "{clicked}");

            let (typed, error) = tool(port, "browser_type", json!({"ref": "e2", "text": "copper", "submit": true})).await;
            assert!(!error, "{typed}");
            assert!(typed.starts_with("Typed into ref:e2 and submitted. ### Page"), "{typed}");

            let w = r.host.0.borrow();
            let inputs: Vec<&InputEvent> = w.inputs.iter().map(|(_, e)| e).collect();
            assert_eq!(inputs.len(), 3, "{inputs:?}");
            assert!(matches!(inputs[0], InputEvent::Click { x, y, count: 1, .. } if *x == 60.0 && *y == 35.0));
            assert!(matches!(inputs[1], InputEvent::Click { x, y, .. } if *x == 100.0 && *y == 110.0));
            assert!(matches!(inputs[2], InputEvent::Key { key, .. } if key == "Enter"));
            let set = w.calls.iter().find(|c| c.1 == "page/set_value.js").unwrap();
            assert_eq!(set.2, json!({"target": "ref:e2", "text": "copper"}));
            assert!(w.calls.iter().all(|c| c.0 == "0000abc2"));
        })
        .await;
}

#[tokio::test]
async fn named_tabs_errors_and_groups() {
    local()
        .run_until(async {
            let r = start(false);
            let port = r.server.port;
            let (text, error) = tool(port, "browser_get_text", json!({"tab": "00000001"})).await;
            assert!(!error);
            assert_eq!(text, "Example Domain");
            assert_eq!(r.host.0.borrow().calls.last().unwrap().0, "00000001");

            let (text, error) = tool(port, "browser_click", json!({"tab": 7})).await;
            assert!(error);
            assert!(text.starts_with("no tab 7 — ### Open tabs\n"), "{text}");

            let (text, error) = tool(port, "browser_click", json!({})).await;
            assert!(error);
            assert_eq!(text, "ref (from browser_snapshot) or selector required");

            let (text, error) = tool(port, "browser_click", json!({"ref": "e9"})).await;
            assert!(error);
            assert_eq!(
                text,
                "ref:e9: not found. Take a fresh browser_snapshot — refs change when the page does."
            );

            let (text, _) = tool(port, "browser_groups", json!({"action": "list"})).await;
            assert_eq!(text, "- Work: [Mail]");

            let (text, error) = tool(port, "canvas_list", json!({})).await;
            assert!(error);
            assert_eq!(text, "Canvases are not available in this build yet");

            let (text, error) = tool(port, "voice_status", json!({})).await;
            assert!(error);
            assert_eq!(text, "Unknown tool voice_status");

            let (text, _) = tool(
                port,
                "browser_resize",
                json!({"width": 1024, "height": 700}),
            )
            .await;
            assert_eq!(text, "Window is 1024×700");
        })
        .await;
}

#[tokio::test]
async fn digit_ids_and_tabs_in_other_spaces() {
    local()
        .run_until(async {
            let r = start(false);
            let port = r.server.port;
            {
                // One tab in another space (no index) whose short id is all
                // digits, like the QA run's 72135451.
                let mut w = r.host.0.borrow_mut();
                w.tabs.push(TabInfo {
                    id: TabId("72135451-AAAA-BBBB-CCCC-DDDDDDDDDDDD".into()),
                    index: None,
                    title: "Elsewhere".into(),
                    url: Some("https://elsewhere.test/".into()),
                    ..TabInfo::default()
                });
            }

            // The list names it after the row, without an index.
            let (list, error) = tool(port, "browser_tabs", json!({"action": "list"})).await;
            assert!(!error);
            assert_eq!(
                list,
                "### Open tabs\n- 0: (on screen, yours) [Home] (https://home.test/) {tab 00000000} (pinned)\n- 1: [Mail] (https://mail.test/) {tab 00000001} [Work]\n- -: [Elsewhere] (https://elsewhere.test/) {tab 72135451}"
            );

            // An all-digit short id selects that tab, not "index 72135451".
            let (text, error) = tool(port, "browser_tabs", json!({"action": "select", "tab": "72135451"})).await;
            assert!(!error, "{text}");
            assert!(text.starts_with("Selected — your calls now land on this tab; the user's view did not change. {tab 72135451}"), "{text}");
            let (text, error) = tool(port, "browser_get_text", json!({"tab": "7213"})).await;
            assert!(!error, "{text}");
            assert_eq!(r.host.0.borrow().calls.last().unwrap().0, "72135451");

            // Short digit strings and JSON numbers stay indices.
            let (_, error) = tool(port, "browser_get_text", json!({"tab": "1"})).await;
            assert!(!error);
            assert_eq!(r.host.0.borrow().calls.last().unwrap().0, "00000001");
            let (_, error) = tool(port, "browser_get_text", json!({"tab": 0})).await;
            assert!(!error);
            assert_eq!(r.host.0.borrow().calls.last().unwrap().0, "00000000");
            // A number no tab has, that starts an id: the id.
            let (_, error) = tool(port, "browser_get_text", json!({"tab": "72"})).await;
            assert!(!error);
            assert_eq!(r.host.0.borrow().calls.last().unwrap().0, "72135451");
            // Neither an index nor an id: the Mac's error.
            let (text, error) = tool(port, "browser_get_text", json!({"tab": "9"})).await;
            assert!(error);
            assert!(text.starts_with("no tab 9 — ### Open tabs\n"), "{text}");
            // Long digits matching no id are an index no tab has.
            let (text, error) = tool(port, "browser_get_text", json!({"tab": "5555"})).await;
            assert!(error);
            assert!(text.starts_with("no tab 5555 — "), "{text}");
        })
        .await;
}

#[tokio::test]
async fn stop_refuses_the_hand() {
    local()
        .run_until(async {
            let r = start(false);
            let who = Who::pane();
            let mut message = Map::new();
            message.insert("jsonrpc".into(), json!("2.0"));
            message.insert("id".into(), json!(1));
            message.insert("method".into(), json!("tools/call"));
            message.insert(
                "params".into(),
                json!({"name": "browser_tabs", "arguments": {"action": "list"}}),
            );
            r.mcp.stop(&who.key);
            let reply = r
                .mcp
                .handle(&message, &RequestCtx::default(), Some(who.clone()))
                .await
                .unwrap();
            assert_eq!(reply["result"]["isError"], true);
            assert!(
                reply["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .starts_with("Stopped by the user")
            );
            r.mcp.resume();
            let reply = r
                .mcp
                .answer(Body::One(message), &RequestCtx::default())
                .await
                .unwrap();
            assert_eq!(reply["result"]["isError"], false);
        })
        .await;
}

#[tokio::test]
async fn cli_and_bridge_talk_to_the_running_server() {
    local()
        .run_until(async {
            let r = start(false);
            let port = r.server.port;
            let endpoint = Endpoint {
                enabled: true,
                port,
                token: TOKEN.into(),
            };

            let e = endpoint.clone();
            let (code, out, err) = tokio::task::spawn_blocking(move || {
                let (mut out, mut err) = (Vec::new(), Vec::new());
                let ep = || Some(e.clone());
                let launch = || Ok(());
                let mut env = copper_mcp::cli::Env {
                    endpoint: &ep,
                    launch: &launch,
                    launch_env: false,
                    out: &mut out,
                    err: &mut err,
                };
                let code = copper_mcp::cli::run_with(&["tabs".to_string()], &mut env);
                (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
            })
            .await
            .unwrap();
            assert_eq!(code, 0, "{err}");
            assert!(out.starts_with("### Open tabs\n- 0: "), "{out}");

            let e = endpoint.clone();
            let (code, err) = tokio::task::spawn_blocking(move || {
                let (mut out, mut err) = (Vec::new(), Vec::new());
                let ep = || Some(e.clone());
                let launch = || Ok(());
                let mut env = copper_mcp::cli::Env {
                    endpoint: &ep,
                    launch: &launch,
                    launch_env: false,
                    out: &mut out,
                    err: &mut err,
                };
                let code = copper_mcp::cli::run_with(&["click".into(), "e42".into()], &mut env);
                (code, String::from_utf8(err).unwrap())
            })
            .await
            .unwrap();
            assert_eq!(code, 1);
            assert!(err.starts_with("copper: ref:e42: not found."), "{err}");

            let e = endpoint.clone();
            let out = tokio::task::spawn_blocking(move || {
                let input = concat!(
                    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"phi"}}}"#, "\n",
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, "\n",
                    r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#, "\n",
                );
                let mut reader = std::io::BufReader::new(input.as_bytes());
                let (mut out, mut log) = (Vec::new(), Vec::new());
                copper_mcp::bridge::run_with(&mut reader, &mut out, &mut log, &|| Some(e.clone()), &|| Ok(()))
                    .unwrap();
                String::from_utf8(out).unwrap()
            })
            .await
            .unwrap();
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines.len(), 2, "{out}");
            assert!(lines[0].starts_with(r#"{"id":1,"jsonrpc":"2.0","result":{"#), "{}", lines[0]);
            assert_eq!(lines[1], r#"{"id":2,"jsonrpc":"2.0","result":{}}"#);

            // Copper off: every request gets the refusal, initialize included.
            let out = tokio::task::spawn_blocking(move || {
                let input = "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"initialize\"}\n";
                let mut reader = std::io::BufReader::new(input.as_bytes());
                let (mut out, mut log) = (Vec::new(), Vec::new());
                let off = Endpoint { enabled: false, ..endpoint.clone() };
                copper_mcp::bridge::run_with(&mut reader, &mut out, &mut log, &|| Some(off.clone()), &|| Ok(()))
                    .unwrap();
                String::from_utf8(out).unwrap()
            })
            .await
            .unwrap();
            assert_eq!(
                out.trim_end(),
                r#"{"error":{"code":-32000,"message":"Copper's agent server is off — Settings › Agents › Let agents drive this window"},"id":7,"jsonrpc":"2.0"}"#
            );
        })
        .await;
}

/// A TypeSafe stand-in: answers each POST with the next canned body and
/// keeps what it was sent.
fn fake_jev(answers: Vec<Value>) -> (String, std::sync::mpsc::Receiver<(String, Value)>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://127.0.0.1:{}/v1/systemone",
        listener.local_addr().unwrap().port()
    );
    let (seen, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for answer in answers {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut raw = Vec::new();
            let mut chunk = [0u8; 8192];
            let body = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    return;
                }
                raw.extend_from_slice(&chunk[..n]);
                let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&raw[..end]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if raw.len() >= end + 4 + length {
                    let auth = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("authorization:")
                                .map(|v| v.trim().to_string())
                        })
                        .unwrap_or_default();
                    let body: Value =
                        serde_json::from_slice(&raw[end + 4..end + 4 + length]).unwrap();
                    break (auth, body);
                }
            };
            let _ = seen.send(body);
            let text = answer.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    (url, received)
}

#[tokio::test]
async fn jev_run_observes_asks_typesafe_and_acts() {
    local()
        .run_until(async {
            let (url, received) = fake_jev(vec![
                json!({"answers": {
                    "operation": {"choice": "CLICK", "confidence": 0.9, "probabilities": {"CLICK": 0.9, "DONE": 0.1}},
                    "click_target": {"choice": "1", "confidence": 0.8, "probabilities": {"1": 0.8}},
                }, "usage": {"input_tokens": 100}}),
                json!({"answers": {
                    "operation": {"choice": "DONE", "confidence": 0.95, "probabilities": {"DONE": 0.95}},
                }}),
            ]);
            let r = start(true);
            r.host.0.borrow_mut().jev_endpoint = Some(url);
            let port = r.server.port;

            let (observed, error) = tool(port, "jev_observe", json!({"text": false})).await;
            assert!(!error, "{observed}");
            assert_eq!(
                observed,
                "### Page\n- URL: https://home.test/\n- Title: Home\n- Tab: 00000000\n\n### Elements\n[1] button     Search"
            );

            let reply = rpc(port, 11, "tools/call", json!({"name": "jev_run", "arguments": {"goal": "press search"}})).await;
            let result = &reply["result"];
            let report = result["content"][0]["text"].as_str().unwrap();
            assert_eq!(result["isError"], false, "{report}");
            assert!(report.starts_with("### Jev run — done in "), "{report}");
            assert!(report.contains("(1 action, 2 decisions)\nJev said DONE, 95% sure\nGoal: press search\n\n1. CLICK Search (80%, "), "{report}");
            assert!(report.contains(" ms) → page changed\n\n### Page\n- URL: https://home.test/"), "{report}");
            assert!(report.ends_with("DONE is Jev's claim — check the page (browser_snapshot / jev_observe) before telling the user it worked."));
            let summary = result["_meta"]["summary"].as_str().unwrap();
            assert!(summary.starts_with("done · 1 action · ") && summary.ends_with(" · home.test"), "{summary}");

            let (auth, first) = received.recv().unwrap();
            assert_eq!(auth, "bearer ts-test");
            assert_eq!(first["model"], "jev-latest");
            assert_eq!(first["questions"]["operation"]["type"], "choice");
            assert_eq!(
                first["questions"]["click_target"]["criteria"]["1"],
                json!({"element": "[1] Search", "current_value": "", "role": "button"})
            );
            assert_eq!(first["state"]["elements"][0]["operations"], json!(["CLICK"]));
            let (_, second) = received.recv().unwrap();
            assert_eq!(second["state"]["recent_actions"][0]["page_changed"], true);

            let w = r.host.0.borrow();
            assert!(matches!(&w.inputs[0].1, InputEvent::Click { x, y, .. } if *x == 60.0 && *y == 35.0));
        })
        .await;
}
