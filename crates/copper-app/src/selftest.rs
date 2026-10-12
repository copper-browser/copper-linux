//! `copper --selftest-ui <phase> [--out DIR]` (debug builds): a scripted run
//! of the real window that checks what it can and saves screenshots for a
//! human to look at. Run each phase in its own `COPPER_WORLD`:
//!
//! * `fresh` — an empty world: open tabs from a local server, type in the
//!   field and the card, pin, keep, make spaces, go private, find, crash a
//!   page, crowd the row with 40 tabs at 800 px; light and dark, column and
//!   row. Writes `expect.json` (what a relaunch must show) and quits.
//! * `restore` — the same world again: the session is back (order, pins,
//!   kept, spaces, selection), only the selected tab has a page, private
//!   tabs are gone; selecting another tab wakes it.
//! * `mac` — a world holding the Mac `session.json` fixture: it opens.
//!
//! Headless weston has no seat, so input is driven through the shell and
//! the widgets' own signals (`insert-at-cursor` for typing).

use crate::app::{set_exit, settle};
use crate::control::{TabInfo, tab_infos};
use crate::palette::Mode;
use crate::shell::{OpenTab, Shell};
use copper_core::settings::{Look, TabsPosition};
use copper_core::TabId;
use copper_engine::PageView;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita::prelude::AdwDialogExt;
use std::cell::RefCell;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

struct Run {
    shell: Shell,
    out: PathBuf,
    port: u16,
    failures: RefCell<Vec<String>>,
}

impl Run {
    fn check(&self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.borrow_mut().push(what.to_string());
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    async fn shot(&self, name: &str) {
        settle(&self.shell.downgrade(), Duration::from_secs(8)).await;
        let path = self.out.join(format!("{name}.png"));
        match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
            Ok(()) => println!("SHOT {}", path.display()),
            Err(e) => self.check(false, &format!("screenshot {name}: {e}")),
        }
    }

    async fn wait_loaded(&self, id: &TabId) {
        let start = Instant::now();
        loop {
            glib::timeout_future(Duration::from_millis(60)).await;
            if let Some(slot) = self.shell.slot(id)
                && !slot.loading.get()
                && !slot.page.is_loading()
                && slot.page.url().is_some()
            {
                return;
            }
            if start.elapsed() > Duration::from_secs(10) {
                println!("note: {id} still loading after 10 s");
                return;
            }
        }
    }

    fn open(&self, path: &str, fg: bool) -> TabId {
        self.shell.open(
            &self.url(path),
            OpenTab {
                foreground: fg,
                ..Default::default()
            },
        )
    }

    fn infos(&self) -> Vec<TabInfo> {
        tab_infos(&self.shell)
    }
}

/// Start `phase` once the window is up.
pub fn start(shell: &Shell, phase: String, out: PathBuf) {
    let shell = shell.clone();
    // Serve first: a restored tab loads as soon as the window is up.
    let icons: Icons = Arc::default();
    let port = serve(icons.clone());
    glib::spawn_future_local(async move {
        settle(&shell.downgrade(), Duration::from_secs(5)).await;
        let drawn: Vec<Vec<u8>> = PAGES
            .iter()
            .map(|p| icon_png(&shell, p.hue, p.letter))
            .collect();
        if let Ok(mut i) = icons.write() {
            *i = drawn;
        }
        let run = Rc::new(Run {
            shell: shell.clone(),
            out: out.clone(),
            port,
            failures: RefCell::default(),
        });
        let _ = std::fs::create_dir_all(&out);
        match phase.as_str() {
            "fresh" => fresh(&run).await,
            "restore" => restore(&run).await,
            "mac" => mac(&run).await,
            other => run.check(false, &format!("unknown phase {other}")),
        }
        let failures = run.failures.borrow().clone();
        if failures.is_empty() {
            println!("selftest {phase}: all passed");
        } else {
            println!("selftest {phase}: {} failed", failures.len());
            for f in &failures {
                println!("  - {f}");
            }
            set_exit(1);
        }
        shell.shutdown();
        shell.app().quit();
    });
}

