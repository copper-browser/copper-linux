//! Knowing when there is a newer Copper, in the GTK window: GIO's sockets
//! for the check, its toast and Settings › Updates. The check itself — the
//! feed, its cadence, what was seen — is copper-shell-core's
//! [`copper_shell_core::updates`], re-exported here.
//!
//! Inside a Flatpak it does not look at all — updates come through Flatpak.

pub use copper_shell_core::updates::*;

use crate::features::format;
use crate::features::settings::general::flatpak;
use crate::features::settings::{button, group, line};
use crate::platform::Glib;
use crate::platform::Gtk;
use crate::shell::GtkShell;
use crate::shell::Shell;
use copper_core::date::now_unix;
use copper_engine::LocalFuture;
use gio::prelude::*;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Where the file lives.
pub const FEED: &str =
    "https://github.com/copper-browser/copper-linux/releases/latest/download/copper-version.json";
/// The page people download from.
pub const RELEASES: &str = "https://github.com/copper-browser/copper-linux/releases/latest";

/// Copper for Linux's releases.
pub const CHANNEL: Channel = Channel {
    feed: FEED,
    releases: RELEASES,
    tag_page: "https://github.com/copper-browser/copper-linux/releases/tag/",
    system: "Linux",
    managed: flatpak,
};

/// What GTK gives the update check.
struct GtkUpdates;

impl UpdatesApp<Gtk> for GtkUpdates {
    fn channel(&self) -> Channel {
        CHANNEL
    }

    /// GIO's own sockets and TLS — the same glib-networking WebKit uses, so
    /// nothing extra is needed.
    fn exchange(
        &self,
        host: String,
        port: u16,
        https: bool,
        request: Vec<u8>,
    ) -> LocalFuture<Result<Vec<u8>, String>> {
        Box::pin(async move {
            let client = gio::SocketClient::new();
            client.set_tls(https);
            client.set_timeout(20);
            let conn = client
                .connect_to_host_future(&host, port)
                .await
                .map_err(|e| e.message().to_string())?;
            conn.output_stream()
                .write_all_future(request, glib::Priority::DEFAULT)
                .await
                .map_err(|(_, e)| e.message().to_string())?;
            let input = conn.input_stream();
            let mut raw = Vec::new();
            loop {
                let chunk = input
                    .read_bytes_future(16 * 1024, glib::Priority::DEFAULT)
                    .await
                    .map_err(|e| e.message().to_string())?;
                if chunk.is_empty() {
                    break;
                }
                raw.extend_from_slice(&chunk);
                if raw.len() > MAX_ANSWER {
                    return Err("The answer was too big".into());
                }
            }
            let _ = conn.close_future(glib::Priority::DEFAULT).await;
            Ok(raw)
        })
    }

    fn announce(&self, shell: &Shell, v: &str) {
        announce(shell, v);
    }

    fn changed(&self, shell: &Shell) {
        redraw(shell);
    }
}

/// At launch: read `updates.json` and look when due.
pub fn register(shell: &Shell) {
    start::<Gtk>(shell, Rc::new(GtkUpdates));
}

/// One line, with the way to it.
fn announce(shell: &Shell, v: &str) {
    let t = adw::Toast::new(&format!("Copper {v} is available"));
    t.set_button_label(Some("Download"));
    t.set_timeout(10);
    let weak = shell.downgrade();
    t.connect_button_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            open_release(&s);
        }
    });
    shell.ui().toasts.add_toast(t);
}

fn open_release(shell: &Shell) {
    let launcher = gtk::UriLauncher::new(&release_page::<Gtk>());
    launcher.launch(Some(shell.window()), gio::Cancellable::NONE, |r| {
        if let Err(e) = r {
            tracing::warn!(error = %e, "release page");
        }
    });
}

// MARK: - Settings › Updates

struct View {
    group: glib::WeakRef<adw::PreferencesGroup>,
    rows: RefCell<Vec<gtk::Widget>>,
    note: glib::WeakRef<gtk::Label>,
}

