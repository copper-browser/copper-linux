//! The loop against a tiny router on loopback: a tool call, its result and
//! the answer; Stop; the failures; the round budget; the router's quirks;
//! Jev runs and outside drivers in the transcript.

use super::*;
use copper_core::agent::ChatConfig;
use copper_core::cloud::CloudProvided;
use copper_core::intelligence::IntelligenceKeys;
use copper_mcp::IntelligenceSource;
use copper_mcp::drive::{Outcome, Status};
use std::cell::Cell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// -- the router ---------------------------------------------------------------

/// One request the router saw.
#[derive(Clone, Debug)]
struct Seen {
    authorization: String,
    body: Value,
}

/// A router that answers from a script, one `(status, body)` per request;
/// past the end it repeats the last answer.
struct Router {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Router {
    fn start(script: Vec<(u16, Value)>) -> Router {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut n = 0;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                let mut authorization = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        authorization = line["authorization:".len()..].trim().to_string();
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                log.lock().unwrap().push(Seen {
                    authorization,
                    body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                });
                let (status, answer) = script[n.min(script.len() - 1)].clone();
                n += 1;
                let text = answer.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        Router { url, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn says(text: &str) -> (u16, Value) {
    (
        200,
        json!({"model": "sonnet", "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}]}),
    )
}

fn calls(name: &str, args: Value) -> (u16, Value) {
    (
        200,
        json!({"choices": [{"finish_reason": "tool_calls", "message": {"role": "assistant", "content": "",
            "tool_calls": [{"id": "c1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]}),
    )
}

// -- the host -------------------------------------------------------------------

/// One tool call the agent made: its name and arguments.
type Called = (String, Map<String, Value>);

#[derive(Clone)]
struct Mock {
    keys: Rc<RefCell<IntelligenceSource>>,
    config: Rc<RefCell<ChatConfig>>,
    runtime: tokio::runtime::Handle,
    called: Rc<RefCell<Vec<Called>>>,
    /// How long a tool takes.
    slow: Rc<Cell<Duration>>,
    jev_stops: Rc<Cell<u32>>,
    page: Option<PageInFront>,
}

impl Mock {
    fn new(router: &Router, runtime: tokio::runtime::Handle) -> Mock {
        let local: IntelligenceKeys = serde_json::from_value(json!({
            "routerKey": "sk-test",
            "routerURL": router.url,
        }))
        .unwrap();
        Mock {
            keys: Rc::new(RefCell::new(IntelligenceSource { local, cloud: None })),
            config: Rc::default(),
            runtime,
            called: Rc::default(),
            slow: Rc::new(Cell::new(Duration::ZERO)),
            jev_stops: Rc::default(),
            page: Some(PageInFront {
                url: "https://example.com/".into(),
                title: "Example Domain".into(),
                canvas: None,
                text: Some(
                    "Example Domain\nThis domain is for use in documentation examples.".into(),
                ),
            }),
        }
    }
}

impl ChatHost for Mock {
    fn spawn(&self, task: copper_engine::LocalFuture<()>) {
        tokio::task::spawn_local(task);
    }
    fn runtime(&self) -> tokio::runtime::Handle {
        self.runtime.clone()
    }
    fn intelligence(&self) -> IntelligenceSource {
        self.keys.borrow().clone()
    }
    fn set_tier(&self, tier: Tier) {
        self.keys.borrow_mut().local.tier = Some(tier.name().into());
    }
    fn config(&self) -> ChatConfig {
        self.config.borrow().clone()
    }
    fn jev_mode(&self) -> bool {
        true
    }
    fn catalogue(&self, jev: bool) -> Vec<Value> {
        copper_mcp::catalogue::catalogue(jev, true)
    }
    fn page(&self) -> copper_engine::LocalFuture<Option<PageInFront>> {
        let page = self.page.clone();
        Box::pin(async move { page })
    }
    fn call(
        &self,
        name: &str,
        args: &Map<String, Value>,
    ) -> copper_engine::LocalFuture<ToolOutput> {
        self.called
            .borrow_mut()
            .push((name.to_string(), args.clone()));
        let slow = self.slow.get();
        let name = name.to_string();
        Box::pin(async move {
            tokio::time::sleep(slow).await;
            match name.as_str() {
                "browser_snapshot" => ToolOutput {
                    text: "### Page\n- heading \"Example Domain\" [ref=e3]".into(),
                    line: Some("Read the page".into()),
                    ..ToolOutput::default()
                },
                "browser_take_screenshot" => ToolOutput {
                    text: "[screenshot attached]".into(),
                    pictures: vec![(vec![137, 80, 78, 71], "image/png".into())],
                    ..ToolOutput::default()
                },
                "canvas_apply" => ToolOutput {
                    text: r#"{"applied":0,"errors":[{"index":0,"op":"add","error":"bad shape"}]}"#
                        .into(),
                    ..ToolOutput::default()
                },
                _ => ToolOutput {
                    text: format!("Unknown tool {name}"),
                    is_error: true,
                    ..ToolOutput::default()
                },
            }
        })
    }
    fn stop_jev(&self) {
        self.jev_stops.set(self.jev_stops.get() + 1);
    }
}

// -- running ----------------------------------------------------------------------

/// Run `test` on a current-thread runtime with a local set (the UI thread's
/// stand-in); requests go out on a second runtime, as the browser's do.
fn run<F: std::future::Future<Output = ()>>(test: impl FnOnce(tokio::runtime::Handle) -> F) {
    let net = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let ui = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&ui, test(net.handle().clone()));
}

fn host(router: &Router, net: &tokio::runtime::Handle) -> Mock {
    Mock::new(router, net.clone())
}

async fn until(chat: &Chat<Mock>, what: &str, ok: impl Fn(&Chat<Mock>) -> bool) {
    for _ in 0..500 {
        if ok(chat) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}: {:#}", chat.state());
}

fn roles(state: &Value) -> Vec<String> {
    state["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["role"].as_str().unwrap().to_string())
        .collect()
}

fn detail(item: &Value) -> Value {
    serde_json::from_str(item["detail"].as_str().unwrap()).unwrap()
}

// -- the tests ---------------------------------------------------------------------

#[test]
fn a_tool_call_then_the_answer() {
    run(|net| async move {
        let router = Router::start(vec![
            calls("browser_snapshot", json!({"reason": "read it"})),
            says("The heading is **Example Domain**."),
        ]);
        let mock = host(&router, &net);
        let chat = Chat::new(mock.clone());
        let changes = Rc::new(Cell::new(0));
        let c = changes.clone();
        chat.on_change(move || c.set(c.get() + 1));
        assert!(chat.available());
        chat.send("  What is the heading on this page?  ").unwrap();
        assert_eq!(chat.send("again"), Err(SendError::Busy));
        assert_eq!(chat.state()["busy"], true);
        assert_eq!(chat.state()["status"], "Thinking…");
        until(&chat, "the answer", |c| !c.busy()).await;
        assert!(changes.get() > 3, "every change is told");

        let state = chat.state();
        assert_eq!(roles(&state), ["user", "activity", "assistant"]);
        assert_eq!(state["available"], true);
        assert_eq!(state["model"], "sonnet");
        assert_eq!(state["models"].as_array().unwrap().len(), 3);
        assert_eq!(
            state["models"][0],
            json!({"id": "haiku", "name": "Haiku", "ready": true, "detail": "Quick"})
        );
        assert_eq!(state["pageContext"], true);
        assert_eq!(state["status"], "");
        let items = state["items"].as_array().unwrap();
        assert_eq!(items[0]["text"], "What is the heading on this page?");
        assert_eq!(items[2]["text"], "The heading is **Example Domain**.");
        assert!(
            items[1]["text"]
                .as_str()
                .unwrap()
                .starts_with("Worked for "),
            "{}",
            items[1]
        );
        assert!(items[1]["text"].as_str().unwrap().ends_with(" · 1 step"));
        let steps = detail(&items[1])["steps"].clone();
        assert_eq!(steps[0]["tool"], "browser_snapshot");
        assert_eq!(steps[0]["title"], "Read the page", "the tool's own line");
        assert_eq!(steps[0]["ok"], true);
        assert_eq!(steps[0]["running"], false);
        assert_eq!(mock.called.borrow()[0].0, "browser_snapshot");

        let seen = router.seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].authorization, "Bearer sk-test");
        let first = &seen[0].body;
        assert_eq!(first["model"], "sonnet");
        assert_eq!(first["max_tokens"], wire::OUTPUT_LIMIT);
        assert_eq!(first["tool_choice"], "auto");
        assert!(first.get("temperature").is_none());
        assert_eq!(first["messages"][0]["role"], "system");
        let question = first["messages"][1]["content"][0]["text"].as_str().unwrap();
        assert!(question.starts_with("<page>\nCurrent page: https://example.com/\nTitle: Example Domain\nVisible text (first 3000 chars):\nExample Domain"), "{question}");
        assert!(question.ends_with("</page>\n\nWhat is the heading on this page?"));
        let tools: Vec<&str> = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(tools.contains(&"browser_snapshot") && tools.contains(&"canvas_read"));
        assert!(!tools.contains(&"jev_run"), "no Jev key, no Jev tools");
        let second = &seen[1].body["messages"];
        assert_eq!(second[2]["tool_calls"][0]["id"], "c1");
        assert_eq!(second[3]["role"], "tool");
        assert_eq!(second[3]["tool_call_id"], "c1");
        assert!(
            second[3]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Example Domain")
        );
    });
}

#[test]
fn jev_tools_with_a_key_and_no_page_context_when_off() {
    run(|net| async move {
        let router = Router::start(vec![says("ok")]);
        let mock = host(&router, &net);
        mock.keys.borrow_mut().local.jev_key = Some("ts-test".into());
        mock.config.borrow_mut().page_context = Some(false);
        let chat = Chat::new(mock.clone());
        assert_eq!(chat.state()["pageContext"], false);
        chat.send("hi").unwrap();
        until(&chat, "the answer", |c| !c.busy()).await;
        let body = &router.seen()[0].body;
        assert_eq!(body["messages"][1]["content"][0]["text"], "hi");
        let names: Vec<&str> = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            &names[..4],
            ["jev_run", "jev_step", "jev_extract", "jev_observe"]
        );
    });
}

#[test]
fn stop_cuts_the_step_short_and_stops_jev() {
    run(|net| async move {
        let router = Router::start(vec![
            calls("jev_run", json!({"goal": "search for Copper"})),
            says("late"),
        ]);
        let mock = host(&router, &net);
        mock.slow.set(Duration::from_secs(30));
        let chat = Chat::new(mock.clone());
        chat.send("Use Jev").unwrap();
        until(&chat, "the jev step", |c| {
            c.items().iter().any(|i| i.running)
        })
        .await;
        // The tools report Jev's run on the pane's behalf.
        let who = copper_mcp::Who::jev(Some(&copper_mcp::Who::pane()));
        chat.drive(&DriveEvent::RunBegan {
            who,
            goal: "search for Copper".into(),
            tab: copper_core::TabId("t".into()),
        });
        chat.drive(&DriveEvent::Cycle);
        chat.drive(&DriveEvent::Phase {
            id: 1,
            kind: copper_mcp::drive::PhaseKind::Observe,
            title: "Reading the page".into(),
        });
        let state = chat.state();
        let activity = &state["items"][1];
        assert_eq!(activity["busy"], true);
        assert_eq!(
            activity["text"],
            "Jev: search for Copper · Reading the page"
        );
        assert_eq!(
            detail(activity)["steps"][0]["jev"]["cycles"][0]["text"],
            "Reading the page"
        );
        // The next move ends the last, outcome or not.
        chat.drive(&DriveEvent::Cycle);
        let cycles = detail(&chat.state()["items"][1])["steps"][0]["jev"]["cycles"].clone();
        assert_eq!(cycles[0]["running"], false);
        assert_eq!(cycles[1]["running"], true);
        assert_eq!(cycles[1]["text"], "Starting");

        chat.stop();
        assert_eq!(mock.jev_stops.get(), 1, "Jev stops before its next move");
        let state = chat.state();
        assert_eq!(state["busy"], false);
        assert_eq!(state["status"], "Stopped");
        assert_eq!(roles(&state), ["user", "activity", "activity"]);
        let step = &detail(&state["items"][1])["steps"][0];
        assert_eq!(step["ok"], false);
        assert_eq!(step["running"], false);
        assert_eq!(step["error"], "Stopped");
        assert_eq!(state["items"][2]["text"], "Stopped");
        // A second Stop on an idle agent leaves nothing behind.
        chat.stop();
        assert_eq!(chat.items().len(), 3);
        // The next question goes out with the cut-off call answered.
        mock.slow.set(Duration::ZERO);
        chat.send("again").unwrap();
        until(&chat, "the second answer", |c| !c.busy()).await;
        let messages = &router.seen()[1].body["messages"];
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["content"][0]["text"], words::CANCELLED);
        assert_eq!(chat.items().last().unwrap().text, "late");
    });
}

#[test]
fn failures_say_what_the_mac_says() {
    run(|net| async move {
        // Not set up: no request, the Mac's words.
        let router = Router::start(vec![says("never")]);
        let mock = host(&router, &net);
        mock.keys.borrow_mut().local.router_key = None;
        let chat = Chat::new(mock.clone());
        assert_eq!(chat.state()["available"], false);
        assert_eq!(chat.send("   "), Err(SendError::Empty));
        chat.send("hello").unwrap();
        let state = chat.state();
        assert_eq!(roles(&state), ["user", "error"]);
        assert_eq!(state["items"][1]["text"], words::NOT_SET_UP);
        assert!(router.seen().is_empty());

        // A router error carries its body; the question leaves the history.
        let router = Router::start(vec![(500, json!({"error": "boom"})), says("")]);
        let chat = Chat::new(host(&router, &net));
        chat.send("first").unwrap();
        until(&chat, "the error", |c| !c.busy()).await;
        let state = chat.state();
        assert_eq!(roles(&state), ["user", "error"]);
        assert_eq!(state["items"][1]["text"], r#"router 500: {"error":"boom"}"#);
        // An empty reply is an error, not silence.
        chat.send("second").unwrap();
        until(&chat, "the empty reply", |c| !c.busy()).await;
        let state = chat.state();
        assert_eq!(state["items"][3]["text"], words::EMPTY_REPLY);
        assert_eq!(state["items"][3]["role"], "error");
        let messages = router.seen()[1].body["messages"].clone();
        assert_eq!(
            messages.as_array().unwrap().len(),
            2,
            "the failed question is gone: {messages}"
        );

        // Nothing listening: the transport's words, no address.
        let mock = host(&router, &net);
        mock.keys.borrow_mut().local.router_url = Some("http://127.0.0.1:9".into());
        let chat = Chat::new(mock);
        chat.send("x").unwrap();
        until(&chat, "the failure", |c| !c.busy()).await;
        let text = chat.state()["items"][1]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.starts_with("Couldn't reach the router") && !text.contains("127.0.0.1"),
            "{text}"
        );
    });
}

#[test]
fn the_round_budget_ends_a_question() {
    run(|net| async move {
        let router = Router::start(vec![calls("browser_snapshot", json!({}))]);
        let mock = host(&router, &net);
        mock.config.borrow_mut().max_turns = Some(2);
        mock.config.borrow_mut().version = Some(2);
        let chat = Chat::new(mock.clone());
        chat.send("loop").unwrap();
        until(&chat, "the budget", |c| !c.busy()).await;
        assert_eq!(router.seen().len(), 2);
        let state = chat.state();
        assert_eq!(roles(&state), ["user", "activity", "error"]);
        assert_eq!(state["items"][2]["text"], words::stop_note(2, false));
        assert!(
            detail(&state["items"][1])["steps"]
                .as_array()
                .unwrap()
                .len()
                == 2
        );

        // Copper Cloud's limit wins.
        let cloud: CloudProvided =
            serde_json::from_value(json!({"agent": {"max_turns": 1}})).unwrap();
        mock.keys.borrow_mut().cloud = Some(cloud);
        chat.clear();
        assert!(chat.items().is_empty());
        chat.send("loop").unwrap();
        until(&chat, "the cloud's budget", |c| !c.busy()).await;
        assert_eq!(chat.items().last().unwrap().text, words::stop_note(1, true));
    });
}

#[test]
fn the_router_quirks() {
    run(|net| async move {
        let router = Router::start(vec![
            (
                400,
                json!({"error": "max_tokens: 16000 > 8192, which is the maximum allowed"}),
            ),
            (
                400,
                json!({"error": "This model does not support prompt caching (cache_control)"}),
            ),
            says("fine"),
        ]);
        let chat = Chat::new(host(&router, &net));
        chat.send("q").unwrap();
        until(&chat, "the answer", |c| !c.busy()).await;
        let seen = router.seen();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].body["max_tokens"], wire::OUTPUT_LIMIT);
        assert_eq!(seen[1].body["max_tokens"], wire::FALLBACK_LIMIT);
        assert!(seen[1].body.to_string().contains("cache_control"));
        assert!(!seen[2].body.to_string().contains("cache_control"));
        assert_eq!(chat.items().last().unwrap().text, "fine");
    });
}

#[test]
fn nudges_cutoffs_pictures_and_board_errors() {
    run(|net| async move {
        let router = Router::start(vec![
            says("Let me do that now:"),
            (
                200,
                json!({"choices": [{"finish_reason": "length", "message": {"content": "half an ans"}}]}),
            ),
            (
                200,
                json!({"choices": [{"finish_reason": "tool_calls", "message": {"tool_calls": [
                    {"id": "", "function": {"name": "browser_take_screenshot", "arguments": ""}},
                    {"id": "b", "function": {"name": "canvas_apply", "arguments": "{\"ops\":[{}]}"}},
                    {"id": "c", "function": {"name": "canvas_apply", "arguments": "{\"ops\":["}}
                ]}}]}),
            ),
            says("Done."),
        ]);
        let chat = Chat::new(host(&router, &net));
        chat.send("draw").unwrap();
        until(&chat, "the answer", |c| !c.busy()).await;
        let seen = router.seen();
        assert_eq!(seen.len(), 4);
        let second = seen[1].body["messages"].as_array().unwrap();
        assert_eq!(
            second.last().unwrap()["content"][0]["text"],
            words::ACT_NUDGE
        );
        let third = seen[2].body["messages"].as_array().unwrap();
        assert_eq!(
            third.last().unwrap()["content"][0]["text"],
            words::CUT_NUDGE
        );
        let fourth = seen[3].body["messages"].as_array().unwrap();
        let n = fourth.len();
        // The calls, their three results, then the screenshot as a picture.
        assert_eq!(fourth[n - 5]["role"], "assistant");
        let call_id = fourth[n - 5]["tool_calls"][0]["id"].as_str().unwrap();
        assert!(call_id.starts_with("call_"), "{call_id}");
        assert_eq!(
            fourth[n - 5]["tool_calls"][2]["function"]["arguments"],
            "{}",
            "broken calls are emptied"
        );
        assert_eq!(fourth[n - 4]["tool_call_id"], call_id);
        assert!(
            fourth[n - 2]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Not run: the arguments were not a valid JSON object")
        );
        assert_eq!(
            fourth[n - 1]["content"][0]["text"],
            "Screenshot from the tool call above:"
        );
        assert_eq!(
            fourth[n - 1]["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw=="
        );

        let state = chat.state();
        assert_eq!(
            roles(&state),
            ["user", "activity", "error", "activity", "assistant"],
            "{state:#}"
        );
        assert_eq!(state["items"][2]["text"], words::CUT_NOTE);
        let first = detail(&state["items"][1]);
        assert_eq!(first["steps"][0]["aside"], true);
        assert_eq!(first["steps"][0]["title"], "Let me do that now:");
        assert_eq!(
            first["steps"][1]["title"], "half an ans",
            "half an answer is narration"
        );
        let steps = detail(&state["items"][3])["steps"].clone();
        assert_eq!(steps[0]["tool"], "browser_take_screenshot");
        assert_eq!(steps[1]["ok"], false, "nothing applied is a failure");
        assert_eq!(
            steps[1]["warning"],
            "1 of 1 not applied — op 1 (add): bad shape"
        );
        assert_eq!(steps[2]["ok"], false);
        assert_eq!(
            steps[2]["error"],
            "Its arguments were not valid JSON — not run"
        );
        assert!(
            state["items"][3]["text"]
                .as_str()
                .unwrap()
                .ends_with("3 steps · 2 failed")
        );
    });
}

#[test]
fn models_and_outside_drivers() {
    run(|net| async move {
        let router = Router::start(vec![says("ok")]);
        let mock = host(&router, &net);
        let chat = Chat::new(mock.clone());
        chat.set_model("opus").unwrap();
        assert_eq!(chat.state()["model"], "opus");
        assert!(
            chat.set_model("gpt")
                .unwrap_err()
                .contains("agent.state() lists them")
        );
        chat.send("q").unwrap();
        until(&chat, "the answer", |c| !c.busy()).await;
        assert_eq!(router.seen()[0].body["model"], "opus");

        let who = copper_mcp::Who {
            key: "claude-code:1".into(),
            agent: "Claude Code".into(),
            thread: "Fix the build".into(),
            ..copper_mcp::Who::default()
        };
        chat.drive(&DriveEvent::CallBegan {
            ticket: 1,
            who: who.clone(),
            tool: "browser_click".into(),
            title: "Clicking Save".into(),
            detail: None,
            tab: None,
            behind: false,
        });
        let state = chat.state();
        let card = state["items"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(card["role"], "drive");
        assert_eq!(card["text"], "Claude Code");
        assert_eq!(card["detail"], "Fix the build");
        assert_eq!(card["busy"], true);
        chat.drive(&DriveEvent::CallEnded {
            ticket: 1,
            who: who.clone(),
            tool: "browser_click".into(),
            outcome: Outcome::default(),
            tab: None,
        });
        chat.drive(&DriveEvent::CallBegan {
            ticket: 2,
            who,
            tool: "browser_snapshot".into(),
            title: "Reading the page".into(),
            detail: None,
            tab: None,
            behind: false,
        });
        assert_eq!(
            chat.items()
                .iter()
                .filter(|i| i.kind == Kind::Drive)
                .count(),
            1,
            "one card per stretch"
        );
        // The pane's own calls are its activity, never a card.
        chat.drive(&DriveEvent::CallBegan {
            ticket: 3,
            who: copper_mcp::Who::pane(),
            tool: "browser_snapshot".into(),
            title: "Reading the page".into(),
            detail: None,
            tab: None,
            behind: false,
        });
        assert_eq!(
            chat.items()
                .iter()
                .filter(|i| i.kind == Kind::Drive)
                .count(),
            1
        );
        // A Jev run nobody in the pane asked for is a card of its own.
        chat.drive(&DriveEvent::RunBegan {
            who: copper_mcp::Who::jev(None),
            goal: "Download Copper".into(),
            tab: copper_core::TabId("t".into()),
        });
        chat.drive(&DriveEvent::Outcome(Outcome {
            operation: "CLICK".into(),
            label: "“Download”".into(),
            ..Outcome::default()
        }));
        chat.drive(&DriveEvent::RunFinished {
            status: Status::Done,
            note: "found it".into(),
        });
        let state = chat.state();
        let card = state["items"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(card["text"], "Jev");
        assert_eq!(card["detail"], "Download Copper");
        assert!(card.get("busy").is_none());
    });
}
