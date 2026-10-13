//! Downloads (Ctrl+Shift+Y; Mac: `Recall.swift` `DownloadsPanel`,
//! `Fork/DownloadsUI.swift`, `docs/downloads.md`): what is still arriving
//! on top, then this session's finished and failed rows, then the files
//! kept from earlier sessions (`downloads.json`).
//!
//! An arriving row wears a ring that fills with its progress (a dashed ring
//! when the size is unknown) and one line of figures with digits that hold
//! still — `5.7 of 10.5 MB · 588 KB/s · 8 s left`; × cancels it and takes
//! the partial file. A finished row opens its file; hover for Show in Files
//! (the desktop's file manager, `org.freedesktop.FileManager1.ShowItems`)
//! and a remove-from-list ×. A failed row offers Retry. Removing a row never
//! deletes a file.

use super::{Refresh, close, escape_closes, showing};
use crate::features::format;
use crate::features::ui;
use crate::shell::Shell;
use copper_core::downloads::{DownloadId, DownloadState, Keep, LiveDownload};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

struct DownloadsPanel {
    frame: ui::Frame,
    search: gtk::SearchEntry,
    content: gtk::Box,
    count: gtk::Label,
    clear: gtk::Button,
    /// The rows drawn last, by live id, for updating figures in place.
    live: RefCell<Vec<(DownloadId, LiveRow)>>,
    /// The shape drawn last (ids and states), to redraw only when it changes.
    shape: RefCell<String>,
}

/// The parts of an arriving row that move.
#[derive(Clone)]
struct LiveRow {
    ring: gtk::DrawingArea,
    fraction: Rc<Cell<Option<f64>>>,
    detail: gtk::Label,
    bar: gtk::ProgressBar,
}

thread_local! {
    static PANEL: RefCell<Option<Rc<DownloadsPanel>>> = const { RefCell::new(None) };
    static TICK: Cell<bool> = const { Cell::new(false) };
    static DOOR: RefCell<DoorState> = RefCell::default();
}

/// Keep the panel in step with the downloads.
pub fn watch(shell: &Shell) {
    let weak = shell.downgrade();
    shell.downloads().watch(move || {
        let Some(s) = weak.upgrade() else { return };
        doors(&s);
        if showing("downloads") {
            if let Some(p) = PANEL.with(|p| p.borrow().clone()) {
                p.update(&s);
            }
            s.downloads().seen();
        }
        ensure_tick(&s);
    });
}

/// While anything arrives, the figures are re-read twice a second (speed
/// and time left move between engine events).
fn ensure_tick(shell: &Shell) {
    if TICK.with(|t| t.replace(true)) {
        return;
    }
    let weak = shell.downgrade();
    glib::timeout_add_local(Duration::from_millis(500), move || {
        let Some(s) = weak.upgrade() else {
            TICK.with(|t| t.set(false));
            return glib::ControlFlow::Break;
        };
        let active = s.downloads().model(&s).active_count() > 0;
        doors(&s);
        if showing("downloads")
            && let Some(p) = PANEL.with(|p| p.borrow().clone())
        {
            p.update(&s);
        }
        if !active {
            TICK.with(|t| t.set(false));
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

pub fn build(shell: &Shell) -> (gtk::Widget, Refresh) {
    let weak = shell.downgrade();
    let frame = ui::frame("Downloads", move || {
        if let Some(s) = weak.upgrade() {
            close(&s);
        }
    });
    let search = ui::search("Search downloads");
    frame.body.append(&search);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.body.append(&ui::scroller(&content));
    let count = ui::count("");
    count.set_hexpand(true);
    count.set_ellipsize(gtk::pango::EllipsizeMode::End);
    frame.foot.append(&count);
    let folder = ui::pill("Open folder", false);
    let weak = shell.downgrade();
    folder.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            let dir = s.downloads().folder(&s);
            let _ = std::fs::create_dir_all(&dir);
            open_path(&s, &dir);
        }
    });
    frame.foot.append(&folder);
    let clear = ui::pill("Clear list", false);
    clear.set_tooltip_text(Some("The files stay where they are"));
    let weak = shell.downgrade();
    clear.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            s.downloads().clear(&s);
        }
    });
    frame.foot.append(&clear);
    escape_closes(shell, &frame.root);

    let panel = Rc::new(DownloadsPanel {
        frame,
        search,
        content,
        count,
        clear,
        live: RefCell::default(),
        shape: RefCell::default(),
    });
    let weak = shell.downgrade();
    let p = panel.clone();
    panel.search.connect_search_changed(move |_| {
        if let Some(s) = weak.upgrade() {
            p.draw(&s);
        }
    });
    PANEL.with(|x| *x.borrow_mut() = Some(panel.clone()));
    let root = panel.frame.root.clone().upcast();
    let p = panel.clone();
    let refresh: Refresh = Rc::new(move |s: &Shell| {
        s.downloads().seen();
        p.draw(s);
    });
    (root, refresh)
}