async fn type_into(text: &gtk::Text, s: &str) {
    for ch in s.chars() {
        // A key replaces the selection (an offered completion), as typing does.
        text.delete_selection();
        text.emit_by_name::<()>("insert-at-cursor", &[&ch.to_string()]);
        glib::timeout_future(Duration::from_millis(15)).await;
    }
}

async fn fresh(run: &Run) {
    let s = &run.shell;
    s.set_look(Look::Light);
    s.set_tabs_position(TabsPosition::Left);
    run.check(s.browser().all_tabs().count() == 0, "a fresh world has no tabs");
    run.check(s.ui().card.is_open(), "an empty space opens the card");
    run.shot("00-first-run").await;
    s.ui().card.close(s);

    // Home: three pins, two kept, four of today's.
    let studio = run.open("/studio", true);
    run.wait_loaded(&studio).await;
    let mail = run.open("/mail", false);
    let arena = run.open("/arena", false);
    let docs = run.open("/docs", false);
    let notes = run.open("/notes", false);
    let rust = run.open("/rust", false);
    let long = run.open("/long", false);
    let weather = run.open("/weather", false);
    for id in [&mail, &arena, &docs, &notes, &rust, &long, &weather] {
        s.select(id);
        run.wait_loaded(id).await;
    }
    for id in [&studio, &mail, &arena] {
        s.pin(id);
    }
    s.set_saved(&docs, true);
    s.set_saved(&notes, true);
    let infos = run.infos();
    run.check(infos.iter().filter(|t| t.pinned).count() == 3, "three pins");
    run.check(infos.iter().filter(|t| t.saved && !t.pinned).count() == 2, "two kept tabs");
    run.check(
        s.browser().tab(&long).is_some_and(|t| t.title.starts_with("A very long title")),
        "titles come from the page",
    );
    run.check(s.favicons().borrow().contains_key(&notes), "favicons come from the page");

    // Reorder by dropping: weather before rust.
    s.drop_tab(&weather, &rust, false);
    let order: Vec<TabId> = s.visible_ids();
    let pw = order.iter().position(|i| *i == weather);
    let pr = order.iter().position(|i| *i == rust);
    run.check(pw.is_some() && pw < pr, "a dropped tab lands before its target");

    // Close and reopen.
    s.select(&rust);
    s.close(&rust);
    run.check(s.browser().tab(&rust).is_none(), "Ctrl+W closes");
    let back = s.reopen();
    run.check(
        back.as_ref().and_then(|id| s.browser().tab(id).map(|t| t.url.ends_with("/rust"))) == Some(true),
        "Ctrl+Shift+T brings it back",
    );
    let rust = back.unwrap_or(rust);
    run.wait_loaded(&rust).await;

    // Ctrl+Tab walks the visible list.
    s.select(&notes);
    let before = s.active_id();
    s.cycle_tab(true);
    s.cycle_tab(false);
    run.check(s.active_id() == before, "Ctrl+Tab then Ctrl+Shift+Tab come back");
    s.select_nth(Some(0));
    run.check(s.active_id().as_ref() == Some(&studio), "Ctrl+1 is the first pin");
    s.select(&notes);
    run.wait_loaded(&notes).await;

    // Spaces.
    let work = s.add_space("Work");
    s.set_space_colour(&work, crate::spaces::colour::SpaceColour::Teal);
    let w1 = run.open("/tracker", true);
    let w2 = run.open("/design", false);
    run.open("/standup", false);
    run.wait_loaded(&w1).await;
    let reading = s.add_space("Reading");
    s.set_space_colour(&reading, crate::spaces::colour::SpaceColour::Indigo);
    s.set_space_icon(&reading, Some("📚"));
    let r1 = run.open("/essay", true);
    run.wait_loaded(&r1).await;
    s.set_space_profile(&work, Some("Work"));
    let spaces = crate::control::space_infos(s);
    run.check(spaces.len() == 3, "three spaces");
    run.check(
        spaces.iter().any(|x| x.name == "Work" && x.colour == "Teal" && x.profile.as_deref() == Some("Work")),
        "Work is teal with its own cookies",
    );
    let home = spaces[0].id.clone();

    // Cookie isolation: a cookie set in Home is absent in profiled Work.
    s.switch_space(&home);
    s.select(&notes);
    run.wait_loaded(&notes).await;
    if let Some(p) = s.page(&notes) {
        let _ = p
            .eval("document.cookie = 'copper_m2=home; path=/'; 1", &copper_engine::World::Page)
            .await;
    }
    s.switch_space(&work);
    s.select(&w2);
    run.wait_loaded(&w2).await;
    let seen = match s.page(&w2) {
        Some(p) => p.eval("document.cookie", &copper_engine::World::Page).await.ok(),
        None => None,
    };
    run.check(
        seen.as_ref().is_some_and(|v| !v.to_string().contains("copper_m2")),
        "a profiled space does not see another space's cookies",
    );
    run.shot("10-work-light-left").await;

    // Back home: the screenshots that matter.
    s.switch_space(&home);
    s.select(&notes);
    run.wait_loaded(&notes).await;
    run.shot("01-light-left").await;

    // Typing in the header field: inline completion and suggestions, and
    // nothing asked of the network until Return.
    settle(&s.downgrade(), Duration::from_secs(4)).await;
    let before = REQUESTS.load(Ordering::SeqCst);
    s.ui().edit_address(s);
    s.ui().header_text().set_text("");
    type_into(&s.ui().header_text(), "127.0.0.1/es").await;
    let shown = s.ui().header_text().text().to_string();
    run.check(shown == "127.0.0.1/essay", &format!("the field completes a visited address inline ({shown})"));
    run.shot("02-field-suggestions").await;
    run.check(
        REQUESTS.load(Ordering::SeqCst) == before,
        "nothing typed reaches the network before Return",
    );
    s.ui().header_text().emit_by_name::<()>("activate", &[]);
    let here = s.active_id();
    if let Some(id) = &here {
        run.wait_loaded(id).await;
    }
    run.check(
        s.browser().active().is_some_and(|t| t.url == run.url("/essay")),
        "Return goes to the completed address in the same tab",
    );
    run.check(here.as_ref() == Some(&notes), "the field navigates the selected tab");
    s.back();
    run.wait_loaded(&notes).await;

    // The card: Ctrl+T, typing words.
    s.ui().card.open(s, Mode::Launch);
    type_into(&s.ui().card_text(), "thermal").await;
    run.shot("03-card-launch").await;
    s.ui().card.close(s);

    // Ctrl+K: open tabs, then commands; Return switches.
    s.ui().card.open(s, Mode::Palette);
    run.shot("04-palette").await;
    type_into(&s.ui().card_text(), "new").await;
    run.shot("05-palette-commands").await;
    s.ui().card_text().set_text("");
    type_into(&s.ui().card_text(), "weather").await;
    s.ui().card_text().emit_by_name::<()>("activate", &[]);
    run.check(s.active_id().as_ref() == Some(&weather), "Ctrl+K finds a tab by name and Return switches to it");
    run.check(!s.ui().card.is_open(), "the card closes on Return");
    s.select(&notes);
    run.wait_loaded(&notes).await;

    // Find.
    s.ui().stage.find.open(s);
    type_into(&s.ui().find_text(), "storage").await;
    glib::timeout_future(Duration::from_millis(400)).await;
    run.shot("06-find").await;
    s.ui().stage.find.close(s);

    // Settings and the space editor.
    let settings = crate::window::settings::dialog(s);
    run.shot("18-settings").await;
    settings.close();
    let here = s.browser().current_space_id().to_string();
    if let Some(editor) = crate::spaces::ui::editor(s, &here) {
        run.shot("19-space-editor").await;
        editor.close();
    }
    glib::timeout_future(Duration::from_millis(300)).await;

    // Dark, then the row.
    s.set_look(Look::Dark);
    run.shot("07-dark-left").await;
    s.set_tabs_position(TabsPosition::Top);
    run.shot("08-dark-top").await;
    s.set_look(Look::Light);
    run.shot("09-light-top").await;
    s.ui().card.open(s, Mode::Palette);
    run.shot("11-light-top-palette").await;
    s.ui().card.close(s);
    s.set_tabs_position(TabsPosition::Left);

    // A private tab: its own ephemeral profile, the private tint.
    let private = s.open(
        &run.url("/private"),
        OpenTab {
            foreground: true,
            private: true,
            ..Default::default()
        },
    );
    run.wait_loaded(&private).await;
    let ephemeral = s
        .page(&private)
        .map(|p| copper_engine::Profile::is_ephemeral(p.profile()))
        .unwrap_or(false);
    run.check(ephemeral, "a private tab is on an ephemeral profile");
    let cookie = match s.page(&private) {
        Some(p) => p.eval("document.cookie", &copper_engine::World::Page).await.ok(),
        None => None,
    };
    run.check(
        cookie.as_ref().is_some_and(|v| !v.to_string().contains("copper_m2")),
        "a private tab does not see normal cookies",
    );
    run.shot("12-private-left").await;
    let roots = [s.store().paths().data.clone(), crate::session::cache_dir(s.store())];
    let leaked = roots.iter().any(|r| tree_mentions(r, "private"));
    run.check(!leaked, "a private tab writes no profile folder");
    s.set_look(Look::Dark);
    run.shot("13-private-dark").await;
    s.set_look(Look::Light);

    // A crashed page.
    s.select(&docs);
    run.wait_loaded(&docs).await;
    if let Some(p) = s.page(&docs) {
        p.terminate_web_process();
    }
    glib::timeout_future(Duration::from_millis(800)).await;
    run.check(
        s.slot(&docs).is_some_and(|x| x.crashed.get()),
        "a killed web process shows the crashed page",
    );
    let crashes = s.store().paths().file(copper_core::store::DataFile::Crashes);
    s.saver().flush();
    run.check(crashes.exists(), "crashes.json is written");
    run.shot("14-crash").await;
    s.reload(&docs);
    run.wait_loaded(&docs).await;
    run.check(s.slot(&docs).is_some_and(|x| !x.crashed.get()), "Reload brings it back");

    // A failed load.
    let failed = s.open(
        "http://copper-selftest.invalid/",
        OpenTab {
            foreground: true,
            ..Default::default()
        },
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(8)
        && !s.slot(&failed).is_some_and(|x| x.failed_url.borrow().is_some())
    {
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    run.check(
        s.slot(&failed).is_some_and(|x| x.failed_url.borrow().is_some()),
        "a failed load says why, with Try Again",
    );
    run.shot("15-failed").await;
    s.close_force(&failed);

    // Forty tabs at 800 px, both layouts: no horizontal scrollbar.
    let crowd = s.add_space("Crowd");
    for i in 0..40 {
        s.open(
            &run.url(&format!("/crowd/{i}")),
            OpenTab {
                foreground: i == 39,
                at_end: true,
                ..Default::default()
            },
        );
    }
    s.window().set_default_size(800, 640);
    glib::timeout_future(Duration::from_millis(600)).await;
    let (min_w, _) = {
        let (m, n, _, _) = s.window().measure(gtk::Orientation::Horizontal, -1);
        (m, n)
    };
    run.check(min_w <= 800, &format!("40 tabs fit 800 px with the column (min width {min_w})"));
    run.shot("16-crowd-left-800").await;
    s.set_tabs_position(TabsPosition::Top);
    glib::timeout_future(Duration::from_millis(600)).await;
    let (min_w, _, _, _) = s.window().measure(gtk::Orientation::Horizontal, -1);
    run.check(min_w <= 800, &format!("40 tabs fit 800 px in the row (min width {min_w})"));
    run.shot("17-crowd-top-800").await;
    s.set_tabs_position(TabsPosition::Left);
    s.remove_space(&crowd, false);
    s.window().set_default_size(1440, 900);
    glib::timeout_future(Duration::from_millis(400)).await;

    // Leave the world as a relaunch must find it.
    s.switch_space(&home);
    s.select(&notes);
    run.wait_loaded(&notes).await;
    s.save_now();
    let expect = layout(s);
    let path = run.out.join("expect.json");
    if let Err(e) = std::fs::write(&path, serde_json::to_vec_pretty(&expect).unwrap_or_default()) {
        run.check(false, &format!("write expect.json: {e}"));
    }
    run.check(
        !session_mentions(s, "/private"),
        "the private tab is not in session.json",
    );
}

async fn restore(run: &Run) {
    let s = &run.shell;
    let expect: Option<serde_json::Value> = std::fs::read(run.out.join("expect.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    run.check(expect.is_some(), "expect.json from the fresh phase");
    let now = layout(s);
    if let Some(e) = &expect {
        run.check(*e == now, "order, pins, kept tabs, spaces and selection are back");
        if *e != now {
            println!("expected {e}\n     got {now}");
        }
    }
    let infos = run.infos();
    let live = infos.iter().filter(|t| t.live).count();
    run.check(live == 1, &format!("only the selected tab has a page ({live} live)"));
    run.check(
        infos.iter().filter(|t| !t.active).all(|t| t.sleeping),
        "every other tab is asleep",
    );
    run.check(infos.iter().all(|t| !t.private), "private tabs are not restored");
    run.shot("20-restored").await;
    let other = infos.iter().find(|t| !t.active && t.index.is_some()).map(|t| t.id.clone());
    if let Some(id) = other {
        s.select(&id);
        run.wait_loaded(&id).await;
        let woke = tab_infos(s).iter().find(|t| t.id == id).is_some_and(|t| t.live && !t.sleeping);
        run.check(woke, "selecting a sleeping tab wakes it");
    }
}

async fn mac(run: &Run) {
    let s = &run.shell;
    let spaces = crate::control::space_infos(s);
    run.check(spaces.len() == 4, &format!("the Mac session's four spaces ({})", spaces.len()));
    let infos = run.infos();
    run.check(infos.len() > 100, &format!("its tabs ({})", infos.len()));
    run.check(infos.iter().filter(|t| t.pinned).count() == 6, "its six pins");
    run.check(infos.iter().filter(|t| t.live).count() <= 1, "nothing but the selected tab loads");
    run.shot("30-mac-session").await;
}

/// What a relaunch must show, without ids (they are fresh each run).
fn layout(s: &Shell) -> serde_json::Value {
    let b = s.browser();
    let current = b.current_space_id().to_string();
    let active = b.active().map(|t| t.url.clone());
    let pins: Vec<_> = b.pins().iter().map(|t| t.url.clone()).collect();
    let spaces: Vec<_> = b
        .spaces()
        .map(|sp| {
            serde_json::json!({
                "name": sp.name,
                "hue": sp.hue.map(|h| (h * 1000.0).round() / 1000.0),
                "icon": sp.icon,
                "profile": sp.profile,
                "current": sp.id == current,
                "tabs": b
                    .row(&sp.id)
                    .iter()
                    .filter(|t| !t.private)
                    .map(|t| serde_json::json!([t.url, t.saved]))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({ "pins": pins, "spaces": spaces, "active": active })
}

/// Some file or folder under `root` has `needle` in its name.
fn tree_mentions(root: &std::path::Path, needle: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else { return false };
    rd.flatten().any(|e| {
        e.file_name().to_string_lossy().contains(needle)
            || (e.file_type().is_ok_and(|t| t.is_dir()) && tree_mentions(&e.path(), needle))
    })
}

fn session_mentions(s: &Shell, needle: &str) -> bool {
    std::fs::read_to_string(s.store().paths().data.join("session.json"))
        .map(|t| t.contains(needle))
        .unwrap_or(false)
}

// MARK: a tiny local site

struct Page {
    path: &'static str,
    title: &'static str,
    heading: &'static str,
    body: &'static str,
    hue: f64,
    letter: &'static str,
}

const PAGES: &[Page] = &[
    Page { path: "/studio", title: "Office Commun", heading: "Office Commun", body: "Independent design studio working across research, identity and production. We work alongside the teams we make things for, from the first question to the live release.", hue: 0.0, letter: "O" },
    Page { path: "/mail", title: "Inbox (3) — Mail", heading: "Inbox", body: "Three unread: the site visit schedule, a shipping notice, and the October invoice.", hue: 0.58, letter: "M" },
    Page { path: "/arena", title: "Are.na / Blocks", heading: "Blocks", body: "A quiet place to collect references: images, links, and notes, arranged into channels.", hue: 0.08, letter: "A" },
    Page { path: "/docs", title: "WebKitWebView — WebKitGTK 6.0 Reference", heading: "WebKitWebView", body: "The central class of the WebKitGTK API. It is responsible for managing the drawing of the content and forwarding of events.", hue: 0.62, letter: "W" },
    Page { path: "/notes", title: "Field notes: thermal storage at desert sites, week 41", heading: "Field notes, week 41", body: "Charging held steady through the afternoon peak. The storage block reached its setpoint by two, and discharge carried the load well past sunset. Dust on the east array cost about three percent until the Thursday wash.", hue: 0.1, letter: "N" },
    Page { path: "/rust", title: "Validating References with Lifetimes - The Rust Programming Language", heading: "Validating References with Lifetimes", body: "Lifetimes are another kind of generic that we’ve already been using. Rather than ensuring that a type has the behavior we want, lifetimes ensure that references are valid as long as we need them to be.", hue: 0.03, letter: "R" },
    Page { path: "/long", title: "A very long title that goes on and on to check that the column ellipsizes it properly rather than ever scrolling sideways", heading: "A long title", body: "This page exists to have a title longer than any tab can show.", hue: 0.8, letter: "L" },
    Page { path: "/weather", title: "Weather — Mojave, 34° and clear", heading: "34° and clear", body: "Clear skies through the weekend. Winds from the west, eight to twelve miles an hour.", hue: 0.14, letter: "W" },
    Page { path: "/tracker", title: "Sprint 42 · Tracker", heading: "Sprint 42", body: "Eleven open, four in review, twenty-three done.", hue: 0.48, letter: "T" },
    Page { path: "/design", title: "Column spacing — Design review", heading: "Column spacing", body: "Rows at thirty-four points, a single hairline between kept and today’s tabs.", hue: 0.9, letter: "D" },
    Page { path: "/standup", title: "Standup notes — Tuesday", heading: "Tuesday", body: "Shell lands today; spaces after lunch.", hue: 0.36, letter: "S" },
    Page { path: "/essay", title: "On Restraint in Interface Design", heading: "On restraint", body: "The best interface is the one you stop noticing. Every border, every badge, every animation has to earn its place against the page it frames.", hue: 0.7, letter: "E" },
    Page { path: "/private", title: "Private window test", heading: "Nothing here is kept", body: "This tab has its own cookie jar, and it is forgotten when the tab closes.", hue: 0.75, letter: "P" },
];

type Icons = Arc<RwLock<Vec<Vec<u8>>>>;

/// Serve the pages on 127.0.0.1; returns the port.
fn serve(icons: Icons) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:47731")
        .or_else(|_| std::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bind");
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let icons = icons.read().map(|i| i.clone()).unwrap_or_default();
            std::thread::spawn(move || answer(stream, &icons));
        }
    });
    port
}

/// Requests the local site has answered (the "nothing leaves before
/// Return" check reads it).
static REQUESTS: AtomicUsize = AtomicUsize::new(0);

fn answer(mut stream: std::net::TcpStream, icons: &[Vec<u8>]) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    REQUESTS.fetch_add(1, Ordering::SeqCst);
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (status, kind, body): (&str, &str, Vec<u8>) = if let Some(i) = path.strip_prefix("/icon/") {
        let i: usize = i.trim_end_matches(".png").parse().unwrap_or(0);
        match icons.get(i) {
            Some(png) => ("200 OK", "image/png", png.clone()),
            None => ("404 Not Found", "text/plain", b"no".to_vec()),
        }
    } else if let Some(n) = path.strip_prefix("/crowd/") {
        let html = page_html(&format!("Crowded tab {n}"), &format!("Tab {n}"), "One of forty.", 0);
        ("200 OK", "text/html; charset=utf-8", html.into_bytes())
    } else if let Some((i, p)) = PAGES.iter().enumerate().find(|(_, p)| p.path == path) {
        ("200 OK", "text/html; charset=utf-8", page_html(p.title, p.heading, p.body, i).into_bytes())
    } else {
        ("404 Not Found", "text/plain", b"not found".to_vec())
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

fn page_html(title: &str, heading: &str, body: &str, icon: usize) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>{title}</title>
<link rel="icon" type="image/png" href="/icon/{icon}.png">
<style>
:root {{ color-scheme: light; }}
body {{ margin: 0; font: 17px/1.65 "Cantarell", "Noto Sans", system-ui, sans-serif; color: #111; background: #fff; }}
main {{ max-width: 620px; margin: 22vh auto 0; padding: 0 32px; }}
h1 {{ font-size: 18px; font-weight: 600; margin: 0 0 18px; letter-spacing: -0.01em; }}
p {{ color: #666; margin: 0 0 18px; }}
a {{ color: #111; text-decoration: none; border-bottom: 1px solid #ddd; }}
.cta {{ display: inline-block; margin-top: 8px; background: #111; color: #fff; border-radius: 99px; padding: 10px 22px; font-size: 15px; border: 0; }}
</style></head><body><main><h1>{heading}</h1><p>{body}</p>
<p>Read the <a href="/essay">essay</a>, the <a href="/notes">field notes</a> or the <a href="/docs">reference</a>.</p>
<a class="cta" href="/studio">Start a project</a></main></body></html>"#
    )
}

/// A 32×32 favicon like a real site's: a letter on a rounded square.
fn icon_png(shell: &Shell, hue: f64, letter: &str) -> Vec<u8> {
    let c = crate::spaces::colour::Rgb::hsv(hue, 0.5, 0.72);
    let snap = gtk::Snapshot::new();
    let rect = gtk::graphene::Rect::new(0.0, 0.0, 32.0, 32.0);
    snap.push_rounded_clip(&gtk::gsk::RoundedRect::from_rect(rect, 7.0));
    snap.append_color(&gtk::gdk::RGBA::new(c.r as f32, c.g as f32, c.b as f32, 1.0), &rect);
    let layout = shell.window().create_pango_layout(Some(letter));
    let mut font = gtk::pango::FontDescription::from_string("Sans Bold");
    font.set_absolute_size(18.0 * gtk::pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    let (w, h) = layout.pixel_size();
    snap.translate(&gtk::graphene::Point::new((32 - w) as f32 / 2.0, (32 - h) as f32 / 2.0));
    snap.append_layout(&layout, &gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.95));
    snap.pop();
    let Some(node) = snap.to_node() else { return Vec::new() };
    let Some(renderer) = shell.window().renderer() else { return Vec::new() };
    renderer.render_texture(&node, Some(&rect)).save_to_png_bytes().to_vec()
}
