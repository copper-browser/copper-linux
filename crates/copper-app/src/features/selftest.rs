//! The features lane's end-to-end run (`COPPER_SELFTEST=features`, in a
//! `COPPER_WORLD`, on a display): the real window, a local fixture site, and
//! checks printed as `PASS`/`FAIL` lines plus screenshots in
//! `COPPER_SELFTEST_OUT` (default `/tmp/copper-features`).
//!
//! * the ad blocker on a page with ten known trackers (blocked counts, the
//!   cosmetic slots), then off for that site, then on again;
//! * hide anything through the real picker (a trusted click), the element
//!   still hidden after a reload, `hidden.json` on disk;
//! * reading mode on an article;
//! * History, Bookmarks, Downloads panels, light and dark;
//! * a real download from a throttled local server
//!   (`COPPER_SLOW_DOWNLOAD=http://127.0.0.1:<port>`, copper-ref
//!   `docs/fixtures/slow-download.py`): progress, then finished;
//! * the settings pages; the update check against a local feed.

use super::{curtain, panels, reader, shield, updates};
use crate::platform::Gtk;
use crate::shell::GtkShell;
use crate::shell::{OpenTab, Shell};
use copper_core::TabId;
use copper_core::downloads::DownloadState;
use copper_core::settings::Look;
use copper_engine::{InputEvent, JsonValue, PageView, World};
use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Ten of the Mac's unwanted domains, one tracker each on the fixture page.
const TEN: [&str; 10] = [
    "doubleclick.net",
    "googlesyndication.com",
    "google-analytics.com",
    "googletagmanager.com",
    "amazon-adsystem.com",
    "criteo.com",
    "taboola.com",
    "hotjar.com",
    "connect.facebook.net",
    "scorecardresearch.com",
];