/// What a row shows, read out of the model so no borrow is held while
/// drawing.
enum Line {
    Live(LiveDownload, String),
    Kept(Keep),
}

impl DownloadsPanel {
    fn lines(&self, shell: &Shell) -> (Vec<Line>, usize, usize) {
        let typed = self.search.text().trim().to_lowercase();
        let hit = |name: &str, from: &str| {
            typed.is_empty()
                || name.to_lowercase().contains(&typed)
                || from.to_lowercase().contains(&typed)
        };
        let model = shell.downloads().model(shell);
        let live: Vec<LiveDownload> = model.live().to_vec();
        let paths: Vec<&str> = live.iter().filter_map(|d| d.path.as_deref()).collect();
        let kept: Vec<Keep> = model
            .kept()
            .iter()
            .filter(|k| !paths.contains(&k.path.as_str()))
            .cloned()
            .collect();
        let total = live.len() + kept.len();
        let active = model.active_count();
        drop(model);
        let mut out = Vec::new();
        // Arriving first, then this session's, newest first (the model's order).
        let mut ordered = live.clone();
        ordered.sort_by_key(|d| d.state != DownloadState::Active);
        for d in ordered {
            let from = crate::shell::host_of(&d.source_url);
            if hit(&d.name, &from) {
                out.push(Line::Live(d, from));
            }
        }
        for k in kept {
            if hit(&k.name, &k.from) {
                out.push(Line::Kept(k));
            }
        }
        (out, total, active)
    }

    /// Redraw when rows came or went or changed state; otherwise only the
    /// moving figures.
    fn update(self: &Rc<Self>, shell: &Shell) {
        let (lines, _, _) = self.lines(shell);
        if shape(&lines) != *self.shape.borrow() {
            self.draw(shell);
            return;
        }
        let rows = self.live.borrow().clone();
        for line in &lines {
            if let Line::Live(d, from) = line
                && let Some((_, r)) = rows.iter().find(|(id, _)| *id == d.id)
            {
                r.detail.set_text(&format::download_line(d, from));
                let f = d.fraction();
                r.fraction.set(f);
                r.ring.queue_draw();
                if let Some(f) = f {
                    r.bar.set_fraction(f);
                } else {
                    r.bar.pulse();
                }
            }
        }
    }

