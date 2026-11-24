//! Tidy on Linux (Mac `Fork/Tidy.swift`, copper-browser/Copper#200): the
//! palette's Tidy Tabs and the column's "Tidy" on the seam
//! (`window/column.rs`) file the space's loose tabs into folders
//! ([`copper_shell_core::tidy`]); "Tidied N tabs into M folders · Undo"
//! follows, and Ctrl+Z undoes while it is up — otherwise the key stays the
//! page's.
//!
//! `COPPER_SELFTEST=tidy` (in a throwaway `COPPER_WORLD`, on a display) runs
//! it end to end: by site, by a model (a fake router on loopback) and past
//! the model's 8 seconds; screenshots in `COPPER_SELFTEST_OUT`.

use crate::actions::{Command, register_action, register_command};
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_shell_core::tidy::{UNDO_FOR, tidied_words};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::RefCell;

thread_local! {
    /// The pill on show, so Ctrl+Z can take it down with the Undo.
    static PILL: RefCell<Option<adw::Toast>> = const { RefCell::new(None) };
}

/// Called once from `actions::register_lanes`.
pub fn register(shell: &Shell) {
    register_action(shell, "tidy-tabs", &[], run);
    register_action(shell, "undo-tidy", &[], |s| {
        undo(s);
    });
    register_command(
        shell,
        Command::new("tidy-tabs", "Tidy Tabs", "folder-new-symbolic", run)
            .when(|s| s.tidy_candidates().len() >= 2),
    );
    keys(shell);
    selftest::maybe_start(shell);
}

/// Tidy now; the pill follows when anything was filed.
pub fn run(shell: &Shell) {
    let s = shell.clone();
    glib::spawn_future_local(async move {
        let done = s.tidy_tabs(false).await;
        if done.tabs > 0 {
            pill(&s, done.tabs, done.folders.len());
        }
    });
}

/// Undo the last Tidy, while offered, and take its pill down.
pub fn undo(shell: &Shell) -> usize {
    if let Some(t) = PILL.with(|p| p.borrow_mut().take()) {
        t.dismiss();
    }
    shell.undo_tidy()
}

fn pill(shell: &Shell, tabs: usize, folders: usize) {
    let t = adw::Toast::new(&tidied_words(tabs, folders));
    t.set_button_label(Some("Undo"));
    t.set_action_name(Some("win.undo-tidy"));
    t.set_timeout(UNDO_FOR.as_secs() as u32);
    if let Some(old) = PILL.with(|p| p.borrow_mut().replace(t.clone())) {
        old.dismiss();
    }
    shell.ui().toasts.add_toast(t);
}

/// Does this key undo a Tidy? Ctrl+Z, only while its pill is up.
pub fn takes(shell: &Shell, key: gtk::gdk::Key, mods: gtk::gdk::ModifierType) -> bool {
    use gtk::gdk::ModifierType as M;
    let mods = mods & (M::CONTROL_MASK | M::SHIFT_MASK | M::ALT_MASK | M::SUPER_MASK);
    mods == M::CONTROL_MASK
        && matches!(key, gtk::gdk::Key::z | gtk::gdk::Key::Z)
        && shell.tidy_undo_line().is_some()
}

/// Ctrl+Z before the page or a field sees it, while Undo is on offer.
fn keys(shell: &Shell) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = shell.downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(shell) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if takes(&shell, key, mods) {
            undo(&shell);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    shell.window().add_controller(keys);
}

mod selftest {
    //! `COPPER_SELFTEST=tidy`: seven sleeping tabs on three sites and an
    //! empty "GitHub" folder; Tidy by site from the palette (GitHub reused,
    //! a new Ycombinator folder, apple.com left loose), Ctrl+Z; then a
    //! model on a fake loopback router names the folders from titles and
    //! hosts only; then a router slower than 8 s, which falls back by site.

    use super::*;
    use copper_core::browser::OpenOptions;
    use copper_core::groups::TabGroup;
    use copper_core::intelligence::IntelligenceKeys;
    use copper_shell_core::agents::groups;
    use copper_shell_core::tidy::INSTRUCTIONS;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const TABS: [(&str, &str); 7] = [
        (
            "https://github.com/copper-browser/copper-linux",
            "copper-browser/copper-linux: Copper for Linux",
        ),
        (
            "https://news.ycombinator.com/item?id=1",
            "Show HN: A browser for agents",
        ),
        (
            "https://github.com/copper-browser/copper-linux/pulls",
            "Pull requests · copper-browser/copper-linux",
        ),
        (
            "https://news.ycombinator.com/item?id=2",
            "Ask HN: What are you reading?",
        ),
        ("https://www.apple.com/mac/", "Mac - Apple"),
        (
            "https://gist.github.com/someone/abc",
            "Notes on GTK overlays",
        ),
        (
            "https://news.ycombinator.com/item?id=3",
            "The Rust release notes, explained",
        ),
    ];

