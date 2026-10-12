//! A tiny WebKitGTK harness: one window with one `WebView` on the GTK main
//! thread, a local HTTP server for `tests/pages`, every Copper message
//! handler registered and recorded, and helpers to run a [`Call`].

pub mod server;

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

use copper_pagejs::{Call, Frames, Injection, UserScript, messages};
use gtk4::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::Value;
use webkit6::prelude::*;
use webkit6::{
    LoadEvent, UserContentInjectedFrames, UserContentManager, UserScriptInjectionTime, WebView,
};

pub type Res = Result<(), String>;
pub type Case = for<'a> fn(&'a H) -> Pin<Box<dyn Future<Output = Res> + 'a>>;

/// Shorthand for building a case list entry from an `async fn(&H) -> Res`.
macro_rules! case {
    ($f:ident) => {{
        fn wrapper(
            h: &crate::support::H,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::support::Res> + '_>>
        {
            Box::pin($f(h))
        }
        (stringify!($f), wrapper as crate::support::Case)
    }};
}

/// Fail a case with a formatted message unless the condition holds.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)+) => {{
        let holds: bool = $cond;
        if !holds {
            return Err(format!($($arg)+));
        }
    }};
}

/// Fail unless two values are equal, showing both.
macro_rules! ensure_eq {
    ($left:expr, $right:expr $(,)?) => {{
        let (l, r) = (&$left, &$right);
        if l != r {
            return Err(format!(
                "{} != {}\n--- left\n{:?}\n--- right\n{:?}",
                stringify!($left),
                stringify!($right),
                l,
                r
            ));
        }
    }};
}

pub struct H {
    pub base: String,
    pub view: WebView,
    pub ucm: UserContentManager,
    messages: Rc<RefCell<Vec<(String, Value)>>>,
}