    fn draw(self: &Rc<Self>, shell: &Shell) {
        ui::clear(&self.content);
        self.live.borrow_mut().clear();
        let (lines, total, active) = self.lines(shell);
        *self.shape.borrow_mut() = shape(&lines);
        self.clear.set_sensitive(total > active);
        let folder = shell.downloads().folder(shell);
        let folder_name = folder
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| folder.to_string_lossy().to_string());
        self.count.set_text(&if total == 0 {
            format!("Files land in {folder_name}")
        } else if active > 0 {
            format!("{active} arriving")
        } else if total == 1 {
            "1 file".to_string()
        } else {
            format!("{total} files")
        });
        if lines.is_empty() {
            self.content.append(&ui::empty(if total == 0 {
                "Nothing downloaded yet."
            } else {
                "Nothing matches."
            }));
            return;
        }
        let list = ui::list();
        let weak = shell.downgrade();
        list.connect_row_activated(move |_, row| {
            let Some(s) = weak.upgrade() else { return };
            let path = row.widget_name().to_string();
            if !path.is_empty() && Path::new(&path).exists() {
                open_path(&s, Path::new(&path));
            }
        });
        for line in &lines {
            let row = match line {
                Line::Live(d, from) => self.live_row(shell, d, from),
                Line::Kept(k) => self.kept_row(shell, k),
            };
            list.append(&row);
        }
        self.content.append(&list);
    }

    fn live_row(self: &Rc<Self>, shell: &Shell, d: &LiveDownload, from: &str) -> gtk::ListBoxRow {
        let running = d.state == DownloadState::Active;
        let fraction = Rc::new(Cell::new(d.fraction()));
        let ring = ring(fraction.clone());
        let mark: gtk::Widget = if running {
            ring.clone().upcast()
        } else {
            file_mark(&d.name, matches!(d.state, DownloadState::Failed { .. }))
        };
        let r = ui::row(&mark, &d.name, &format::download_line(d, from));
        r.detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let exists = d.path.as_deref().is_some_and(|p| Path::new(p).exists());
        if d.state == DownloadState::Finished && exists {
            r.row.set_widget_name(d.path.as_deref().unwrap_or(""));
            r.row.set_tooltip_text(Some("Open"));
        } else {
            r.row.set_activatable(false);
        }
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("bar");
        bar.set_fraction(d.fraction().unwrap_or(0.0));
        bar.set_visible(running);
        r.texts.append(&bar);
        let id = d.id;
        match &d.state {
            DownloadState::Active => {
                let x = ui::icon_button("window-close-symbolic", "Cancel download");
                x.add_css_class("keep");
                let weak = shell.downgrade();
                x.connect_clicked(move |_| {
                    if let Some(s) = weak.upgrade() {
                        s.downloads().cancel(id);
                    }
                });
                r.end.append(&x);
            }
            DownloadState::Failed { reason } => {
                r.row.set_tooltip_text(Some(reason));
                let retry = ui::pill("Retry", false);
                let weak = shell.downgrade();
                retry.connect_clicked(move |_| {
                    if let Some(s) = weak.upgrade() {
                        s.downloads().retry(&s, id);
                    }
                });
                r.end.append(&retry);
                r.end.append(&self.remove_button(shell, Remove::Live(id)));
            }
            _ => {
                if exists && let Some(p) = &d.path {
                    r.end.append(&show_in_files(shell, p));
                }
                r.end.append(&self.remove_button(shell, Remove::Live(id)));
            }
        }
        if running {
            self.live.borrow_mut().push((
                id,
                LiveRow {
                    ring,
                    fraction,
                    detail: r.detail.clone(),
                    bar,
                },
            ));
        }
        r.row
    }

    fn kept_row(self: &Rc<Self>, shell: &Shell, k: &Keep) -> gtk::ListBoxRow {
        let exists = k.still_there();
        let now = copper_core::date::now_unix();
        let when = format::ago(k.date.unix(), now);
        let detail = if k.from.is_empty() {
            when
        } else {
            format!("{} · {when}", k.from)
        };
        let r = ui::row(&file_mark(&k.name, false), &k.name, &detail);
        if exists {
            r.row.set_widget_name(&k.path);
            r.row.set_tooltip_text(Some("Open"));
            r.end.append(&show_in_files(shell, &k.path));
        } else {
            r.row.add_css_class("missing");
            r.row.set_activatable(false);
            r.row.set_tooltip_text(Some("No longer where it was saved"));
        }
        r.end
            .append(&self.remove_button(shell, Remove::Kept(k.path.clone())));
        r.row
    }

    fn remove_button(self: &Rc<Self>, shell: &Shell, what: Remove) -> gtk::Button {
        let x = ui::icon_button("window-close-symbolic", "Remove from list");
        let weak = shell.downgrade();
        x.connect_clicked(move |_| {
            let Some(s) = weak.upgrade() else { return };
            match &what {
                Remove::Live(id) => s.downloads().remove(*id),
                Remove::Kept(path) => s.downloads().forget(&s, path),
            }
        });
        x
    }
}

enum Remove {
    Live(DownloadId),
    Kept(String),
}

/// Ids and states, in order: a change here is a redraw.
fn shape(lines: &[Line]) -> String {
    let mut s = String::new();
    for l in lines {
        match l {
            Line::Live(d, _) => {
                s.push_str(&format!("{}:{:?}:{};", d.id.0, d.state, d.path.is_some()))
            }
            Line::Kept(k) => s.push_str(&format!("k:{};", k.path)),
        }
    }
    s
}