pub fn maybe_start(shell: &Shell) {
    if std::env::var("COPPER_SELFTEST").as_deref() != Ok("features") {
        return;
    }
    let out = std::env::var("COPPER_SELFTEST_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/copper-features"));
    let _ = std::fs::create_dir_all(&out);
    // A fixed port (COPPER_FIXTURE_PORT, default 47811), so the run can
    // point COPPER_UPDATE_FEED at its /feed.json before launch.
    let server = Server::start();
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        crate::app::settle(&weak, Duration::from_secs(8)).await;
        let Some(shell) = weak.upgrade() else { return };
        let run = Rc::new(Run {
            shell: shell.clone(),
            out,
            server,
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
    server: Server,
    failures: RefCell<Vec<String>>,
}

impl Run {
    fn check(&self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.failures.borrow_mut().push(what.to_string());
        }
    }

    fn note(&self, what: &str) {
        println!("NOTE {what}");
    }

    async fn pause(&self, ms: u64) {
        glib::timeout_future(Duration::from_millis(ms)).await;
    }

    async fn shot(&self, name: &str) {
        self.pause(700).await;
        let path = self.out.join(format!("{name}.png"));
        let mut last = None;
        for _ in 0..4 {
            match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
                Ok(()) => {
                    println!("SHOT {}", path.display());
                    return;
                }
                Err(e) => {
                    let w = self.shell.window();
                    println!(
                        "NOTE screenshot {name}: {e} (window {}x{} mapped {} realized {})",
                        w.width(),
                        w.height(),
                        w.is_mapped(),
                        w.is_realized()
                    );
                    last = Some(e);
                    // A layout still pending makes an empty snapshot: nudge
                    // the frame clock (headless weston can leave it idle).
                    w.queue_resize();
                    w.queue_draw();
                    if let Some(fc) = w.frame_clock() {
                        fc.request_phase(
                            gtk4::gdk::FrameClockPhase::LAYOUT | gtk4::gdk::FrameClockPhase::PAINT,
                        );
                    }
                    self.pause(800).await;
                }
            }
        }
        if let Some(e) = last {
            self.check(false, &format!("screenshot {name}: {e}"));
        }
    }

    fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.server.port)
    }

    fn open(&self, url: &str) -> TabId {
        self.shell.open(
            url,
            OpenTab {
                foreground: true,
                ..Default::default()
            },
        )
    }

    async fn loaded(&self, id: &TabId) {
        let start = Instant::now();
        // A beat for the load to start at all.
        self.pause(150).await;
        loop {
            if let Some(slot) = self.shell.slot(id)
                && !slot.loading.get()
                && !slot.page.is_loading()
                && slot.page.url().is_some()
            {
                break;
            }
            if start.elapsed() > Duration::from_secs(15) {
                self.note(&format!("{id} still loading after 15 s"));
                break;
            }
            self.pause(60).await;
        }
        self.pause(250).await;
    }

    async fn eval(&self, id: &TabId, js: &str) -> JsonValue {
        let Some(slot) = self.shell.slot(id) else {
            return JsonValue::Null;
        };
        slot.page
            .eval(js, &World::Page)
            .await
            .unwrap_or(JsonValue::Null)
    }

    async fn all(self: &Rc<Self>) {
        self.shell.set_look(Look::Light);
        self.shield().await;
        self.hide().await;
        self.reader().await;
        self.panels(false).await;
        self.downloads().await;
        self.settings().await;
        self.shell.set_look(Look::Dark);
        self.pause(400).await;
        self.panels(true).await;
        self.settings_dark().await;
        self.updates().await;
    }

    // MARK: the ad blocker

    async fn shield(&self) {
        let start = Instant::now();
        while shield::current_list::<Gtk>().is_none() && start.elapsed() < Duration::from_secs(30) {
            self.pause(100).await;
        }
        self.check(
            shield::current_list::<Gtk>().is_some(),
            "shield: the list compiled at launch",
        );
        self.note(&format!(
            "shield: list {} ready after {} ms",
            shield::current_list::<Gtk>().unwrap_or_default(),
            start.elapsed().as_millis()
        ));

        self.server.clear();
        let id = self.open(&self.url("localhost", "/shield"));
        self.loaded(&id).await;
        let seen = self.server.trackers_seen();
        let ran = self.eval(&id, "JSON.stringify(window.ran || [])").await;
        let slots = self
            .eval(
                &id,
                "JSON.stringify(['ins','google_ads_iframe_1','div-gpt-ad-123','keep'].map(function(i){var e=document.getElementById(i);return e?getComputedStyle(e).display:'gone'}))",
            )
            .await;
        println!(
            "shield on: trackers requested {}/10 {:?}; ran {ran}; slots {slots}",
            seen.len(),
            seen
        );
        self.check(
            seen.is_empty(),
            &format!(
                "shield on: 10/10 trackers blocked ({} requested)",
                seen.len()
            ),
        );
        self.check(
            self.server.saw("/t/control.js"),
            "shield on: the first-party control script loaded",
        );
        let s = slots.as_str().unwrap_or("");
        self.check(
            s == r#"["none","none","none","block"]"#,
            &format!("shield on: ad slots hidden, the article kept ({s})"),
        );
        self.shot("10-shield-fixture").await;

        // Off for this site: the page reloads and every tracker loads.
        self.server.clear();
        shield::set_paused(&self.shell, "localhost", true);
        let t = Instant::now();
        while (shield::busy::<Gtk>() || self.server.trackers_seen().len() < 10)
            && t.elapsed() < Duration::from_secs(15)
        {
            self.pause(100).await;
        }
        self.loaded(&id).await;
        let seen = self.server.trackers_seen();
        println!(
            "shield paused on localhost: trackers requested {}/10",
            seen.len()
        );
        self.check(
            seen.len() == 10,
            &format!(
                "shield off for the site: 10/10 trackers load ({})",
                seen.len()
            ),
        );
        self.check(
            self.shell.settings().shield_paused() == vec!["localhost".to_string()],
            "shield.paused stored as the Mac keys it",
        );

        // Back on.
        self.server.clear();
        shield::set_paused(&self.shell, "localhost", false);
        let t = Instant::now();
        while shield::busy::<Gtk>() && t.elapsed() < Duration::from_secs(15) {
            self.pause(100).await;
        }
        self.pause(600).await;
        self.loaded(&id).await;
        let seen = self.server.trackers_seen();
        println!("shield back on: trackers requested {}/10", seen.len());
        self.check(
            seen.is_empty(),
            &format!(
                "shield back on: 10/10 blocked again ({} requested)",
                seen.len()
            ),
        );

        // Off everywhere, then on.
        self.server.clear();
        shield::set_enabled(&self.shell, false);
        self.shell.reload_active();
        self.loaded(&id).await;
        let off = self.server.trackers_seen().len();
        shield::set_enabled(&self.shell, true);
        self.server.clear();
        self.shell.reload_active();
        self.loaded(&id).await;
        let on = self.server.trackers_seen().len();
        println!("shield switched off: {off}/10 requested; on again: {on}/10 requested");
        self.check(
            off == 10 && on == 0,
            "shield switch: off loads all ten, on blocks all ten",
        );
    }

    // MARK: hide anything

    async fn hide(&self) {
        let id = self.open(&self.url("127.0.0.1", "/hide"));
        self.loaded(&id).await;
        let before = self
            .eval(
                &id,
                "getComputedStyle(document.getElementById('cookie-bar')).display",
            )
            .await;
        self.check(
            before.as_str() == Some("flex"),
            &format!("hide: the cookie bar shows at first ({before})"),
        );
        curtain::toggle(&self.shell);
        self.pause(300).await;
        self.check(
            curtain::picking::<Gtk>(&id),
            "hide: Ctrl+Shift+H arms the picker",
        );
        // A trusted click in the middle of the cookie bar.
        let rect = self
            .eval(
                &id,
                "(function(){var r=document.getElementById('cookie-bar').getBoundingClientRect();return [r.left+r.width/2, r.top+r.height/2]})()",
            )
            .await;
        let (x, y) = (
            rect.get(0).and_then(JsonValue::as_f64).unwrap_or(0.0),
            rect.get(1).and_then(JsonValue::as_f64).unwrap_or(0.0),
        );
        if let Some(slot) = self.shell.slot(&id) {
            let _ = slot
                .page
                .input(InputEvent::Move {
                    x,
                    y,
                    modifiers: Default::default(),
                })
                .await;
            self.pause(150).await;
            self.shot("20-hide-picking").await;
            match slot.page.input(InputEvent::click(x, y)).await {
                Ok(trust) => self.note(&format!("hide: click delivered ({trust:?})")),
                Err(e) => self.note(&format!("hide: click failed: {e}")),
            }
        }
        let t = Instant::now();
        while curtain::veils::<Gtk>("127.0.0.1").is_empty() && t.elapsed() < Duration::from_secs(5)
        {
            self.pause(100).await;
        }
        let veils = curtain::veils::<Gtk>("127.0.0.1");
        println!(
            "hide: hidden on 127.0.0.1: {:?}",
            veils
                .iter()
                .map(|v| (&v.selector, &v.label))
                .collect::<Vec<_>>()
        );
        self.check(
            veils.iter().any(|v| v.selector.contains("cookie-bar")),
            "hide: clicking the cookie bar hides it",
        );
        let now = self
            .eval(
                &id,
                "getComputedStyle(document.getElementById('cookie-bar')).display",
            )
            .await;
        self.check(
            now.as_str() == Some("none"),
            &format!("hide: gone at once ({now})"),
        );
        curtain::stop(&self.shell, &id);
        self.pause(200).await;

        // Reload: still hidden, from the first paint (the sheet is in before
        // any script runs, so the page's own first look already finds it hidden).
        self.shell.reload_active();
        self.loaded(&id).await;
        let after = self
            .eval(
                &id,
                "getComputedStyle(document.getElementById('cookie-bar')).display",
            )
            .await;
        let first = self.eval(&id, "window.firstLook").await;
        println!("hide: after reload display={after} first-look={first}");
        self.check(
            after.as_str() == Some("none"),
            "hide: still hidden after a reload",
        );
        self.check(
            first.as_str() == Some("none"),
            &format!("hide: hidden before the page's first script ran ({first})"),
        );
        self.shot("21-hide-after-reload").await;
        // A page on another site is untouched.
        let other = self.open(&self.url("localhost", "/hide"));
        self.loaded(&other).await;
        let there = self
            .eval(
                &other,
                "getComputedStyle(document.getElementById('cookie-bar')).display",
            )
            .await;
        self.check(
            there.as_str() == Some("flex"),
            &format!("hide: other sites untouched ({there})"),
        );
        self.shell.close(&other);

        self.shell.save_now();
        self.pause(800).await;
        let file = self.shell.store().paths().data.join("hidden.json");
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        println!("hide: {} = {text}", file.display());
        self.check(
            text.contains("\"127.0.0.1\"") && text.contains("cookie-bar"),
            "hide: hidden.json keeps it (Mac shape: host → [veil])",
        );
        panels::show(&self.shell, "hidden");
        self.pause(300).await;
        self.shot("22-hidden-panel").await;
        panels::close(&self.shell);
    }

    // MARK: reading mode

    async fn reader(&self) {
        let id = self.open(&self.url("127.0.0.1", "/article"));
        self.loaded(&id).await;
        self.shot("30-article-page").await;
        reader::toggle(&self.shell);
        let t = Instant::now();
        while !reader::reading(&id) && t.elapsed() < Duration::from_secs(5) {
            self.pause(100).await;
        }
        let has = self
            .eval(
                &id,
                "!!document.getElementById('office-reader') && !document.querySelector('nav')",
            )
            .await;
        self.check(
            reader::reading(&id) && has == JsonValue::Bool(true),
            "reader: the article, nothing around it",
        );
        self.shot("31-reader-view").await;
        reader::toggle(&self.shell);
        self.loaded(&id).await;
        let back = self.eval(&id, "!!document.querySelector('nav')").await;
        self.check(
            !reader::reading(&id) && back == JsonValue::Bool(true),
            "reader: again puts the page back",
        );
        // A page with nothing to read says so.
        let empty = self.open(&self.url("127.0.0.1", "/hide"));
        self.loaded(&empty).await;
        reader::toggle(&self.shell);
        self.pause(500).await;
        self.check(
            !reader::reading(&empty),
            "reader: a page without an article stays as it is",
        );
        self.shell.close(&empty);
    }

    // MARK: panels

    async fn panels(&self, dark: bool) {
        let suffix = if dark { "dark" } else { "light" };
        if !dark {
            // Some history and bookmarks to show.
            for p in ["/notes", "/essay", "/docs"] {
                let id = self.open(&self.url("127.0.0.1", p));
                self.loaded(&id).await;
            }
            let id = self.open(&self.url("127.0.0.1", "/article"));
            self.loaded(&id).await;
            super::panels::bookmarks::bookmark_current(&self.shell);
            let n = self.shell.bookmarks().count();
            super::panels::bookmarks::bookmark_current(&self.shell);
            self.check(
                self.shell.bookmarks().count() == n,
                "bookmarks: Ctrl+D twice keeps one",
            );
            {
                let mut b = self.shell.bookmarks_mut();
                let kids = vec![
                    copper_core::bookmarks::Bookmark::site(
                        "Field notes",
                        &self.url("127.0.0.1", "/notes"),
                    ),
                    copper_core::bookmarks::Bookmark::site(
                        "The essay",
                        &self.url("127.0.0.1", "/essay"),
                    ),
                ];
                b.insert(
                    copper_core::bookmarks::Bookmark::folder("Reading", kids),
                    None,
                );
                b.add(&self.url("127.0.0.1", "/docs"), "Reference");
            }
            super::panels::bookmarks::save(&self.shell);
        }
        panels::show(&self.shell, "history");
        self.pause(300).await;
        self.check(
            panels::showing("history"),
            "history: Ctrl+Y shows the panel",
        );
        self.shot(&format!("40-history-{suffix}")).await;
        if !dark {
            // Search narrows.
            if let Some(entry) = find_entry(&self.shell, "Search everywhere you have been") {
                entry.set_text("essay");
                // Headless weston has no seat: a focused field with text
                // cannot be snapshotted there (gdk_seat_get_keyboard).
                gtk4::prelude::GtkWindowExt::set_focus(self.shell.window(), None::<&gtk4::Widget>);
                self.pause(400).await;
                self.shot("41-history-search").await;
                entry.set_text("");
            }
        }
        panels::show(&self.shell, "bookmarks");
        self.pause(300).await;
        self.shot(&format!("42-bookmarks-{suffix}")).await;
        panels::show(&self.shell, "downloads");
        self.pause(300).await;
        self.shot(&format!("43-downloads-{suffix}")).await;
        panels::close(&self.shell);
        self.pause(300).await;
    }

    // MARK: a real download

    async fn downloads(&self) {
        let Ok(base) = std::env::var("COPPER_SLOW_DOWNLOAD") else {
            self.check(
                false,
                "downloads: COPPER_SLOW_DOWNLOAD (slow-download.py) not given",
            );
            return;
        };
        let url = format!("{base}/big?mb=8&kbps=1600");
        panels::show(&self.shell, "downloads");
        let doors_before = visible_with_class(&self.shell, "copper-door");
        let started = self.shell.downloads().start(&self.shell, &url);
        self.check(started, "downloads: started from the active tab");
        let t = Instant::now();
        let mut mid: Option<(f64, String)> = None;
        let mut shot = false;
        let mut fractions = Vec::new();
        loop {
            self.pause(250).await;
            let (state, fraction, line) = {
                let m = self.shell.downloads().model(&self.shell);
                match m.live().first() {
                    Some(d) => (
                        Some(d.state.clone()),
                        d.fraction(),
                        super::format::download_line(d, ""),
                    ),
                    None => (None, None, String::new()),
                }
            };
            if let Some(f) = fraction {
                fractions.push(f);
            }
            if state == Some(DownloadState::Active)
                && let Some(f) = fraction
                && f > 0.25
                && !shot
            {
                mid = Some((f, line.clone()));
                println!("downloads: mid-way {:.0}% — {line}", f * 100.0);
                self.shot("50-download-progress").await;
                shot = true;
            }
            if matches!(
                state,
                Some(DownloadState::Finished) | Some(DownloadState::Failed { .. })
            ) || t.elapsed() > Duration::from_secs(40)
            {
                println!(
                    "downloads: ended {state:?} after {:.1} s — {line}",
                    t.elapsed().as_secs_f64()
                );
                break;
            }
        }
        let rising = fractions.windows(2).all(|w| w[1] >= w[0]) && fractions.len() >= 4;
        self.check(
            mid.as_ref().is_some_and(|(_, l)| {
                l.contains(" of ")
                    && l.contains("/s")
                    && (l.contains("left") || l.contains("starting"))
            }),
            &format!(
                "downloads: progress with size, speed and time left ({:?})",
                mid.map(|m| m.1)
            ),
        );
        self.check(
            rising,
            &format!(
                "downloads: progress only rises ({} samples)",
                fractions.len()
            ),
        );
        let (state, path) = {
            let m = self.shell.downloads().model(&self.shell);
            m.live()
                .first()
                .map(|d| (d.state.clone(), d.path.clone()))
                .unwrap_or((DownloadState::Cancelled, None))
        };
        self.check(
            state == DownloadState::Finished,
            &format!("downloads: finished ({state:?})"),
        );
        let doors_after = visible_with_class(&self.shell, "copper-door");
        println!("downloads: doors visible before {doors_before}, after {doors_after}");
        self.check(
            doors_before == 0 && doors_after >= 1,
            "downloads: the door appears once something downloads",
        );
        let size = path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        println!("downloads: file {path:?} size {size:?}");
        self.check(
            size == Some(8 * 1024 * 1024),
            "downloads: the whole file is on disk",
        );
        self.pause(400).await;
        self.shot("51-download-finished").await;
        // Show in Files: the desktop's file manager over D-Bus.
        if let (Some(p), Ok(log)) = (&path, std::env::var("COPPER_FILEMANAGER_LOG")) {
            super::panels::downloads::reveal(&self.shell, std::path::Path::new(p));
            let uri = gio::File::for_path(p).uri().to_string();
            let t = Instant::now();
            let mut said = String::new();
            while t.elapsed() < Duration::from_secs(5) {
                self.pause(150).await;
                said = std::fs::read_to_string(&log).unwrap_or_default();
                if said.contains(&uri) {
                    break;
                }
            }
            println!("downloads: file manager heard: {}", said.trim());
            self.check(
                said.contains(&format!("ShowItems {uri}")),
                "downloads: Show in Files asks org.freedesktop.FileManager1.ShowItems",
            );
        }
        self.shell.save_now();
        self.pause(800).await;
        let file = self.shell.store().paths().data.join("downloads.json");
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        println!("downloads: {} = {text}", file.display());
        self.check(
            text.contains("\"name\":\"big") && text.contains("\"date\""),
            "downloads: downloads.json keeps it",
        );

        // Cancel one part way.
        let url = format!("{base}/big?mb=20&kbps=800&n=2");
        self.shell.downloads().start(&self.shell, &url);
        let t = Instant::now();
        let mut id = None;
        while t.elapsed() < Duration::from_secs(10) {
            self.pause(200).await;
            let m = self.shell.downloads().model(&self.shell);
            if let Some(d) = m
                .live()
                .iter()
                .find(|d| d.state == DownloadState::Active && d.received > 0)
            {
                id = Some((d.id, d.path.clone()));
                break;
            }
        }
        if let Some((rid, path)) = id {
            self.shell.downloads().cancel(rid);
            self.pause(800).await;
            let gone = self.shell.downloads().model(&self.shell).get(rid).is_none();
            let partial = path
                .as_ref()
                .is_some_and(|p| std::path::Path::new(p).exists());
            self.check(
                gone && !partial,
                "downloads: × cancels and takes the partial file",
            );
        } else {
            self.check(false, "downloads: second download never started");
        }
        // A failure, then Retry.
        let url = format!("{base}/fail?mb=4&at=0.3&kbps=4000&n={}", std::process::id());
        self.shell.downloads().start(&self.shell, &url);
        let t = Instant::now();
        let mut failed = None;
        while t.elapsed() < Duration::from_secs(15) {
            self.pause(200).await;
            let m = self.shell.downloads().model(&self.shell);
            if let Some(d) = m
                .live()
                .iter()
                .find(|d| matches!(d.state, DownloadState::Failed { .. }))
            {
                failed = Some(d.id);
                break;
            }
        }
        self.check(
            failed.is_some(),
            "downloads: a broken transfer shows as failed",
        );
        if failed.is_some() {
            self.shot("52-download-failed").await;
        }
        if let Some(fid) = failed {
            let ok = self.shell.downloads().retry(&self.shell, fid);
            let t = Instant::now();
            let mut done = false;
            while ok && t.elapsed() < Duration::from_secs(15) {
                self.pause(200).await;
                let m = self.shell.downloads().model(&self.shell);
                if m.live()
                    .iter()
                    .any(|d| d.name.starts_with("fail") && d.state == DownloadState::Finished)
                {
                    done = true;
                    break;
                }
            }
            self.check(ok && done, "downloads: Retry starts over and finishes");
        }
        self.shot("53-downloads-panel").await;
        let widths = panels::widths();
        println!("panels: widths {widths:?}");
        self.check(
            widths
                .iter()
                .all(|(_, min, nat)| *min <= 360 && *nat <= 360),
            "panels: nothing pushes the side panel past 360 px",
        );
        panels::close(&self.shell);
    }

    // MARK: settings

    async fn settings(&self) {
        let dialog = crate::window::settings::dialog(&self.shell);
        for (name, shot) in [
            ("general", "60-settings-general"),
            ("tabs", "61-settings-tabs"),
            ("appearance", "62-settings-appearance"),
            ("privacy", "63-settings-privacy"),
            ("extensions", "64-settings-extensions"),
            ("updates", "65-settings-updates"),
        ] {
            dialog.set_visible_page_name(name);
            self.pause(400).await;
            self.shot(shot).await;
        }
        dialog.close();
        self.pause(300).await;
    }

    async fn settings_dark(&self) {
        let dialog = crate::window::settings::dialog(&self.shell);
        for (name, shot) in [
            ("privacy", "66-settings-privacy-dark"),
            ("appearance", "67-settings-appearance-dark"),
        ] {
            dialog.set_visible_page_name(name);
            self.pause(400).await;
            self.shot(shot).await;
        }
        dialog.close();
        self.pause(300).await;
    }

    // MARK: updates

    async fn updates(&self) {
        if std::env::var_os("COPPER_UPDATE_FEED").is_none() {
            self.note("updates: COPPER_UPDATE_FEED not set; skipped");
            return;
        }
        updates::check::<Gtk>(&self.shell, true);
        let ok = updates::wait_checked::<Gtk>(Duration::from_secs(10)).await;
        let avail = updates::available::<Gtk>();
        println!(
            "updates: checked {ok}, available {avail:?}, page {}",
            updates::release_page::<Gtk>()
        );
        self.check(
            avail.as_deref() == Some("9.9.9"),
            "updates: a newer version in the feed is offered",
        );
        self.check(
            updates::release_page::<Gtk>()
                == "https://github.com/copper-browser/copper-linux/releases/tag/v9.9.9",
            "updates: Download opens the release page",
        );
        self.shot("70-update-toast").await;
        let dialog = crate::window::settings::dialog(&self.shell);
        dialog.set_visible_page_name("updates");
        self.pause(500).await;
        self.shot("71-settings-updates-available").await;
        dialog.close();
        // Against GitHub itself (no release yet is a fine answer).
        match updates::get::<Gtk>(updates::FEED).await {
            Ok(b) => self.note(&format!("updates: GitHub feed answered {} bytes", b.len())),
            Err(e) => self.note(&format!("updates: GitHub feed: {e}")),
        }
    }
}

