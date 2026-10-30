//! QA hooks for this lane (debug builds only):
//!
//! * `COPPER_AGENTS_WATCH=<file>` — while the app runs, append one JSON
//!   line to `<file>` whenever the user's side changes: the selected tab,
//!   the focused widget, whether the window is active, plus what agents
//!   hold and touch. An agent working in its own tab must not change the
//!   first three.
//! * `COPPER_SELFTEST=agents` (out dir `COPPER_SELFTEST_OUT`, default
//!   `/tmp/copper-agents-selftest`) — Settings › Agents (light, dark, a busy
//!   port), Add key opening Settings › Intelligence (Check, the Jev key
//!   saved), the switch starting and stopping the listener, token rotation,
//!   and the capsule / tab square / Stop; screenshots, PASS/FAIL lines,
//!   then quit (exit 1 on a failure).

use crate::shell::Shell;

pub fn start(shell: &Shell) {
    #[cfg(debug_assertions)]
    {
        if let Ok(path) = std::env::var("COPPER_AGENTS_WATCH")
            && !path.trim().is_empty()
        {
            debug::watch(shell, std::path::PathBuf::from(path));
        }
        if std::env::var("COPPER_SELFTEST").as_deref() == Ok("agents") {
            let out = std::env::var("COPPER_SELFTEST_OUT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/copper-agents-selftest"));
            debug::selftest(shell, out);
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = shell;
}

#[cfg(debug_assertions)]
mod debug {
    use super::super::agents;
    use crate::shell::{GtkShell, OpenTab, Shell};
    use copper_core::settings::Look;
    use copper_engine::{Backstage, BackstageMode, Engine as _};
    use copper_mcp::Who;
    use copper_mcp::drive::DriveEvent;
    use gtk4 as gtk;
    use gtk4::prelude::*;
    use libadwaita::prelude::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    pub fn watch(shell: &Shell, path: PathBuf) {
        let weak = shell.downgrade();
        let started = Instant::now();
        let last: RefCell<String> = RefCell::default();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let Some(s) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let focus = GtkWindowExt::focus(s.window())
                .map(|w| format!("{}@{:p}", w.type_().name(), w.as_ptr()))
                .unwrap_or_default();
            let (held, hands) = match agents() {
                Some(a) => (
                    a.stage
                        .held()
                        .iter()
                        .map(|t| t.0.clone())
                        .collect::<Vec<_>>(),
                    a.marks
                        .snapshot()
                        .into_iter()
                        .map(|(t, who, busy)| json!([t.0, who, busy]))
                        .collect::<Vec<_>>(),
                ),
                None => (Vec::new(), Vec::new()),
            };
            let active = s.active_id().map(|t| t.0).unwrap_or_default();
            let pill =
                agents().is_some_and(|a| s.active_id().is_some_and(|id| a.marks.pill_visible(&id)));
            let status = s.engine().backstage().status();
            let mode = match status.mode {
                BackstageMode::Idle => "idle",
                BackstageMode::InWindow => "in-window",
                BackstageMode::Degraded => "degraded",
                _ => "other",
            };
            let state = json!({
                "active": active,
                "focus": focus,
                "windowActive": s.window().is_active(),
                "held": held,
                "hands": hands,
                "pillOnActive": pill,
                "backstage": mode,
                "hosted": status.hosted,
            });
            let line = state.to_string();
            if *last.borrow() != line {
                *last.borrow_mut() = line;
                let mut row = state;
                row["t"] = json!(started.elapsed().as_millis() as u64);
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    let _ = writeln!(f, "{row}");
                }
            }
            glib::ControlFlow::Continue
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
                self.failures.borrow_mut().push(what.to_string());
            }
        }

        async fn shot(&self, name: &str) {
            glib::timeout_future(Duration::from_millis(700)).await;
            let path = self.out.join(format!("{name}.png"));
            match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
                Ok(()) => println!("SHOT {}", path.display()),
                Err(e) => self.check(false, &format!("screenshot {name}: {e}")),
            }
        }
    }

    fn listening(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(500),
        )
        .is_ok()
    }

    fn scrollers(w: &gtk::Widget, out: &mut Vec<gtk::ScrolledWindow>) {
        if let Ok(sw) = w.clone().downcast::<gtk::ScrolledWindow>()
            && sw.is_mapped()
        {
            out.push(sw);
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            scrollers(&child, out);
            c = child.next_sibling();
        }
    }

    fn buttons(w: &gtk::Widget, label: &str, out: &mut Vec<gtk::Button>) {
        if let Ok(b) = w.clone().downcast::<gtk::Button>()
            && b.label().as_deref() == Some(label)
        {
            out.push(b);
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            buttons(&child, label, out);
            c = child.next_sibling();
        }
    }

    /// Text boxes whose placeholder is `hint`.
    fn entries(w: &gtk::Widget, hint: &str, out: &mut Vec<gtk::Editable>) {
        let shown = w.find_property("placeholder-text").and_then(|_| {
            w.property_value("placeholder-text")
                .get::<Option<String>>()
                .ok()
                .flatten()
        });
        if shown.as_deref() == Some(hint)
            && let Ok(e) = w.clone().downcast::<gtk::Editable>()
        {
            out.push(e);
        }
        let mut c = w.first_child();
        while let Some(child) = c {
            entries(&child, hint, out);
            c = child.next_sibling();
        }
    }

    fn scroll_to_end(w: &gtk::Widget) {
        let mut all = Vec::new();
        scrollers(w, &mut all);
        for sw in all {
            let adj = sw.vadjustment();
            adj.set_value(adj.upper() - adj.page_size());
        }
    }

    fn scroll_to_start(w: &gtk::Widget) {
        let mut all = Vec::new();
        scrollers(w, &mut all);
        for sw in all {
            sw.vadjustment().set_value(0.0);
        }
    }

    const PAGE: &str = "data:text/html,<title>Field notes</title><body style='font:17px/1.5 sans-serif;margin:12vh auto;max-width:34em;color:%23222'><h1 style='font-weight:600'>Field notes</h1><p>The user's own tab. An agent working in a tab of its own never changes it.</p><p><input placeholder='Search notes' style='font:inherit;padding:6px 10px;width:20em'></p></body>";

    pub fn selftest(shell: &Shell, out: PathBuf) {
        let shell = shell.clone();
        glib::spawn_future_local(async move {
            crate::app::settle(&shell.downgrade(), Duration::from_secs(5)).await;
            let _ = std::fs::create_dir_all(&out);
            let run = Run {
                shell: shell.clone(),
                out,
                failures: RefCell::default(),
            };
            body(&run).await;
            let failures = run.failures.borrow().clone();
            if failures.is_empty() {
                println!("selftest agents: all passed");
            } else {
                println!("selftest agents: {} failed", failures.len());
                for f in &failures {
                    println!("  - {f}");
                }
                crate::app::set_exit(1);
            }
            shell.shutdown();
            shell.app().quit();
        });
    }

    async fn body(run: &Run) {
        let s = &run.shell;
        let Some(a) = agents() else {
            run.check(false, "agents state exists");
            return;
        };
        let tab = s.open(
            PAGE,
            OpenTab {
                foreground: true,
                ..Default::default()
            },
        );
        crate::app::settle(&s.downgrade(), Duration::from_secs(8)).await;

        // The switch starts and stops the listener.
        let port = a.port();
        a.set_enabled(true);
        run.check(
            a.running() && listening(port),
            "the switch on: listening on the port",
        );
        let d = super::super::settings::present(s);
        run.shot("agents-settings-light").await;
        scroll_to_end(d.upcast_ref());
        run.shot("agents-settings-light-end").await;
        s.set_look(Look::Dark);
        run.shot("agents-settings-dark-end").await;
        scroll_to_start(d.upcast_ref());
        run.shot("agents-settings-dark").await;
        s.set_look(Look::Light);
        d.close();
        a.set_enabled(false);
        glib::timeout_future(Duration::from_millis(300)).await;
        run.check(
            !a.running() && !listening(port),
            "the switch off: nothing listens",
        );

        // A busy port says so.
        let busy = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let taken = busy.local_addr().map(|a| a.port()).unwrap_or(0);
        a.set_port(taken);
        a.set_enabled(true);
        let trouble = a.trouble().unwrap_or_default();
        run.check(
            !a.running() && trouble.contains("another program is using it"),
            "a busy port: not listening, and the status says why",
        );
        let d = super::super::settings::present(s);
        run.shot("agents-settings-busy").await;
        d.close();
        drop(busy);
        a.set_port(port);
        run.check(a.running() && listening(port), "back on the usual port");

        // Copy config puts a working client config on the clipboard.
        let http: serde_json::Value = serde_json::from_str(&a.http_config()).unwrap_or_default();
        let server = &http["mcpServers"]["copper"];
        run.check(
            server["url"] == a.endpoint().as_str()
                && server["headers"]["Authorization"] == format!("Bearer {}", a.token()).as_str(),
            "Copy config (HTTP): the endpoint and this token",
        );
        let stdio: serde_json::Value = serde_json::from_str(&a.stdio_config()).unwrap_or_default();
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        run.check(
            stdio["mcpServers"]["copper"]["command"] == exe.as_str()
                && stdio["mcpServers"]["copper"]["args"][0] == "--mcp-stdio",
            "Copy config (stdio): this binary with --mcp-stdio",
        );
        let d = super::super::settings::present(s);
        glib::timeout_future(Duration::from_millis(300)).await;
        let mut copies = Vec::new();
        buttons(d.upcast_ref(), "Copy config", &mut copies);
        buttons(d.upcast_ref(), "Copy stdio config", &mut copies);
        let mut pasted = Vec::new();
        for b in &copies {
            b.emit_clicked();
            pasted.push(
                s.window()
                    .clipboard()
                    .read_text_future()
                    .await
                    .ok()
                    .flatten()
                    .map(|t| t.to_string())
                    .unwrap_or_default(),
            );
        }
        run.check(
            pasted == [a.http_config(), a.stdio_config()],
            "Copy config and Copy stdio config put the HTTP and stdio configs on the clipboard",
        );
        d.close();

        // Jev on with no key: Agents points to Intelligence, whose Check
        // says where a key comes from without asking anyone; a key typed
        // there lands in intelligence.json (0600) and the pointer goes.
        a.set_jev(true);
        let d = super::super::settings::present(s);
        glib::timeout_future(Duration::from_millis(300)).await;
        scroll_to_end(d.upcast_ref());
        run.shot("agents-settings-jev").await;
        let mut add = Vec::new();
        buttons(d.upcast_ref(), "Add key", &mut add);
        if let Some(b) = add.first() {
            b.emit_clicked();
        }
        run.check(
            d.visible_page_name().as_deref() == Some("intelligence"),
            "Agents › Jev key › Add key opens Intelligence",
        );
        let mut test = Vec::new();
        buttons(d.upcast_ref(), "Test", &mut test);
        if let Some(b) = test.first() {
            b.emit_clicked();
        }
        glib::timeout_future(Duration::from_millis(500)).await;
        let mut again = Vec::new();
        buttons(d.upcast_ref(), "Test again", &mut again);
        run.check(
            !again.is_empty(),
            "Intelligence › Check answers with no keys",
        );
        run.shot("intelligence-settings-light").await;
        s.set_look(Look::Dark);
        run.shot("intelligence-settings-dark").await;
        s.set_look(Look::Light);
        let mut fields = Vec::new();
        entries(d.upcast_ref(), "ts-…", &mut fields);
        if let Some(e) = fields.first() {
            e.set_text("ts-selftest");
        }
        let typed = s
            .store()
            .load::<copper_core::intelligence::IntelligenceKeys>()
            .ok()
            .and_then(|l| l.value.jev_key);
        let mode = std::fs::metadata(s.store().path(copper_core::DataFile::Intelligence))
            .map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777)
            .unwrap_or(0);
        run.check(
            typed.as_deref() == Some("ts-selftest") && mode == 0o600,
            "the Jev key typed on Intelligence is in intelligence.json, 0600",
        );
        d.close();
        let d = super::super::settings::present(s);
        glib::timeout_future(Duration::from_millis(300)).await;
        let mut add = Vec::new();
        buttons(d.upcast_ref(), "Add key", &mut add);
        run.check(
            add.iter().all(|b| !b.is_mapped()),
            "with a key, Agents no longer points to Intelligence",
        );
        d.close();
        crate::features::settings::intelligence::edit(s, |k| k.jev_key = None);
        a.set_jev(false);

        // A new token takes effect in the file and the listener.
        let before = a.token();
        a.rotate_token();
        let after = a.token();
        let on_disk = s
            .store()
            .load::<copper_core::agent::AgentFile>()
            .ok()
            .and_then(|l| l.value.token)
            .unwrap_or_default();
        run.check(
            before != after && after.len() >= 32 && on_disk == after,
            "Rotate token: a new token, written to agent.json",
        );
        let mode = std::fs::metadata(s.store().path(copper_core::DataFile::Agent))
            .map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o777)
            .unwrap_or(0);
        run.check(mode == 0o600, "agent.json is 0600");

        // Holding a background tab backstage, and letting it go (what the
        // five-minute linger does): its view leaves its hidden frame for the
        // ghost host, then comes back.
        let bg = s.open(
            "data:text/html,<title>Behind</title><p>An agent's tab",
            OpenTab::default(),
        );
        glib::timeout_future(Duration::from_millis(800)).await;
        let fresh = a.stage.prepare(s, &bg);
        let slot = s.slot(&bg);
        let backstage = s.engine().backstage();
        let hung = slot.as_ref().is_some_and(|sl| {
            let view = sl.page.widget();
            view.parent()
                .is_some_and(|p| p != *sl.frame.upcast_ref::<gtk::Widget>())
                && view.is_mapped()
                && backstage.holds(&sl.page)
        });
        run.check(
            fresh && hung && s.active_id().as_ref() == Some(&tab),
            "a background tab is held backstage (mapped, out of its frame), the user's tab unchanged",
        );
        a.stage.release(s, &bg);
        let back = slot.as_ref().is_some_and(|sl| {
            sl.page.widget().parent().as_ref() == Some(sl.frame.upcast_ref::<gtk::Widget>())
                && !backstage.holds(&sl.page)
        });
        run.check(
            back && a.stage.held().is_empty(),
            "released: back in its frame, no longer held",
        );
        s.close(&bg);

        // The capsule and the tab's square while an agent drives the tab.
        let who = Who {
            key: "qa-session".into(),
            agent: "Claude Code".into(),
            thread: "copper-linux".into(),
            seed: "qa-session".into(),
            ..Who::default()
        };
        a.marks.event(&DriveEvent::CallBegan {
            ticket: 9001,
            who: who.clone(),
            tool: "browser_click".into(),
            title: "Clicking Search notes".into(),
            detail: None,
            tab: Some(tab.clone()),
            behind: false,
        });
        run.check(
            a.marks.pill_visible(&tab),
            "the capsule shows over the page being driven",
        );
        let badge = gtk::Image::new();
        crate::agents::paint_badge(&tab, &badge);
        run.check(
            badge.has_css_class("agent-mark") && badge.paintable().is_some(),
            "the tab gets the agent's square",
        );
        run.shot("agents-pill-light").await;
        s.set_look(Look::Dark);
        run.shot("agents-pill-dark").await;
        s.set_look(Look::Light);
        a.marks.event(&DriveEvent::CallEnded {
            ticket: 9001,
            who: who.clone(),
            tool: "browser_click".into(),
            outcome: Default::default(),
            tab: Some(tab.clone()),
        });
        glib::timeout_future(Duration::from_millis(4300)).await;
        run.check(
            !a.marks.pill_visible(&tab),
            "the capsule goes a few seconds after the call",
        );
        let badge = gtk::Image::new();
        crate::agents::paint_badge(&tab, &badge);
        run.check(
            badge.has_css_class("agent-mark"),
            "the square stays for half a minute",
        );

        // Stop.
        a.marks.event(&DriveEvent::CallBegan {
            ticket: 9002,
            who: who.clone(),
            tool: "browser_type".into(),
            title: "Typing into Search notes".into(),
            detail: None,
            tab: Some(tab.clone()),
            behind: false,
        });
        a.marks.stop_all();
        run.shot("agents-pill-stopped").await;
        run.check(
            a.marks.pill_visible(&tab),
            "Stop: the capsule says it stopped",
        );
        a.set_enabled(false);
        let _ = s.window().is_active();
    }
}