/// The ring an arriving row wears: an arc that fills, or a dashed circle
/// when the size is unknown (never a false percentage).
fn ring(fraction: Rc<Cell<Option<f64>>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(18);
    area.set_accessible_role(gtk::AccessibleRole::ProgressBar);
    area.set_draw_func(move |area, cr, w, h| {
        let ink = area.color();
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let r = (w.min(h) as f64 / 2.0) - 1.5;
        cr.set_line_width(2.0);
        cr.set_source_rgba(
            ink.red() as f64,
            ink.green() as f64,
            ink.blue() as f64,
            0.16,
        );
        match fraction.get() {
            Some(f) => {
                cr.arc(cx, cy, r, 0.0, 2.0 * PI);
                let _ = cr.stroke();
                cr.set_source_rgba(
                    ink.red() as f64,
                    ink.green() as f64,
                    ink.blue() as f64,
                    0.85,
                );
                cr.set_line_cap(gtk::cairo::LineCap::Round);
                let end = -PI / 2.0 + 2.0 * PI * f.clamp(0.0, 1.0);
                if f > 0.0 {
                    cr.arc(cx, cy, r, -PI / 2.0, end);
                    let _ = cr.stroke();
                }
            }
            None => {
                cr.set_source_rgba(ink.red() as f64, ink.green() as f64, ink.blue() as f64, 0.6);
                cr.set_dash(&[2.5, 3.0], 0.0);
                cr.arc(cx, cy, r, 0.0, 2.0 * PI);
                let _ = cr.stroke();
            }
        }
        // The arrow inside.
        cr.set_dash(&[], 0.0);
        cr.set_line_width(1.4);
        cr.set_source_rgba(
            ink.red() as f64,
            ink.green() as f64,
            ink.blue() as f64,
            0.75,
        );
        cr.move_to(cx, cy - 3.5);
        cr.line_to(cx, cy + 3.0);
        cr.move_to(cx - 2.8, cy + 0.4);
        cr.line_to(cx, cy + 3.2);
        cr.line_to(cx + 2.8, cy + 0.4);
        let _ = cr.stroke();
    });
    area
}

/// A file's mark: a symbolic icon for its kind (the theme's own symbolic
/// set, so it takes the panel's ink in light and dark alike).
fn file_mark(name: &str, failed: bool) -> gtk::Widget {
    let icon = if failed {
        "dialog-warning-symbolic"
    } else {
        kind_icon(name)
    };
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(16);
    image.add_css_class("file-mark");
    image.set_size_request(18, 18);
    image.upcast()
}

fn kind_icon(name: &str) -> &'static str {
    let (kind, _) = gio::content_type_guess(Some(name), None::<&[u8]>);
    let mime = gio::content_type_get_mime_type(&kind)
        .map(|m| m.to_string())
        .unwrap_or_default();
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if mime.starts_with("image/") {
        "image-x-generic-symbolic"
    } else if mime.starts_with("video/") {
        "video-x-generic-symbolic"
    } else if mime.starts_with("audio/") {
        "audio-x-generic-symbolic"
    } else if matches!(
        ext.as_str(),
        "zip"
            | "gz"
            | "tgz"
            | "xz"
            | "bz2"
            | "zst"
            | "7z"
            | "rar"
            | "tar"
            | "deb"
            | "rpm"
            | "flatpak"
    ) {
        "package-x-generic-symbolic"
    } else if mime == "application/pdf" || mime.starts_with("text/") || mime.contains("document") {
        "x-office-document-symbolic"
    } else {
        "text-x-generic-symbolic"
    }
}

fn show_in_files(shell: &Shell, path: &str) -> gtk::Button {
    let b = ui::icon_button("folder-open-symbolic", "Show in Files");
    let weak = shell.downgrade();
    let path = path.to_string();
    b.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            reveal(&s, Path::new(&path));
        }
    });
    b
}

