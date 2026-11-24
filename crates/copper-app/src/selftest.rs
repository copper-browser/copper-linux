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
//! * `chrome` — Mac #181's small chrome items: a globe (never a letter) for
//!   a site with no icon, in a pin and a row; the can't-connect page and its
//!   Details; "Not secure" on an http page's address under the pointer.
//! * `calendar` — the Calendar pin's card: with no calendar, then with one
//!   this run serves (meetings around now); what it lists and leaves out,
//!   that the page keeps the keyboard, and that it goes when the pointer
//!   leaves. Saves the card light and dark.
//! * `sleep` — tab memory (Copper#131, #134, #181, #127): heavy pages,
//!   a light page, two pins and another space's own tab; the sweep with
//!   seconds standing in for the half hour puts the heavy ones and the big
//!   pin to sleep and leaves the rest; a pin's killed process comes back in
//!   the background three times; a page on screen killed three times stops
//!   with "This page keeps closing as it loads."; Ctrl+W on a pin steps
//!   away. Writes nothing for the next phase but the session.
//! * `sleep-restore` — the same world again: the big pin asleep at quit
//!   stays asleep, the other pin loads in the background.
//!
//! Headless weston has no seat, so input is driven through the shell and
//! the widgets' own signals (`insert-at-cursor` for typing).