impl H {
    pub fn new(base: String) -> H {
        let ucm = UserContentManager::new();
        let messages: Rc<RefCell<Vec<(String, Value)>>> = Rc::default();
        for (name, world) in messages::HANDLERS {
            assert!(
                ucm.register_script_message_handler(name, world),
                "register {name}"
            );
            let sink = messages.clone();
            let label = name.to_string();
            ucm.connect_script_message_received(Some(name), move |_, value| {
                let json = value
                    .to_json(0)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "null".into());
                let parsed = serde_json::from_str(&json).unwrap_or(Value::String(json));
                sink.borrow_mut().push((label.clone(), parsed));
            });
        }
        let view = WebView::builder().user_content_manager(&ucm).build();
        view.set_vexpand(true);
        view.set_hexpand(true);
        let window = gtk4::Window::builder()
            .title("copper-pagejs tests")
            .default_width(1024)
            .default_height(768)
            .child(&view)
            .build();
        window.present();
        H {
            base,
            view,
            ucm,
            messages,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// Load a page from `tests/pages` and wait for the load to finish.
    pub async fn load(&self, path: &str) -> Res {
        let url = self.url(path);
        let done = Rc::new(RefCell::new(None::<Result<(), String>>));
        let d = done.clone();
        let h1 = self.view.connect_load_changed(move |_, ev| {
            if ev == LoadEvent::Finished && d.borrow().is_none() {
                *d.borrow_mut() = Some(Ok(()));
            }
        });
        let d = done.clone();
        let h2 = self.view.connect_load_failed(move |_, _, uri, err| {
            *d.borrow_mut() = Some(Err(format!("{uri}: {err}")));
            false
        });
        self.view.load_uri(&url);
        let ok = wait(Duration::from_secs(15), || done.borrow().is_some()).await;
        self.view.disconnect(h1);
        self.view.disconnect(h2);
        if !ok {
            return Err(format!("{url} did not finish loading"));
        }
        done.take().unwrap()?;
        // A beat for layout and the page's own load handlers.
        sleep(150).await;
        Ok(())
    }

    /// Run a call the way the engine will: as a `call_async_javascript_function` body.
    pub async fn run<T: DeserializeOwned>(&self, call: &Call<T>) -> Result<T, String> {
        let json = self.run_json(call).await?;
        call.decode(json.as_deref()).map_err(|e| e.to_string())
    }

    /// The raw JSON of a call's result (`None` for `undefined`).
    pub async fn run_json<T>(&self, call: &Call<T>) -> Result<Option<String>, String> {
        let value = self
            .view
            .call_async_javascript_function_future(
                &call.function_body(),
                None,
                call.world().name(),
                None,
            )
            .await
            .map_err(|e| format!("{}: {e}", call.script().file()))?;
        if value.is_undefined() {
            return Ok(None);
        }
        Ok(value.to_json(0).map(|s| s.to_string()))
    }

    /// Also check the `evaluate_javascript` path for synchronous calls.
    pub async fn run_expression<T: DeserializeOwned>(&self, call: &Call<T>) -> Result<T, String> {
        let value = self
            .view
            .evaluate_javascript_future(&call.expression(), call.world().name(), None)
            .await
            .map_err(|e| format!("{}: {e}", call.script().file()))?;
        let json = if value.is_undefined() {
            None
        } else {
            value.to_json(0).map(|s| s.to_string())
        };
        call.decode(json.as_deref()).map_err(|e| e.to_string())
    }

    /// Evaluate ad-hoc test JavaScript (an expression) and parse its JSON.
    pub async fn js(&self, expression: &str) -> Result<Value, String> {
        let value = self
            .view
            .evaluate_javascript_future(expression, None, None)
            .await
            .map_err(|e| format!("js {expression:?}: {e}"))?;
        if value.is_undefined() {
            return Ok(Value::Null);
        }
        let json = value.to_json(0).map(|s| s.to_string()).unwrap_or_default();
        serde_json::from_str(&json).map_err(|e| format!("js {expression:?} gave {json:?}: {e}"))
    }

    pub async fn js_str(&self, expression: &str) -> Result<String, String> {
        match self.js(expression).await? {
            Value::String(s) => Ok(s),
            other => Err(format!("{expression} gave {other}, not a string")),
        }
    }

    pub fn add_user_script(&self, us: &UserScript) {
        let frames = match us.frames {
            Frames::MainFrame => UserContentInjectedFrames::TopFrame,
            Frames::AllFrames => UserContentInjectedFrames::AllFrames,
        };
        let time = match us.injection {
            Injection::DocumentStart => UserScriptInjectionTime::Start,
            Injection::DocumentEnd => UserScriptInjectionTime::End,
        };
        let script = match us.world.name() {
            None => webkit6::UserScript::new(&us.source, frames, time, &[], &[]),
            Some(world) => {
                webkit6::UserScript::for_world(&us.source, frames, time, world, &[], &[])
            }
        };
        self.ucm.add_script(&script);
    }

    pub fn clear_user_scripts(&self) {
        self.ucm.remove_all_scripts();
    }

    pub fn clear_messages(&self) {
        self.messages.borrow_mut().clear();
    }

    /// Messages received on one handler, oldest first.
    pub fn messages(&self, handler: &str) -> Vec<Value> {
        self.messages
            .borrow()
            .iter()
            .filter(|(name, _)| name == handler)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// Wait (≤ 5 s) for a message on `handler` matching `pred`.
    pub async fn message(
        &self,
        handler: &str,
        pred: impl Fn(&Value) -> bool,
    ) -> Result<Value, String> {
        let found = |h: &H| h.messages(handler).into_iter().find(|v| pred(v));
        let ok = wait(Duration::from_secs(5), || found(self).is_some()).await;
        if !ok {
            return Err(format!(
                "no matching {handler} message; got {:?}",
                self.messages(handler)
            ));
        }
        Ok(found(self).unwrap())
    }
}

pub async fn sleep(ms: u64) {
    glib::timeout_future(Duration::from_millis(ms)).await;
}

/// Poll `cond` on the main loop until it holds or `limit` passes.
pub async fn wait(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while !cond() {
        if start.elapsed() > limit {
            return false;
        }
        sleep(20).await;
    }
    true
}

/// Run every case whose name contains one of the positional arguments (all
/// when none are given; `--flags` from `cargo test` are ignored), print one
/// line each, and exit non-zero on any failure.
pub fn main(cases: &[(&'static str, Case)]) {
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    if std::env::args().any(|a| a == "--list") {
        for (name, _) in cases {
            println!("{name}: test");
        }
        return;
    }
    let srv = server::start().expect("start the page server");
    gtk4::init().expect("gtk init (is WAYLAND_DISPLAY/DISPLAY set?)");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(600));
        eprintln!("webview tests: watchdog fired after 600 s");
        std::process::exit(3);
    });
    let h = H::new(srv.base.clone());
    let selected: Vec<(&'static str, Case)> = cases
        .iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
        .copied()
        .collect();
    let main_loop = glib::MainLoop::new(None, false);
    glib::MainContext::default().spawn_local(async move {
        let mut failed = Vec::new();
        println!("\nrunning {} webview tests", selected.len());
        for (name, case) in &selected {
            h.clear_user_scripts();
            h.clear_messages();
            let started = Instant::now();
            let result = match glib::future_with_timeout(Duration::from_secs(45), case(&h)).await {
                Ok(r) => r,
                Err(_) => Err("timed out after 45 s".into()),
            };
            let ms = started.elapsed().as_millis();
            match result {
                Ok(()) => println!("test {name} ... ok ({ms} ms)"),
                Err(why) => {
                    println!("test {name} ... FAILED ({ms} ms)\n{why}\n");
                    failed.push(*name);
                }
            }
        }
        if failed.is_empty() {
            println!("\nwebview test result: ok. {} passed", selected.len());
            std::process::exit(0);
        }
        println!(
            "\nwebview test result: FAILED. {} passed; {} failed: {}",
            selected.len() - failed.len(),
            failed.len(),
            failed.join(", ")
        );
        std::process::exit(1);
    });
    main_loop.run();
}