/// Open a file or folder with the desktop's choice of app.
pub fn open_path(shell: &Shell, path: &Path) {
    let file = gio::File::for_path(path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    let weak = shell.downgrade();
    launcher.launch(Some(shell.window()), gio::Cancellable::NONE, move |r| {
        if let Err(e) = r {
            tracing::warn!(error = %e, "open");
            if let Some(s) = weak.upgrade() {
                s.toast("No app here opens that");
            }
        }
    });
}

/// Show `path` selected in the desktop's file manager
/// (`org.freedesktop.FileManager1.ShowItems`); without one, the folder opens.
pub fn reveal(shell: &Shell, path: &Path) {
    let uri = gio::File::for_path(path).uri().to_string();
    let weak = shell.downgrade();
    let folder = path.parent().map(Path::to_path_buf);
    gio::bus_get(gio::BusType::Session, gio::Cancellable::NONE, move |bus| {
        let Ok(bus) = bus else {
            fallback(&weak, folder.as_deref());
            return;
        };
        let args = (vec![uri], String::new()).to_variant();
        bus.call(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
            move |r| {
                if let Err(e) = r {
                    tracing::info!(error = %e, "no file manager on the bus; opening the folder");
                    fallback(&weak, folder.as_deref());
                }
            },
        );
    });
}

fn fallback(weak: &crate::shell::WeakShell, folder: Option<&Path>) {
    if let (Some(s), Some(dir)) = (weak.upgrade(), folder) {
        open_path(&s, dir);
    }
}

// MARK: - the door

/// What every door shows (one at the column's foot, one at the end of the
/// tab row).
#[derive(Default)]
struct DoorState {
    doors: Vec<(glib::WeakRef<gtk::Button>, glib::WeakRef<gtk::DrawingArea>)>,
    /// Shown once anything was downloaded this session; then it stays.
    shown: bool,
    /// Arriving last time it looked (to catch the moment the last lands).
    was_active: bool,
    /// The check is up until then.
    pulse_until: Option<std::time::Instant>,
    look: Look,
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
enum Look {
    #[default]
    Resting,
    Arriving(Option<f64>),
    Landed,
    Failed,
}

/// The door (Mac `DownloadsDoor`): not there until a download starts; a ring
/// that fills with everything arriving; a check when the last lands; then a
/// resting arrow with a dot for what you have not looked at; an exclamation
/// in ember after a failure. Click for the Downloads panel.
pub fn door(shell: &Shell) -> gtk::Widget {
    let area = gtk::DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(18);
    let button = gtk::Button::new();
    button.set_child(Some(&area));
    button.add_css_class("foot-button");
    button.add_css_class("flat");
    button.add_css_class("copper-door");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some("Downloads"));
    button.update_property(&[gtk::accessible::Property::Label("Downloads")]);
    let weak = shell.downgrade();
    button.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            super::toggle(&s, "downloads");
            s.downloads().seen();
            doors(&s);
        }
    });
    let weak = shell.downgrade();
    area.set_draw_func(move |area, cr, w, h| {
        let Some(s) = weak.upgrade() else { return };
        let (look, unseen) = (DOOR.with(|d| d.borrow().look), unseen(&s));
        draw_door(area, cr, w, h, look, unseen);
    });
    let shown = DOOR.with(|d| d.borrow().shown);
    button.set_visible(shown);
    DOOR.with(|d| {
        let mut d = d.borrow_mut();
        d.doors.retain(|(b, _)| b.upgrade().is_some());
        d.doors.push((button.downgrade(), area.downgrade()));
    });
    button.upcast()
}

fn unseen(shell: &Shell) -> bool {
    shell
        .downloads()
        .model(shell)
        .live()
        .iter()
        .any(|d| d.unseen)
}