use crate::app::{set_exit, settle};
use crate::control::{TabInfo, tab_infos};
use crate::palette::Mode;
use crate::shell::GtkShell;
use crate::shell::{OpenTab, Shell};
use copper_core::TabId;
use copper_core::settings::{Look, TabSwitching, TabsPosition};
use copper_engine::PageView;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita::prelude::{AdwDialogExt, PreferencesDialogExt};
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
            "calendar" => calendar(&run).await,
            "sleep" => sleep(&run).await,
            "sleep-restore" => sleep_restore(&run).await,
            "chrome" => chrome(&run).await,
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
    run.check(
        s.browser().all_tabs().count() == 0,
        "a fresh world has no tabs",
    );
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
    run.check(
        infos.iter().filter(|t| t.saved && !t.pinned).count() == 2,
        "two kept tabs",
    );
    run.check(
        s.browser()
            .tab(&long)
            .is_some_and(|t| t.title.starts_with("A very long title")),
        "titles come from the page",
    );
    run.check(
        s.favicons().borrow().contains_key(&notes),
        "favicons come from the page",
    );

    // Reorder by dropping: weather before rust.
    s.drop_tab(&weather, &rust, false);
    let order: Vec<TabId> = s.visible_ids();
    let pw = order.iter().position(|i| *i == weather);
    let pr = order.iter().position(|i| *i == rust);
    run.check(
        pw.is_some() && pw < pr,
        "a dropped tab lands before its target",
    );

    // Close and reopen.
    s.select(&rust);
    s.close(&rust);
    run.check(s.browser().tab(&rust).is_none(), "Ctrl+W closes");
    let back = s.reopen();
    run.check(
        back.as_ref()
            .and_then(|id| s.browser().tab(id).map(|t| t.url.ends_with("/rust")))
            == Some(true),
        "Ctrl+Shift+T brings it back",
    );
    let rust = back.unwrap_or(rust);
    run.wait_loaded(&rust).await;

    // Ctrl+Tab walks the visible list.
    s.select(&notes);
    let before = s.active_id();
    s.cycle_tab(true);
    s.cycle_tab(false);
    run.check(
        s.active_id() == before,
        "Ctrl+Tab then Ctrl+Shift+Tab come back",
    );
    // Most recent: held, each Tab goes one deeper; let go, it lands there.
    s.update_settings(|st| st.set_tab_switching(TabSwitching::Recent));
    s.select(&weather);
    s.select(&rust);
    s.select(&notes);
    s.switch_tab(true);
    let first = s.active_id();
    s.switch_tab(true);
    let second = s.active_id();
    s.tab_cycle_ended();
    s.switch_tab(true);
    s.tab_cycle_ended();
    run.check(
        first.as_ref() == Some(&rust)
            && second.as_ref() == Some(&weather)
            && s.active_id().as_ref() == Some(&notes),
        "Ctrl+Tab in most-recent order walks back through the tabs used",
    );
    s.update_settings(|st| st.set_tab_switching(TabSwitching::Row));
    // Zoom is kept for the site and put back when a page of it loads.
    s.select(&notes);
    run.wait_loaded(&notes).await;
    s.zoom(Some(true));
    let kept = s.settings().zoom_for("127.0.0.1");
    if let Some(p) = s.page(&notes) {
        p.set_zoom(1.0);
    }
    s.reload(&notes);
    glib::timeout_future(Duration::from_millis(300)).await;
    run.wait_loaded(&notes).await;
    let back = s.page(&notes).map(|p| p.zoom());
    s.zoom(None);
    run.check(
        (kept - 1.1).abs() < 0.001
            && back.is_some_and(|z| (z - 1.1).abs() < 0.01)
            && s.settings().get("zoom.127.0.0.1").is_none(),
        "zoom is remembered per site, and forgotten at 100%",
    );
    s.select_nth(Some(0));
    run.check(
        s.active_id().as_ref() == Some(&studio),
        "Ctrl+1 is the first pin",
    );
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
        spaces.iter().any(|x| {
            x.name == "Work" && x.colour == "Teal" && x.profile.as_deref() == Some("Work")
        }),
        "Work is teal with its own cookies",
    );
    let home = spaces[0].id.clone();

    // Cookie isolation: a cookie set in Home is absent in profiled Work.
    s.switch_space(&home);
    s.select(&notes);
    run.wait_loaded(&notes).await;
    if let Some(p) = s.page(&notes) {
        let _ = p
            .eval(
                "document.cookie = 'copper_m2=home; path=/'; 1",
                &copper_engine::World::Page,
            )
            .await;
    }
    s.switch_space(&work);
    s.select(&w2);
    run.wait_loaded(&w2).await;
    let seen = match s.page(&w2) {
        Some(p) => p
            .eval("document.cookie", &copper_engine::World::Page)
            .await
            .ok(),
        None => None,
    };
    run.check(
        seen.as_ref()
            .is_some_and(|v| !v.to_string().contains("copper_m2")),
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
    run.check(
        shown == "127.0.0.1/essay",
        &format!("the field completes a visited address inline ({shown})"),
    );
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
        s.browser()
            .active()
            .is_some_and(|t| t.url == run.url("/essay")),
        "Return goes to the completed address in the same tab",
    );
    run.check(
        here.as_ref() == Some(&notes),
        "the field navigates the selected tab",
    );
    s.back();
    run.wait_loaded(&notes).await;

    // The card: Ctrl+T, typing words.
    s.ui().card.open(s, Mode::Launch);
    type_into(&s.ui().card_text(), "thermal").await;
    run.shot("03-card-launch").await;
    s.ui().card.close(s);

    // Ctrl+T with nothing typed: Recently Closed, Return reopens.
    let gone = run.open("/design", false);
    s.close(&gone);
    s.ui().card.open(s, Mode::Launch);
    run.shot("03-card-closed").await;
    s.ui().card_text().emit_by_name::<()>("activate", &[]);
    let back = s.active_id();
    run.check(
        back.as_ref()
            .and_then(|id| s.browser().tab(id).map(|t| t.url.clone()))
            == Some(run.url("/design")),
        "Ctrl+T's Recently Closed reopens the tab",
    );
    if let Some(id) = back {
        s.close(&id);
    }
    s.select(&notes);

    // Ctrl+S: the column goes and comes back with its rows.
    s.ui().toggle_fold(s);
    run.check(
        !s.ui().column.widget.is_visible(),
        "Ctrl+S hides the column",
    );
    run.shot("03-folded").await;
    s.ui().toggle_fold(s);
    run.check(
        s.ui().column.widget.is_visible() && s.ui().column.pin(&s.browser().pins()[0].id).is_some(),
        "Ctrl+S again shows it, drawn",
    );

    // Ctrl+K: open tabs, then commands; Return switches.
    s.ui().card.open(s, Mode::Palette);
    run.shot("04-palette").await;
    type_into(&s.ui().card_text(), "new").await;
    run.shot("05-palette-commands").await;
    s.ui().card_text().set_text("");
    type_into(&s.ui().card_text(), "weather").await;
    s.ui().card_text().emit_by_name::<()>("activate", &[]);
    run.check(
        s.active_id().as_ref() == Some(&weather),
        "Ctrl+K finds a tab by name and Return switches to it",
    );
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
        Some(p) => p
            .eval("document.cookie", &copper_engine::World::Page)
            .await
            .ok(),
        None => None,
    };
    run.check(
        cookie
            .as_ref()
            .is_some_and(|v| !v.to_string().contains("copper_m2")),
        "a private tab does not see normal cookies",
    );
    run.shot("12-private-left").await;
    let roots = [
        s.store().paths().data.clone(),
        crate::session::cache_dir(s.store()),
    ];
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
    let crashes = s
        .store()
        .paths()
        .file(copper_core::store::DataFile::Crashes);
    s.saver().flush();
    run.check(crashes.exists(), "crashes.json is written");
    run.shot("14-crash").await;
    s.reload(&docs);
    run.wait_loaded(&docs).await;
    run.check(
        s.slot(&docs).is_some_and(|x| !x.crashed.get()),
        "Reload brings it back",
    );

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
        && !s
            .slot(&failed)
            .is_some_and(|x| x.failed_url.borrow().is_some())
    {
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    run.check(
        s.slot(&failed)
            .is_some_and(|x| x.failed_url.borrow().is_some()),
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
    run.check(
        min_w <= 800,
        &format!("40 tabs fit 800 px with the column (min width {min_w})"),
    );
    run.shot("16-crowd-left-800").await;
    s.set_tabs_position(TabsPosition::Top);
    glib::timeout_future(Duration::from_millis(600)).await;
    let (min_w, _, _, _) = s.window().measure(gtk::Orientation::Horizontal, -1);
    run.check(
        min_w <= 800,
        &format!("40 tabs fit 800 px in the row (min width {min_w})"),
    );
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
    if let Err(e) = std::fs::write(
        &path,
        serde_json::to_vec_pretty(&expect).unwrap_or_default(),
    ) {
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
        run.check(
            *e == now,
            "order, pins, kept tabs, spaces and selection are back",
        );
        if *e != now {
            println!("expected {e}\n     got {now}");
        }
    }
    // Pins load in the background once the launch settles (Copper#134).
    let pinned = |t: &&TabInfo| t.pinned;
    let start = Instant::now();
    while run.infos().iter().filter(pinned).any(|t| !t.live)
        && start.elapsed() < Duration::from_secs(20)
    {
        glib::timeout_future(Duration::from_millis(200)).await;
    }
    let infos = run.infos();
    let live = infos.iter().filter(|t| t.live && !t.pinned).count();
    run.check(
        live == 1,
        &format!("only the selected tab of the row has a page ({live} live)"),
    );
    run.check(
        infos.iter().filter(pinned).all(|t| t.live),
        "every pin has its page",
    );
    run.check(
        infos
            .iter()
            .filter(|t| !t.active && !t.pinned)
            .all(|t| t.sleeping),
        "every other tab is asleep",
    );
    run.check(
        infos.iter().all(|t| !t.private),
        "private tabs are not restored",
    );
    run.shot("20-restored").await;
    let other = infos
        .iter()
        .find(|t| !t.active && t.index.is_some())
        .map(|t| t.id.clone());
    if let Some(id) = other {
        s.select(&id);
        run.wait_loaded(&id).await;
        let woke = tab_infos(s)
            .iter()
            .find(|t| t.id == id)
            .is_some_and(|t| t.live && !t.sleeping);
        run.check(woke, "selecting a sleeping tab wakes it");
    }
}

/// A page that holds `?mb` megabytes, as X or Gmail hold a gigabyte (the
/// Mac's `docs/fixtures/heavy.html`).
const HEAVY: &str = r#"<!doctype html><title>page</title><body><script>
const q = new URLSearchParams(location.search), mb = +q.get('mb') || 0;
window.held = [];
for (let i = 0; i < mb; i += 50) held.push(new Uint8Array(50 << 20).fill(1));
document.title = q.get('name') || 'page';
</script>"#;

/// What tab `id`'s page weighs now, in MB, and its web process.
async fn weigh(s: &Shell, id: &TabId) -> Option<(u32, u64)> {
    let page = s.page(id)?;
    page.weight().await.map(|w| (w.process, w.bytes >> 20))
}

/// End tab `id`'s web process the way the OS would under memory pressure.
async fn kill(s: &Shell, id: &TabId) -> bool {
    let Some((pid, _)) = weigh(s, id).await else {
        return false;
    };
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .is_ok_and(|st| st.success())
}

/// Wait up to `secs` for `ok`.
async fn until(secs: u64, ok: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if ok() {
            return true;
        }
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    ok()
}

async fn sleep(run: &Run) {
    use copper_core::settings::keys;
    let s = &run.shell;
    // Heavy at 300 MB whatever this machine's memory, every page under the
    // count and the footprint, a pin big at 300 MB after two seconds.
    s.update_settings(|st| {
        st.set(keys::SLEEP_HEAVY, 300);
        st.set(keys::SLEEP_KEEP, 40);
        st.set(keys::SLEEP_BUDGET, 100_000);
        st.set(keys::PINS_SLEEP_AFTER, 2);
    });
    let heavy = |name: &str, mb: u32| format!("/heavy?name={name}&mb={mb}");
    let front = run.open(&heavy("front", 0), true);
    run.wait_loaded(&front).await;
    let mut tabs = Vec::new();
    for (name, mb) in [
        ("heavy", 400),
        ("light", 0),
        ("pinbig", 400),
        ("pinlight", 0),
    ] {
        let id = run.open(&heavy(name, mb), false);
        run.wait_loaded(&id).await;
        tabs.push(id);
    }
    let [loose, light, pin_big, pin_light] = <[TabId; 4]>::try_from(tabs).expect("four");
    s.pin(&pin_big);
    s.pin(&pin_light);
    // Another space whose own tab is heavy, then back.
    let home = s.browser().current_space_id().to_string();
    let away = s.add_space("Away");
    let other = run.open(&heavy("other", 400), true);
    run.wait_loaded(&other).await;
    s.switch_space(&home);
    s.select(&front);
    run.check(
        s.browser().is_remembered(&other),
        "the other space comes back to its heavy tab",
    );
    let _ = away;
    let mut before = 0;
    for id in [&front, &loose, &light, &pin_big, &pin_light, &other] {
        let w = weigh(s, id).await;
        println!(
            "note: {} weighs {w:?}",
            s.browser()
                .tab(id)
                .map(|t| t.title.clone())
                .unwrap_or_default()
        );
        before += w.map_or(0, |w| w.1);
    }
    run.check(
        weigh(s, &loose).await.is_some_and(|w| w.1 >= 400),
        "a heavy page's web process is found and weighed",
    );

    // The sweep, three seconds standing in for the half hour.
    glib::timeout_future(Duration::from_secs(3)).await;
    let chosen = copper_shell_core::shell::sweep(&s.downgrade(), Some(2.0)).await;
    glib::timeout_future(Duration::from_millis(500)).await;
    let live = |id: &TabId| s.slot(id).is_some();
    println!("note: the sweep chose {}", chosen.len());
    run.check(!live(&loose), "the heavy page left alone sleeps");
    run.check(!live(&other), "the other space's heavy tab sleeps");
    run.check(
        !live(&pin_big),
        "the big pin left alone sleeps (Let big pins sleep)",
    );
    run.check(live(&light), "the light page under the budget stays");
    run.check(live(&pin_light), "the light pin stays");
    run.check(live(&front), "the page on screen stays");
    run.check(
        s.browser().tab(&loose).is_some_and(|t| t.sleeping),
        "the heavy tab is still in the row, asleep",
    );
    let mut after = 0;
    for id in [&front, &light, &pin_light] {
        after += weigh(s, id).await.map_or(0, |w| w.1);
    }
    println!("note: {before} MB → {after} MB");
    run.check(before >= after + 1100, "over a gigabyte comes back");
    let status = s.pins_status(None);
    println!("note: pins {status}");
    run.check(
        status["pins"].as_array().is_some_and(|p| {
            p.iter()
                .any(|p| p["dozed"] == true && p["title"] == "pinbig")
        }),
        "copper pins says the big pin dozed",
    );
    // Nobody loads a dozed pin again; the loader does load a pin with no page.
    glib::timeout_future(Duration::from_secs(2)).await;
    run.check(
        !live(&pin_big),
        "the dozed pin is not loaded again behind your back",
    );
    s.pin(&loose);
    run.check(
        until(10, || live(&loose)).await,
        "a sleeping tab pinned is loaded in the background",
    );
    run.check(
        s.active_id() == Some(front.clone()),
        "…without coming on screen",
    );

    // A pin's process killed out of sight comes back, three times in ten minutes.
    for n in 1..=3 {
        run.check(
            kill(s, &pin_light).await,
            &format!("pin process killed ({n})"),
        );
        let back = until(10, || {
            s.slot(&pin_light)
                .is_some_and(|x| !x.crashed.get() && !x.loading.get())
        })
        .await;
        run.wait_loaded(&pin_light).await;
        run.check(
            back && weigh(s, &pin_light).await.is_some(),
            &format!("the pin reloads in the background ({n})"),
        );
    }
    kill(s, &pin_light).await;
    run.check(
        until(5, || s.slot(&pin_light).is_some_and(|x| x.crashed.get())).await,
        "the fourth death in ten minutes leaves the pin stopped",
    );

    // The page on screen: reloaded twice, the third death stops it.
    for n in 1..=2 {
        run.check(
            kill(s, &front).await,
            &format!("front process killed ({n})"),
        );
        glib::timeout_future(Duration::from_millis(600)).await;
        run.wait_loaded(&front).await;
        run.check(
            s.slot(&front).is_some_and(|x| !x.crashed.get()),
            &format!("the page on screen loads again at once ({n})"),
        );
    }
    kill(s, &front).await;
    let stopped = until(5, || {
        s.slot(&front)
            .is_some_and(|x| x.crashed.get() && x.failed_url.borrow().is_some())
    })
    .await;
    run.check(
        stopped,
        "the third death in half a minute stops: This page keeps closing as it loads.",
    );
    run.shot("50-keeps-closing").await;
    s.reload(&front);
    run.wait_loaded(&front).await;
    run.check(
        s.slot(&front).is_some_and(|x| !x.crashed.get()),
        "Try Again loads the address afresh",
    );

    // Ctrl+W on a pin steps away; its page stays.
    s.select(&pin_light);
    s.close(&pin_light);
    run.check(
        s.active_id() != Some(pin_light.clone()),
        "Ctrl+W on a pin steps away",
    );
    run.check(
        live(&pin_light) && s.browser().pins().iter().any(|t| t.id == pin_light),
        "…and the pin keeps its page",
    );
    s.save_now();
}

async fn sleep_restore(run: &Run) {
    let s = &run.shell;
    let pin = |title: &str| {
        s.browser()
            .pins()
            .iter()
            .find(|t| t.title == title)
            .map(|t| t.id.clone())
    };
    let (Some(big), Some(light)) = (pin("pinbig"), pin("pinlight")) else {
        run.check(false, "the pins came back");
        return;
    };
    run.check(
        until(15, || s.slot(&light).is_some()).await,
        "a pin is loaded in the background at launch",
    );
    glib::timeout_future(Duration::from_secs(2)).await;
    run.check(
        s.slot(&big).is_none(),
        "a pin asleep at quit stays asleep at launch",
    );
    s.select(&big);
    run.wait_loaded(&big).await;
    run.check(s.slot(&big).is_some(), "…until you go to it");
}

async fn mac(run: &Run) {
    let s = &run.shell;
    let spaces = crate::control::space_infos(s);
    run.check(
        spaces.len() == 4,
        &format!("the Mac session's four spaces ({})", spaces.len()),
    );
    let infos = run.infos();
    run.check(infos.len() > 100, &format!("its tabs ({})", infos.len()));
    run.check(
        infos.iter().filter(|t| t.pinned).count() == 6,
        "its six pins",
    );
    // Pins load in the background once the launch settles (Copper#134).
    run.check(
        infos.iter().filter(|t| t.live && !t.pinned).count() <= 1,
        "nothing but the selected tab and the pins loads",
    );
    run.shot("30-mac-session").await;
}

async fn chrome(run: &Run) {
    let s = &run.shell;
    s.set_look(Look::Light);
    s.set_tabs_position(TabsPosition::Left);
    let studio = run.open("/studio", true);
    run.wait_loaded(&studio).await;
    // A plain-text 404: no icon, ever.
    let bare = run.open("/no-icon", true);
    run.wait_loaded(&bare).await;
    let pin = run.open("/no-icon-pinned", true);
    run.wait_loaded(&pin).await;
    s.pin(&pin);
    s.select(&bare);
    glib::timeout_future(Duration::from_millis(800)).await;
    run.check(
        !s.favicons().borrow().contains_key(&bare),
        "a site with no icon has none",
    );
    let window: gtk::Widget = s.window().clone().upcast();
    let globes = descendants(&window)
        .filter(|w| w.has_css_class("globe") && w.is_drawable())
        .count();
    run.check(
        globes >= 2,
        &format!("its pin and its row wear a globe ({globes})"),
    );
    run.check(
        !descendants(&window).any(|w| w.has_css_class("chip")),
        "no letter tiles",
    );

    // "Not secure" under the pointer on the http page's address.
    let insecure = || {
        descendants(&window)
            .find(|w| w.has_css_class("not-secure"))
            .is_some_and(|w| w.is_drawable())
    };
    s.ui().header.set_hovering(s, false);
    run.check(!insecure(), "no \"Not secure\" until the pointer is on it");
    s.ui().header.set_hovering(s, true);
    glib::timeout_future(Duration::from_millis(300)).await;
    run.check(
        insecure(),
        "an http page's address says \"Not secure\" under the pointer",
    );
    run.shot("60-not-secure").await;
    s.ui().header.set_hovering(s, false);
    run.shot("61-globes").await;

    // Can't connect.
    let failed = s.open(
        "http://copper-selftest.invalid/",
        OpenTab {
            foreground: true,
            ..Default::default()
        },
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10)
        && !s
            .slot(&failed)
            .is_some_and(|x| x.failed_url.borrow().is_some())
    {
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    let page = descendants(&window)
        .filter_map(|w| w.downcast::<libadwaita::StatusPage>().ok())
        .find(|p| p.is_drawable());
    let said = page.as_ref().map(|p| {
        (
            p.title().to_string(),
            p.description().unwrap_or_default().to_string(),
        )
    });
    println!("note: the page says {said:?}");
    run.check(
        said.as_ref().is_some_and(|(t, d)| {
            t == "Can\u{2019}t connect to copper-selftest.invalid"
                && d == "No site at that address."
        }),
        "a page that can't load says which site and why",
    );
    run.shot("62-cant-connect").await;
    let details = page
        .and_then(|p| descendants(p.upcast_ref()).find_map(|w| w.downcast::<gtk::Expander>().ok()));
    run.check(
        details.as_ref().is_some_and(|d| d.is_drawable()),
        "with its details on request",
    );
    if let Some(d) = details {
        d.set_expanded(true);
    }
    run.shot("63-cant-connect-details").await;
    s.select(&bare);
    s.set_tabs_position(TabsPosition::Top);
    run.shot("64-globes-top").await;
}

/// Every widget under `root`, depth first.
fn descendants(root: &gtk::Widget) -> impl Iterator<Item = gtk::Widget> {
    let mut stack = vec![root.clone()];
    std::iter::from_fn(move || {
        let w = stack.pop()?;
        let mut c = w.first_child();
        while let Some(k) = c {
            c = k.next_sibling();
            stack.push(k);
        }
        Some(w)
    })
}

/// What a relaunch must show, without ids (they are fresh each run).
/// The served calendar's address: Google's shape, so its owner reads.
const CALENDAR: &str = "/calendar/ical/selftest%40example.com/private-x/basic.ics";

async fn calendar(run: &Run) {
    use crate::window::peek;
    let s = &run.shell;
    s.set_look(Look::Light);
    s.set_tabs_position(TabsPosition::Left);
    // Asleep before it loads: signed out, Google sends it elsewhere.
    let pin = s.open("https://calendar.google.com/calendar/r", OpenTab::default());
    s.drop_page(&pin);
    s.browser_mut().set_sleeping(&pin, true);
    s.pin(&pin);
    let page = run.open("/essay", true);
    run.wait_loaded(&page).await;
    if let Some(p) = s.active_page() {
        crate::palette::p_focus(&p);
    }
    settle(&s.downgrade(), Duration::from_secs(2)).await;
    run.check(
        s.ui()
            .column
            .pin(&pin)
            .is_some_and(|p| p.tooltip_text().is_none()),
        "the Calendar pin has no tooltip over its card",
    );
    let focus = gtk::prelude::GtkWindowExt::focus(s.window());

    // A pass over the pin opens nothing.
    peek::rest(s, &pin);
    glib::timeout_future(Duration::from_millis(200)).await;
    peek::left(s, &pin);
    glib::timeout_future(Duration::from_millis(500)).await;
    run.check(
        peek::popover().is_none(),
        "passing over the pin opens nothing",
    );

    peek::rest(s, &pin);
    glib::timeout_future(Duration::from_millis(700)).await;
    let lines = peek::lines();
    run.check(
        lines
            .iter()
            .any(|l| l == copper_shell_core::calendar::NO_FEEDS),
        &format!("with no calendar the card says where to add one: {lines:?}"),
    );
    card_shot(run, "calendar-card-none");
    peek::hide();

    run.check(
        copper_shell_core::calendar::add(s, &run.url(CALENDAR)).is_ok(),
        "a calendar's address is added",
    );
    peek::rest(s, &pin);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10)
        && !peek::lines().iter().any(|l| l == "Daily standup")
    {
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    let lines = peek::lines();
    println!("card: {lines:?}");
    let at = |what: &str| lines.iter().position(|l| l.starts_with(what));
    run.check(
        matches!(
            (at("Daily standup"), at("Design review"), at("Lunch")),
            (Some(a), Some(b), Some(c)) if a < b && b < c
        ),
        "the meetings, soonest first",
    );
    run.check(
        at("Now · until ").is_some() && at("In 25 min · ").is_some(),
        "the standup is on now, the review in 25 minutes",
    );
    run.check(
        lines.iter().filter(|l| *l == "[Join]").count() == 2,
        "Join for the Meet and the Zoom call, none for lunch",
    );
    run.check(
        at("Cancelled").is_none() && at("Vendor").is_none() && at("Offsite").is_none(),
        "no cancelled, declined or all-day events",
    );
    run.check(
        gtk::prelude::GtkWindowExt::focus(s.window()) == focus
            && peek::popover().is_some_and(|p| p.focus_child().is_none()),
        "the page keeps the keyboard",
    );
    for (look, name) in [
        (Look::Light, "calendar-card"),
        (Look::Dark, "calendar-card-dark"),
    ] {
        s.set_look(look);
        glib::timeout_future(Duration::from_millis(400)).await;
        card_shot(run, name);
    }
    s.set_look(Look::Light);
    let settings = crate::window::settings::dialog(s);
    settings.set_visible_page_name(crate::features::settings::calendar::PAGE);
    run.shot("calendar-settings").await;
    settings.close();

    peek::left(s, &pin);
    glib::timeout_future(Duration::from_millis(500)).await;
    run.check(
        peek::popover().is_none(),
        "the card goes once the pointer leaves",
    );
}

/// The open card, as a picture of its own (a popover is a surface of its
/// own, not in the window's).
fn card_shot(run: &Run, name: &str) {
    let path = run.out.join(format!("{name}.png"));
    match crate::window::peek::popover()
        .ok_or_else(|| anyhow::anyhow!("no card"))
        .and_then(|pop| crate::screenshot::save(pop.upcast_ref(), &path))
    {
        Ok(()) => println!("SHOT {}", path.display()),
        Err(e) => run.check(false, &format!("screenshot {name}: {e}")),
    }
}

/// A calendar around now: the Mac's staged meetings
/// (`CalendarPeek.bench` `stage`), then what the card leaves out.
fn calendar_ics() -> String {
    let now = copper_core::date::now_unix() as i64;
    let at = |minutes: i64| {
        glib::DateTime::from_unix_utc(now + minutes * 60)
            .and_then(|d| d.format("%Y%m%dT%H%M%SZ"))
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    let day = glib::DateTime::from_unix_utc(now)
        .and_then(|d| d.format("%Y%m%d"))
        .map(|s| s.to_string())
        .unwrap_or_default();
    let event = |uid: &str, start: i64, end: i64, more: &str| {
        format!(
            "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:{}\r\nDTEND:{}\r\n{more}END:VEVENT\r\n",
            at(start),
            at(end)
        )
    };
    [
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Selftest\r\n".to_string(),
        // Yesterday's, repeating daily: today's is on now.
        event(
            "standup",
            -5 - 24 * 60,
            10 - 24 * 60,
            "RRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Daily standup\r\nDESCRIPTION:Join with Google Meet: https://meet.google.com/abc-defg-hij\\nAgenda: https://docs.google.com/document/d/1\r\n",
        ),
        event(
            "review",
            25,
            55,
            "SUMMARY:Design review with the whole controls team\r\nLOCATION:https://zoom.us/j/123\r\n",
        ),
        event("lunch", 150, 210, "SUMMARY:Lunch\r\n"),
        event("cancelled", 60, 90, "SUMMARY:Cancelled sync\r\nSTATUS:CANCELLED\r\n"),
        event(
            "declined",
            90,
            120,
            "SUMMARY:Vendor call\r\nATTENDEE;PARTSTAT=DECLINED:mailto:selftest@example.com\r\n",
        ),
        format!(
            "BEGIN:VEVENT\r\nUID:offsite\r\nDTSTART;VALUE=DATE:{day}\r\nSUMMARY:Offsite\r\nEND:VEVENT\r\n"
        ),
        "END:VCALENDAR\r\n".to_string(),
    ]
    .concat()
}

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
    let Ok(rd) = std::fs::read_dir(root) else {
        return false;
    };
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
    Page {
        path: "/studio",
        title: "Orchard Studio",
        heading: "Orchard Studio",
        body: "Independent design studio working across research, identity and production. We work alongside the teams we make things for, from the first question to the live release.",
        hue: 0.0,
        letter: "O",
    },
    Page {
        path: "/mail",
        title: "Inbox (3) — Mail",
        heading: "Inbox",
        body: "Three unread: the site visit schedule, a shipping notice, and the October invoice.",
        hue: 0.58,
        letter: "M",
    },
    Page {
        path: "/arena",
        title: "Are.na / Blocks",
        heading: "Blocks",
        body: "A quiet place to collect references: images, links, and notes, arranged into channels.",
        hue: 0.08,
        letter: "A",
    },
    Page {
        path: "/docs",
        title: "WebKitWebView — WebKitGTK 6.0 Reference",
        heading: "WebKitWebView",
        body: "The central class of the WebKitGTK API. It is responsible for managing the drawing of the content and forwarding of events.",
        hue: 0.62,
        letter: "W",
    },
    Page {
        path: "/notes",
        title: "Field notes: thermal storage at desert sites, week 41",
        heading: "Field notes, week 41",
        body: "Charging held steady through the afternoon peak. The storage block reached its setpoint by two, and discharge carried the load well past sunset. Dust on the east array cost about three percent until the Thursday wash.",
        hue: 0.1,
        letter: "N",
    },
    Page {
        path: "/rust",
        title: "Validating References with Lifetimes - The Rust Programming Language",
        heading: "Validating References with Lifetimes",
        body: "Lifetimes are another kind of generic that we’ve already been using. Rather than ensuring that a type has the behavior we want, lifetimes ensure that references are valid as long as we need them to be.",
        hue: 0.03,
        letter: "R",
    },
    Page {
        path: "/long",
        title: "A very long title that goes on and on to check that the column ellipsizes it properly rather than ever scrolling sideways",
        heading: "A long title",
        body: "This page exists to have a title longer than any tab can show.",
        hue: 0.8,
        letter: "L",
    },
    Page {
        path: "/weather",
        title: "Weather — Mojave, 34° and clear",
        heading: "34° and clear",
        body: "Clear skies through the weekend. Winds from the west, eight to twelve miles an hour.",
        hue: 0.14,
        letter: "W",
    },
    Page {
        path: "/tracker",
        title: "Sprint 42 · Tracker",
        heading: "Sprint 42",
        body: "Eleven open, four in review, twenty-three done.",
        hue: 0.48,
        letter: "T",
    },
    Page {
        path: "/design",
        title: "Column spacing — Design review",
        heading: "Column spacing",
        body: "Rows at thirty-four points, a single hairline between kept and today’s tabs.",
        hue: 0.9,
        letter: "D",
    },
    Page {
        path: "/standup",
        title: "Standup notes — Tuesday",
        heading: "Tuesday",
        body: "Shell lands today; spaces after lunch.",
        hue: 0.36,
        letter: "S",
    },
    Page {
        path: "/essay",
        title: "On Restraint in Interface Design",
        heading: "On restraint",
        body: "The best interface is the one you stop noticing. Every border, every badge, every animation has to earn its place against the page it frames.",
        hue: 0.7,
        letter: "E",
    },
    Page {
        path: "/private",
        title: "Private window test",
        heading: "Nothing here is kept",
        body: "This tab has its own cookie jar, and it is forgotten when the tab closes.",
        hue: 0.75,
        letter: "P",
    },
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
        let html = page_html(
            &format!("Crowded tab {n}"),
            &format!("Tab {n}"),
            "One of forty.",
            0,
        );
        ("200 OK", "text/html; charset=utf-8", html.into_bytes())
    } else if path.starts_with("/heavy") {
        (
            "200 OK",
            "text/html; charset=utf-8",
            HEAVY.as_bytes().to_vec(),
        )
    } else if path == CALENDAR {
        ("200 OK", "text/calendar", calendar_ics().into_bytes())
    } else if let Some((i, p)) = PAGES.iter().enumerate().find(|(_, p)| p.path == path) {
        (
            "200 OK",
            "text/html; charset=utf-8",
            page_html(p.title, p.heading, p.body, i).into_bytes(),
        )
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
    snap.append_color(
        &gtk::gdk::RGBA::new(c.r as f32, c.g as f32, c.b as f32, 1.0),
        &rect,
    );
    let layout = shell.window().create_pango_layout(Some(letter));
    let mut font = gtk::pango::FontDescription::from_string("Sans Bold");
    font.set_absolute_size(18.0 * gtk::pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    let (w, h) = layout.pixel_size();
    snap.translate(&gtk::graphene::Point::new(
        (32 - w) as f32 / 2.0,
        (32 - h) as f32 / 2.0,
    ));
    snap.append_layout(&layout, &gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.95));
    snap.pop();
    let Some(node) = snap.to_node() else {
        return Vec::new();
    };
    let Some(renderer) = shell.window().renderer() else {
        return Vec::new();
    };
    renderer
        .render_texture(&node, Some(&rect))
        .save_to_png_bytes()
        .to_vec()
}