/// How many mapped widgets wear `class`.
fn visible_with_class(shell: &Shell, class: &str) -> usize {
    fn walk(w: &gtk4::Widget, class: &str, n: &mut usize) {
        if w.has_css_class(class) && w.is_mapped() {
            *n += 1;
        }
        let mut c = w.first_child();
        while let Some(x) = c {
            walk(&x, class, n);
            c = x.next_sibling();
        }
    }
    let mut n = 0;
    walk(shell.window().upcast_ref(), class, &mut n);
    n
}

fn find_entry(shell: &Shell, placeholder: &str) -> Option<gtk4::SearchEntry> {
    fn walk(w: &gtk4::Widget, p: &str) -> Option<gtk4::SearchEntry> {
        if let Some(e) = w.downcast_ref::<gtk4::SearchEntry>()
            && e.placeholder_text().as_deref() == Some(p)
            && e.is_mapped()
        {
            return Some(e.clone());
        }
        let mut c = w.first_child();
        while let Some(x) = c {
            if let Some(f) = walk(&x, p) {
                return Some(f);
            }
            c = x.next_sibling();
        }
        None
    }
    walk(shell.window().upcast_ref(), placeholder)
}

// MARK: - the fixture site

#[derive(Clone)]
struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Self {
        let port: u16 = std::env::var("COPPER_FIXTURE_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(47811);
        let listener = std::net::TcpListener::bind(("127.0.0.1", port))
            .or_else(|_| std::net::TcpListener::bind("127.0.0.1:0"))
            .expect("bind");
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = log.clone();
                std::thread::spawn(move || answer(stream, port, &log));
            }
        });
        Server { port, seen }
    }

    fn clear(&self) {
        if let Ok(mut s) = self.seen.lock() {
            s.clear();
        }
    }

    fn saw(&self, path: &str) -> bool {
        self.seen
            .lock()
            .map(|s| s.iter().any(|p| p == path))
            .unwrap_or(false)
    }

    fn trackers_seen(&self) -> BTreeSet<String> {
        self.seen
            .lock()
            .map(|s| {
                s.iter()
                    .filter_map(|p| p.strip_prefix("/t/"))
                    .map(|p| p.trim_end_matches(".js").to_string())
                    .filter(|p| p != "control")
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn answer(stream: std::net::TcpStream, port: u16, log: &Arc<Mutex<Vec<String>>>) {
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).unwrap_or(0) <= 2 {
            break;
        }
    }
    if let Ok(mut l) = log.lock() {
        l.push(path.clone());
    }
    let (kind, body) = respond(&path, port);
    let mut s = stream;
    let _ = write!(
        s,
        "HTTP/1.1 {}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        if kind.is_empty() {
            "404 Not Found"
        } else {
            "200 OK"
        },
        body.len()
    );
    let _ = s.write_all(body.as_bytes());
}