thread_local! {
    /// Settings › Updates while it is open.
    static VIEW: RefCell<Option<Rc<View>>> = const { RefCell::new(None) };
}

fn redraw(shell: &Shell) {
    if let Some(v) = VIEW.with(|v| v.borrow().clone()) {
        v.draw(shell);
    }
}

pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    let g = group(Some("This Copper"));
    page.add(&g);
    let note = gtk::Label::new(None);
    note.add_css_class("copper-settings-note");
    note.set_xalign(0.0);
    note.set_wrap(true);
    let notes = group(None);
    notes.add(&note);
    page.add(&notes);
    let view = Rc::new(View {
        group: g.downgrade(),
        rows: RefCell::default(),
        note: note.downgrade(),
    });
    view.draw(shell);
    VIEW.with(|v| *v.borrow_mut() = Some(view));
    page
}

impl View {
    fn draw(&self, shell: &Shell) {
        let (Some(g), Some(note)) = (self.group.upgrade(), self.note.upgrade()) else {
            return;
        };
        for r in self.rows.borrow_mut().drain(..) {
            if let Some(row) = r.downcast_ref::<adw::PreferencesRow>() {
                g.remove(row);
            }
        }
        let mut rows: Vec<gtk::Widget> = Vec::new();
        if flatpak() {
            let r = line(
                &format!("You have Copper {}", version()),
                "Updates come through Flatpak — your software centre, or flatpak update, installs new versions",
            );
            rows.push(r.upcast());
            note.set_text("This Copper is a Flatpak, so it doesn’t look for updates itself.");
        } else {
            let Status {
                checking,
                error,
                latest,
                checked,
            } = status::<Gtk>();
            let newer = latest.as_ref().filter(|l| is_newer(&l.version, version()));
            let status = if checking {
                "Checking…".to_string()
            } else if let Some(l) = newer {
                let mut s = format!("Copper {} is available", l.version);
                if let Some(d) = published(&l.published_at) {
                    s.push_str(&format!(" · {d}"));
                }
                if error.is_some() {
                    s.push_str(" · couldn’t check again just now");
                }
                s
            } else if let Some(e) = &error {
                e.clone()
            } else if latest.is_none() {
                "Not checked yet — Copper looks every six hours on its own".to_string()
            } else {
                "Up to date".to_string()
            };
            let r = line(&format!("You have Copper {}", version()), &status);
            let b = button(if checking { "Checking…" } else { "Check now" }, false);
            b.set_sensitive(!checking);
            let weak = shell.downgrade();
            b.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    check::<Gtk>(&s, true);
                }
            });
            r.add_suffix(&b);
            rows.push(r.upcast());
            if let Some(l) = newer {
                let r = line(
                    &format!("Copper {} is available", l.version),
                    "The .deb and the Flatpak are on the release page. Your package manager installs it — Copper never replaces itself.",
                );
                let d = button("Download", true);
                let weak = shell.downgrade();
                d.connect_clicked(move |_| {
                    if let Some(s) = weak.upgrade() {
                        open_release(&s);
                    }
                });
                r.add_suffix(&d);
                rows.push(r.upcast());
                if let Some(n) = l.notes.as_deref().filter(|n| !n.trim().is_empty()) {
                    rows.push(line(&format!("What’s new in {}", l.version), n).upcast());
                }
            }
            let mut text = "Updates come from github.com/copper-browser/copper-linux".to_string();
            if let Some(t) = checked {
                text.push_str(&format!(
                    " · last checked {}",
                    format::ago::<Glib>(t, now_unix())
                ));
            }
            note.set_text(&text);
        }
        for r in &rows {
            if let Some(row) = r.downcast_ref::<adw::PreferencesRow>() {
                g.add(row);
            }
        }
        *self.rows.borrow_mut() = rows;
    }
}

/// `Oct 12, 2026` from the feed's RFC 3339 time.
fn published(raw: &str) -> Option<String> {
    let t = glib::DateTime::from_iso8601(raw, None).ok()?;
    t.format("%b %-d, %Y").ok().map(|s| s.to_string())
}