    pub fn maybe_start(shell: &Shell) {
        if std::env::var("COPPER_SELFTEST").as_deref() != Ok("tidy") {
            return;
        }
        let out = std::env::var("COPPER_SELFTEST_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp/copper-tidy"));
        let _ = std::fs::create_dir_all(&out);
        let weak = shell.downgrade();
        glib::spawn_future_local(async move {
            crate::app::settle(&weak, Duration::from_secs(8)).await;
            let Some(shell) = weak.upgrade() else { return };
            let run = Rc::new(Run {
                shell: shell.clone(),
                out,
                failures: RefCell::default(),
            });
            run.all().await;
            let failures = run.failures.borrow().clone();
            println!(
                "RESULT {} ({} failed)",
                if failures.is_empty() { "PASS" } else { "FAIL" },
                failures.len()
            );
            for f in &failures {
                println!("  failed: {f}");
            }
            crate::app::set_exit(if failures.is_empty() { 0 } else { 1 });
            shell.save_now();
            shell.shutdown();
            shell.app().quit();
        });
    }

    struct Run {
        shell: Shell,
        out: PathBuf,
        failures: RefCell<Vec<String>>,
    }

    /// Each row of the space: title, Saved, its folder's name.
    type Rows = Vec<(String, bool, Option<String>)>;

    impl Run {
        fn check(&self, ok: bool, what: &str) {
            println!("{} {what}", if ok { "PASS" } else { "FAIL" });
            if !ok {
                self.failures.borrow_mut().push(what.to_string());
            }
        }

        async fn pause(&self, ms: u64) {
            glib::timeout_future(Duration::from_millis(ms)).await;
        }

        async fn shot(&self, name: &str) {
            self.pause(700).await;
            let path = self.out.join(format!("{name}.png"));
            match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
                Ok(()) => println!("SHOT {}", path.display()),
                Err(e) => self.check(false, &format!("screenshot {name}: {e}")),
            }
        }

        fn rows(&self) -> Rows {
            let names = groups::names(&self.shell);
            let b = self.shell.browser();
            b.row(b.current_space_id())
                .iter()
                .map(|t| {
                    let folder = t.group.as_ref().and_then(|g| {
                        names
                            .iter()
                            .find(|(id, _)| id.eq_ignore_ascii_case(g))
                            .map(|(_, n)| n.clone())
                    });
                    (t.title.clone(), t.saved, folder)
                })
                .collect()
        }

        fn folder_of(&self, rows: &Rows, n: usize) -> Option<String> {
            rows.iter()
                .find(|r| r.0 == TABS[n].1)
                .and_then(|r| r.2.clone())
        }

        fn group(&self, name: &str) -> Option<TabGroup> {
            groups::all(&self.shell)
                .into_iter()
                .find(|g| g.name == name)
        }

        /// The seam's "Tidy", as the column draws it now: shown, and its words.
        fn tidy_button(&self) -> Option<(bool, String)> {
            fn find(w: &gtk::Widget) -> Option<gtk::Widget> {
                if w.has_css_class("tidy-seam") {
                    return Some(w.clone());
                }
                let mut c = w.first_child();
                while let Some(child) = c {
                    if let Some(hit) = find(&child) {
                        return Some(hit);
                    }
                    c = child.next_sibling();
                }
                None
            }
            let button = find(self.shell.ui().column.widget.upcast_ref())?;
            let mut words = String::new();
            let mut c = button.first_child().and_then(|b| b.first_child());
            while let Some(w) = c {
                if let Some(l) = w.downcast_ref::<gtk::Label>() {
                    words = l.label().to_string();
                }
                c = w.next_sibling();
            }
            Some((button.is_visible(), words))
        }

        fn tidy_button_shown(&self) -> bool {
            self.tidy_button().is_some_and(|(shown, _)| shown)
        }

        async fn until(&self, secs: u64, f: impl Fn() -> bool) -> bool {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(secs) {
                if f() {
                    return true;
                }
                self.pause(50).await;
            }
            f()
        }

        async fn all(self: &Rc<Self>) {
            let space = self.shell.browser().current_space_id().to_string();
            self.shell.with_browser(|b| {
                for (url, title) in TABS {
                    let id = b.open_tab(
                        url,
                        OpenOptions {
                            title: title.into(),
                            at_end: true,
                            ..Default::default()
                        },
                    );
                    // Asleep: no page, no network.
                    b.update_tab(&id, |t| t.sleeping = true);
                }
            });
            // An empty, shut "GitHub" folder in this space: Tidy reuses it.
            let mut github = TabGroup::new("GitHub", None);
            github.space = Some(space.clone());
            github.collapsed = true;
            let github_id = github.id.clone();
            groups::edit(&self.shell, |f| f.groups.push(github));
            self.pause(300).await;
            let before = self.rows();
            self.check(
                self.shell.tidy_candidates().len() == 7,
                "seven loose tabs to tidy",
            );
            self.check(self.tidy_button_shown(), "\"Tidy\" stands on the seam");
            self.shot("01-before").await;

            // 1. By site, from the palette's Tidy Tabs.
            self.check(
                crate::actions::commands(&self.shell)
                    .iter()
                    .any(|c| c.id == "tidy-tabs" && c.name == "Tidy Tabs"),
                "the palette offers Tidy Tabs",
            );
            crate::actions::run_command(&self.shell, "tidy-tabs");
            let s = self.shell.clone();
            self.check(
                self.until(3, move || s.tidy_undo_line().is_some()).await,
                "tidied by site",
            );
            let rows = self.rows();
            println!("ROWS {rows:?}");
            for n in [0, 2, 5] {
                self.check(
                    self.folder_of(&rows, n).as_deref() == Some("GitHub"),
                    &format!("{} in the existing GitHub folder", TABS[n].0),
                );
            }
            for n in [1, 3, 6] {
                self.check(
                    self.folder_of(&rows, n).as_deref() == Some("Ycombinator"),
                    &format!("{} in a new Ycombinator folder", TABS[n].0),
                );
            }
            self.check(
                self.folder_of(&rows, 4).is_none(),
                "apple.com fits nowhere and stays loose",
            );
            self.check(
                rows.iter().filter(|r| r.2.is_some()).all(|r| r.1),
                "every tidied tab is in Saved",
            );
            self.check(
                self.group("GitHub")
                    .is_some_and(|g| g.id == github_id && !g.collapsed),
                "the GitHub folder is the same one, opened",
            );
            self.check(
                self.group("Ycombinator").is_some_and(|g| g.hue.is_none()),
                "the new folder wears the space's colour",
            );
            self.check(
                self.shell.tidy_undo_line() == Some((6, 2)),
                "the pill says 6 tabs into 2 folders",
            );
            self.check(!self.tidy_button_shown(), "one loose tab: no \"Tidy\"");
            self.shot("02-tidied").await;

            // 2. Ctrl+Z while the pill is up.
            let ctrl = gtk::gdk::ModifierType::CONTROL_MASK;
            self.check(
                takes(&self.shell, gtk::gdk::Key::z, ctrl),
                "Ctrl+Z is Tidy's while its pill is up",
            );
            self.check(
                !takes(
                    &self.shell,
                    gtk::gdk::Key::z,
                    ctrl | gtk::gdk::ModifierType::SHIFT_MASK,
                ),
                "Ctrl+Shift+Z is not",
            );
            self.check(undo(&self.shell) == 6, "Undo brings six back");
            self.pause(300).await;
            self.check(self.rows() == before, "every tab back where it was");
            self.check(
                self.group("Ycombinator").is_none(),
                "the new folder is gone",
            );
            self.check(
                self.group("GitHub").is_some_and(|g| g.collapsed),
                "the reused folder is shut again",
            );
            self.check(
                !takes(&self.shell, gtk::gdk::Key::z, ctrl),
                "afterwards Ctrl+Z is the page's again",
            );
            self.shot("03-undone").await;

            // 3. A model names the folders: titles and hosts only.
            let router = Router::start();
            let _ = self.shell.store().save(&IntelligenceKeys {
                router_key: Some("selftest".into()),
                router_url: Some(format!("http://127.0.0.1:{}", router.port)),
                ..Default::default()
            });
            let done = self.shell.tidy_tabs(false).await;
            println!("MODEL {done:?}");
            self.check(done.by_model, "the model's folders were used");
            let rows = self.rows();
            for n in [0, 2] {
                self.check(
                    self.folder_of(&rows, n).as_deref() == Some("Copper Linux"),
                    &format!("{} in the model's Copper Linux", TABS[n].0),
                );
            }
            for n in [1, 3, 6] {
                self.check(
                    self.folder_of(&rows, n).as_deref() == Some("Reading"),
                    &format!("{} in the model's Reading", TABS[n].0),
                );
            }
            self.check(
                self.folder_of(&rows, 4).is_none() && self.folder_of(&rows, 5).is_none(),
                "tabs the model left out stay loose",
            );
            let asked = router.asked.lock().unwrap().clone();
            let body: Value = asked
                .first()
                .and_then(|b| serde_json::from_str(b).ok())
                .unwrap_or_default();
            let system = body["messages"][0]["content"].clone();
            let system = system
                .as_str()
                .map(str::to_string)
                .or_else(|| system[0]["text"].as_str().map(str::to_string));
            self.check(
                system.as_deref() == Some(INSTRUCTIONS),
                "the model got Tidy's instructions",
            );
            let user: Value = body["messages"][1]["content"]
                .as_str()
                .and_then(|u| serde_json::from_str(u).ok())
                .unwrap_or_default();
            println!("ASKED {user}");
            let tabs = user["tabs"].as_array().cloned().unwrap_or_default();
            self.check(
                tabs.len() == 7
                    && tabs.iter().all(|t| {
                        let mut keys: Vec<&String> = t
                            .as_object()
                            .map(|o| o.keys().collect())
                            .unwrap_or_default();
                        keys.sort();
                        keys == ["host", "i", "title"]
                    })
                    && user.as_object().is_some_and(|o| o.len() == 2)
                    && user["existing"] == json!(["GitHub"]),
                "the model saw titles, hosts and the GitHub folder only",
            );
            self.check(
                tabs.first() == Some(&json!({"host": "github.com", "i": 0, "title": TABS[0].1})),
                "a tab as the model sees it",
            );
            self.shot("04-by-model").await;
            self.shell.undo_tidy();
            self.pause(300).await;
            self.check(self.rows() == before, "Undo after the model's tidy");

            // 4. A model slower than 8 seconds: by site, on time.
            router.slow.store(true, Ordering::SeqCst);
            let start = Instant::now();
            let slot: Rc<RefCell<Option<copper_shell_core::tidy::Tidied>>> = Rc::default();
            let (s, put) = (self.shell.clone(), slot.clone());
            glib::spawn_future_local(async move {
                let done = s.tidy_tabs(false).await;
                *put.borrow_mut() = Some(done);
            });
            self.pause(1000).await;
            self.check(
                self.tidy_button() == Some((true, "Tidying…".into())),
                "the seam says \"Tidying…\" while the model thinks",
            );
            self.shot("05-tidying").await;
            let s = slot.clone();
            self.until(15, move || s.borrow().is_some()).await;
            let took = start.elapsed();
            let done = slot.borrow_mut().take().unwrap_or_default();
            println!("SLOW {done:?} in {took:?}");
            self.check(
                !done.by_model && done.tabs == 6,
                "past 8 seconds the tabs are filed by site",
            );
            self.check(
                took >= Duration::from_millis(7_500) && took < Duration::from_secs(11),
                "the model got its 8 seconds and no more",
            );
            self.shell.undo_tidy();
            let _ =
                std::fs::remove_file(self.shell.store().path(copper_core::DataFile::Intelligence));
        }
    }

    /// A chat-completions router on loopback: answers two folders, or
    /// (`slow`) nothing for twelve seconds. Keeps every request body.
    struct Router {
        port: u16,
        asked: Arc<Mutex<Vec<String>>>,
        slow: Arc<AtomicBool>,
    }

    impl Router {
        fn start() -> Router {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake router");
            let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
            let asked = Arc::new(Mutex::new(Vec::new()));
            let slow = Arc::new(AtomicBool::new(false));
            let (a, s) = (asked.clone(), slow.clone());
            std::thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    let (a, s) = (a.clone(), s.clone());
                    std::thread::spawn(move || answer(conn, &a, &s));
                }
            });
            Router { port, asked, slow }
        }
    }

    fn answer(conn: std::net::TcpStream, asked: &Mutex<Vec<String>>, slow: &AtomicBool) {
        let mut reader = BufReader::new(&conn);
        let mut length = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        asked
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(&body).into_owned());
        if slow.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(12));
        }
        let folders = r#"Here you go: {"folders":[{"name":"Copper Linux","tabs":[0,2]},{"name":"Reading","tabs":[1,3,6]}]}"#;
        let reply = json!({
            "model": "fake",
            "choices": [{ "message": { "role": "assistant", "content": folders } }],
        })
        .to_string();
        let mut conn = conn;
        let _ = write!(
            conn,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
    }
}