const STYLE: &str = r#"<style>
:root { color-scheme: light; }
body { margin: 0; font: 17px/1.65 "Cantarell", "Noto Sans", system-ui, sans-serif; color: #111; background: #fff; }
nav { display: flex; gap: 22px; padding: 18px 40px; border-bottom: 1px solid #eee; font-size: 14px; }
nav a { color: #555; text-decoration: none; }
main { max-width: 640px; margin: 48px auto; padding: 0 32px; }
h1 { font-size: 30px; letter-spacing: -0.015em; margin: 0 0 12px; }
p { color: #333; }
aside { position: fixed; right: 24px; top: 120px; width: 200px; font-size: 13px; color: #666; }
@media (max-width: 1100px) { aside { display: none; } }
aside a { display: block; color: #666; margin: 6px 0; }
#cookie-bar { position: fixed; left: 0; right: 0; bottom: 0; display: flex; align-items: center; gap: 18px;
  padding: 18px 40px; background: #1b1b1b; color: #f1f1f1; font-size: 14px; }
#cookie-bar button { margin-left: auto; background: #fff; color: #111; border: 0; border-radius: 99px; padding: 8px 18px; }
.ad { margin: 24px 0; padding: 24px; background: #fbeaea; color: #8a1c1c; text-align: center; }
</style>"#;

fn respond(path: &str, port: u16) -> (&'static str, String) {
    if let Some(name) = path.strip_prefix("/t/") {
        let name = name.trim_end_matches(".js");
        return (
            "text/javascript",
            format!("window.ran = (window.ran || []).concat([{name:?}]);"),
        );
    }
    let path = path.split('?').next().unwrap_or("/");
    match path {
        "/shield" => {
            let mut scripts = String::new();
            for domain in TEN {
                scripts.push_str(&format!(
                    "<script src=\"http://{domain}@127.0.0.1:{port}/t/{domain}.js\"></script>\n"
                ));
            }
            scripts.push_str("<script src=\"/t/control.js\"></script>\n");
            (
                "text/html; charset=utf-8",
                format!(
                    "<!doctype html><html><head><meta charset=utf-8><title>Tracker fixture</title>{STYLE}</head><body>\
                     <nav><a href=/>Fixture</a><a href=/notes>Notes</a></nav><main>\
                     <h1>Ten trackers</h1><p>This page asks for ten known third-party trackers and one first-party script.</p>\
                     <ins class=\"adsbygoogle ad\" id=\"ins\">Advertisement (adsbygoogle)</ins>\
                     <div class=ad id=\"google_ads_iframe_1\">Advertisement (google_ads_)</div>\
                     <div class=ad id=\"div-gpt-ad-123\">Advertisement (div-gpt-ad)</div>\
                     <div class=\"article\" id=\"keep\"><p>The article stays.</p></div></main>\n{scripts}</body></html>"
                ),
            )
        }
        "/hide" => (
            "text/html; charset=utf-8",
            format!(
                "<!doctype html><html><head><meta charset=utf-8><title>Hide fixture</title>{STYLE}</head><body>\
                 <nav><a href=/>Fixture</a><a href=/notes>Notes</a></nav>\
                 <main><h1>Something in the way</h1><p>A cookie bar sits along the bottom of this page.</p></main>\
                 <div id=\"cookie-bar\"><span>We use cookies to make this site worse.</span><button>Accept all</button></div>\
                 <script>window.firstLook = getComputedStyle(document.getElementById('cookie-bar')).display;</script>\
                 </body></html>"
            ),
        ),
        "/article" => {
            let para = "The heuristic is the old one and it holds up: the piece of the page carrying the most prose, punished for every link inside it — because navigation, related-articles rails and comment threads are all made of links, and prose is not. ";
            let mut body = String::new();
            for i in 0..7 {
                body.push_str(&format!(
                    "<p>{}{}</p>",
                    if i == 2 { "Reading mode keeps only this. " } else { "" },
                    para.repeat(2)
                ));
                if i == 3 {
                    body.push_str("<h2>A quieter page</h2>");
                }
            }
            (
                "text/html; charset=utf-8",
                format!(
                    "<!doctype html><html><head><meta charset=utf-8><title>On reading</title>{STYLE}</head><body>\
                     <nav><a href=/>Home</a><a href=/notes>Notes</a><a href=/essay>Essays</a><a href=/docs>Reference</a></nav>\
                     <aside><b>Related</b><a href=/a>Ten tabs you need</a><a href=/b>Why sidebars</a><a href=/c>Subscribe</a></aside>\
                     <main><article><h1>On reading in a browser</h1>{body}</article>\
                     <div class=ad>Advertisement</div></main></body></html>"
                ),
            )
        }
        "/notes" | "/essay" | "/docs" | "/" => {
            let title = match path {
                "/notes" => "Field notes",
                "/essay" => "The essay",
                "/docs" => "Reference",
                _ => "Fixture",
            };
            (
                "text/html; charset=utf-8",
                format!(
                    "<!doctype html><html><head><meta charset=utf-8><title>{title}</title>{STYLE}</head><body>\
                     <main><h1>{title}</h1><p>A page to have been to.</p></main></body></html>"
                ),
            )
        }
        "/feed.json" => (
            "application/json",
            r#"{"schemaVersion":1,"product":"copper","platform":"linux","version":"9.9.9","releaseId":"v9.9.9","commit":"abc","publishedAt":"2026-10-12T00:00:00Z","latestUrl":"https://github.com/copper-browser/copper-linux/releases/latest","notes":"Quieter downloads and a calmer reader.","assets":[]}"#.to_string(),
        ),
        _ => ("", "not found".to_string()),
    }
}