/// Bring every door in line with the downloads.
fn doors(shell: &Shell) {
    let (live_any, active, fraction, speed, failed) = {
        let m = shell.downloads().model(shell);
        let speed: f64 = m
            .live()
            .iter()
            .filter(|d| d.state == DownloadState::Active)
            .map(|d| d.speed)
            .sum();
        (
            !m.live().is_empty(),
            m.active_count(),
            m.total_fraction(),
            speed,
            m.live()
                .iter()
                .any(|d| matches!(d.state, DownloadState::Failed { .. })),
        )
    };
    let now = std::time::Instant::now();
    let look = DOOR.with(|d| {
        let mut d = d.borrow_mut();
        if live_any {
            d.shown = true;
        }
        if d.was_active && active == 0 && !failed {
            d.pulse_until = Some(now + Duration::from_millis(1400));
        }
        d.was_active = active > 0;
        d.look = if active > 0 {
            Look::Arriving(fraction)
        } else if d.pulse_until.is_some_and(|t| now < t) {
            Look::Landed
        } else if failed {
            Look::Failed
        } else {
            Look::Resting
        };
        d.look
    });
    if look == Look::Landed {
        let weak = shell.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(1450), move || {
            if let Some(s) = weak.upgrade() {
                doors(&s);
            }
        });
    }
    let tip = if active > 0 {
        let n = if active == 1 {
            "1 download".to_string()
        } else {
            format!("{active} downloads")
        };
        format!("{n} · {}", format::speed(speed))
    } else if failed {
        "Downloads — one failed".to_string()
    } else {
        "Downloads".to_string()
    };
    let (shown, list) = DOOR.with(|d| {
        let d = d.borrow();
        (d.shown, d.doors.clone())
    });
    for (b, a) in list {
        if let Some(b) = b.upgrade() {
            b.set_visible(shown);
            b.set_tooltip_text(Some(&tip));
        }
        if let Some(a) = a.upgrade() {
            a.queue_draw();
        }
    }
}

fn draw_door(
    area: &gtk::DrawingArea,
    cr: &gtk::cairo::Context,
    w: i32,
    h: i32,
    look: Look,
    unseen: bool,
) {
    let ink = area.color();
    let (r, g, b) = (ink.red() as f64, ink.green() as f64, ink.blue() as f64);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let rad = (w.min(h) as f64 / 2.0) - 1.5;
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    let arrow = |cr: &gtk::cairo::Context, a: f64| {
        cr.set_dash(&[], 0.0);
        cr.set_line_width(1.5);
        cr.set_source_rgba(r, g, b, a);
        cr.move_to(cx, cy - 4.0);
        cr.line_to(cx, cy + 3.5);
        cr.move_to(cx - 3.2, cy + 0.6);
        cr.line_to(cx, cy + 3.7);
        cr.line_to(cx + 3.2, cy + 0.6);
        let _ = cr.stroke();
    };
    match look {
        Look::Arriving(f) => {
            cr.set_line_width(2.0);
            cr.set_source_rgba(r, g, b, 0.16);
            cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
            let _ = cr.stroke();
            match f {
                Some(f) => {
                    cr.set_source_rgba(r, g, b, 0.85);
                    if f > 0.0 {
                        cr.arc(
                            cx,
                            cy,
                            rad,
                            -PI / 2.0,
                            -PI / 2.0 + 2.0 * PI * f.clamp(0.0, 1.0),
                        );
                        let _ = cr.stroke();
                    }
                }
                None => {
                    cr.set_source_rgba(r, g, b, 0.6);
                    cr.set_dash(&[2.5, 3.0], 0.0);
                    cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
                    let _ = cr.stroke();
                }
            }
            arrow(cr, 0.8);
        }
        Look::Landed => {
            cr.set_line_width(2.0);
            cr.set_source_rgba(r, g, b, 0.85);
            cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
            let _ = cr.stroke();
            cr.set_line_width(1.6);
            cr.move_to(cx - 3.6, cy + 0.2);
            cr.line_to(cx - 1.0, cy + 2.8);
            cr.line_to(cx + 3.8, cy - 2.6);
            let _ = cr.stroke();
        }
        Look::Failed => {
            // The ember a hot tab wears.
            cr.set_source_rgba(0.83, 0.36, 0.16, 1.0);
            cr.set_line_width(1.5);
            cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
            let _ = cr.stroke();
            cr.set_line_width(1.8);
            cr.move_to(cx, cy - 4.0);
            cr.line_to(cx, cy + 0.8);
            let _ = cr.stroke();
            cr.arc(cx, cy + 3.6, 1.0, 0.0, 2.0 * PI);
            let _ = cr.fill();
        }
        Look::Resting => {
            cr.set_line_width(1.4);
            cr.set_source_rgba(r, g, b, 0.55);
            cr.arc(cx, cy, rad, 0.0, 2.0 * PI);
            let _ = cr.stroke();
            arrow(cr, 0.7);
            if unseen {
                cr.set_source_rgba(r, g, b, 0.9);
                cr.arc(cx + rad * 0.72, cy - rad * 0.72, 2.4, 0.0, 2.0 * PI);
                let _ = cr.fill();
            }
        }
    }
}
