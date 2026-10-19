//! copper-shell-core on GTK: the [`Gtk`] platform — WebKitGTK pages, GLib's
//! main loop ([`Glib`]) and the libadwaita window as the shell's view
//! ([`GtkView`], with each tab's page in a [`GtkFrame`]).
//!
//! The shell model (tabs, pages, sessions, downloads, the control API) is
//! copper-shell-core's; this module is the glue that draws it: every
//! [`ShellView`] call lands on the widgets in [`crate::window`].

use crate::actions::Registry;
use crate::shell::Shell;
use crate::window::{Refresh, Ui};
use copper_core::TabId;
use copper_core::settings::{Look, TabsPosition};
use copper_engine_webkitgtk::{WebKitEngine, WebKitPage};
use copper_shell_core::format::{Calendar, Civil};
use copper_shell_core::shell::pages::PageSlot;
use copper_shell_core::{MainLoop, PageFrame, PageStatus, Platform, Question, ShellView};
use gtk4 as gtk;
use gtk4::gdk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{OnceCell, RefCell};
use std::future::Future;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

/// The Linux app: WebKitGTK, GLib, GTK4 + libadwaita.
pub enum Gtk {}

impl Platform for Gtk {
    type Engine = WebKitEngine;
    type Loop = Glib;
    type View = GtkView;
    type Icon = gdk::Texture;
    type Frame = GtkFrame;

    /// GLib's real name, unless it has none (empty, or GLib's "Unknown"):
    /// then the login name.
    fn user_name() -> String {
        let real = glib::real_name().to_string_lossy().to_string();
        if real.trim().is_empty() || real == "Unknown" {
            glib::user_name().to_string_lossy().to_string()
        } else {
            real
        }
    }
}

/// GLib's default main context, which GTK runs.
pub enum Glib {}

impl MainLoop for Glib {
    fn spawn_local(future: impl Future<Output = ()> + 'static) {
        glib::spawn_future_local(future);
    }

    fn sleep(duration: Duration) -> impl Future<Output = ()> + 'static {
        glib::timeout_future(duration)
    }

    fn post(f: impl FnOnce() + Send + 'static) {
        glib::MainContext::default().invoke(f);
    }

    fn is_main_thread() -> bool {
        glib::MainContext::default().is_owner()
    }

    fn idle(f: impl FnOnce() + 'static) {
        glib::idle_add_local_once(f);
    }
}

/// GLib's local time zone and the locale's month names (`%B`).
impl Calendar for Glib {
    fn civil(unix: f64) -> Option<Civil> {
        let d = glib::DateTime::from_unix_local(unix as i64).ok()?;
        Some(Civil {
            year: d.year(),
            month: d.month() as u32,
            day: d.day_of_month() as u32,
            day_of_year: d.day_of_year() as u32,
            hour: d.hour() as u32,
            minute: d.minute() as u32,
        })
    }

    fn month_name(month: u32) -> String {
        glib::DateTime::from_local(2000, month as i32, 1, 12, 0, 0.0)
            .ok()
            .and_then(|d| d.format("%B").ok())
            .map(|s| s.to_string())
            .unwrap_or_default()
    }
}

/// The window: one `adw::ApplicationWindow` and every widget the shell
/// talks to ([`Ui`]), plus what the lanes registered ([`Registry`]).
///
/// The widgets need the shell to exist, so they come second: the shell
/// is made with this view, then [`GtkView::set_ui`] completes it. Until
/// then the view ignores what the shell tells it.
pub struct GtkView {
    app: adw::Application,
    window: adw::ApplicationWindow,
    ui: OnceCell<Ui>,
    registry: RefCell<Registry>,
}

impl GtkView {
    /// The window of `app` (bigger for scripted runs, which screenshot it).
    pub fn new(app: &adw::Application, options: &crate::options::Options) -> GtkView {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Copper")
            .default_width(if options.scripted() { 1440 } else { 1280 })
            .default_height(if options.scripted() { 900 } else { 820 })
            .build();
        window.add_css_class("copper");
        GtkView {
            app: app.clone(),
            window,
            ui: OnceCell::new(),
            registry: RefCell::new(Registry::default()),
        }
    }

    /// The widgets are built: from now on the view draws.
    pub fn set_ui(&self, ui: Ui) {
        if self.ui.set(ui).is_err() {
            tracing::error!("the window's widgets were built twice");
        }
    }

    /// The widgets ([`GtkView::set_ui`] has run: everything after
    /// `Shell::new` in [`crate::shell::build`]).
    pub fn ui(&self) -> &Ui {
        self.ui.get().expect("ui is built in shell::build")
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    pub fn app(&self) -> &adw::Application {
        &self.app
    }

    /// What the lanes added: commands, settings pages, panels, menu items.
    pub fn registry(&self) -> &RefCell<Registry> {
        &self.registry
    }
}

impl ShellView<Gtk> for GtkView {
    fn new_frame(&self, shell: &Shell, tab: &TabId, page: &WebKitPage) -> GtkFrame {
        GtkFrame::new(shell, tab, page)
    }

    fn page_added(&self, _shell: &Shell, slot: &Rc<PageSlot<Gtk>>) {
        if let Some(ui) = self.ui.get() {
            ui.stage.add_page(slot);
        }
    }

    fn page_removed(&self, _shell: &Shell, slot: &Rc<PageSlot<Gtk>>) {
        if let Some(ui) = self.ui.get() {
            ui.stage.remove_page(slot);
        }
    }

    fn show_tab(&self, shell: &Shell, tab: Option<&TabId>) {
        let Some(ui) = self.ui.get() else { return };
        match tab {
            Some(id) => {
                ui.stage.show(shell, Some(id));
                if ui.card.is_auto() {
                    ui.card.close(shell);
                }
            }
            None => {
                ui.stage.show(shell, None);
                if !ui.card.is_open() {
                    ui.card.open_empty(shell);
                }
            }
        }
    }

    fn refresh(&self, shell: &Shell, what: &Refresh) {
        if let Some(ui) = self.ui.get() {
            ui.refresh(shell, what);
        }
    }

    fn set_layout(&self, shell: &Shell, position: TabsPosition) {
        if let Some(ui) = self.ui.get() {
            ui.set_layout(shell, position);
        }
    }

    fn set_progress(&self, loading: bool, fraction: f64) {
        if let Some(ui) = self.ui.get() {
            ui.stage.set_progress(loading, fraction);
        }
    }

    fn set_hover(&self, url: Option<&str>) {
        if let Some(ui) = self.ui.get() {
            ui.stage.set_hover(url);
        }
    }

    fn apply_look(&self, look: Look) {
        let scheme = match look {
            Look::Light => adw::ColorScheme::ForceLight,
            Look::Dark => adw::ColorScheme::ForceDark,
            Look::System => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    fn is_dark(&self) -> bool {
        adw::StyleManager::default().is_dark()
    }

    fn toast(&self, text: &str) {
        if let Some(ui) = self.ui.get() {
            ui.toast(text);
        }
    }

    fn ask(&self, _shell: &Shell, question: Question, answer: Box<dyn FnOnce(bool)>) {
        let dialog = adw::AlertDialog::new(Some(&question.title), question.detail.as_deref());
        dialog.add_response("deny", &question.deny);
        dialog.add_response("allow", &question.allow);
        dialog.set_default_response(Some("deny"));
        dialog.set_close_response("deny");
        let answer = RefCell::new(Some(answer));
        dialog.connect_response(None, move |_, response| {
            if let Some(answer) = answer.borrow_mut().take() {
                answer(response == "allow");
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Closing the window writes everything; a light/dark switch of the
    /// system redraws the tints. Then the window shows.
    fn present(&self, shell: &Shell) {
        let weak = shell.downgrade();
        self.window.connect_close_request(move |_| {
            if let Some(s) = weak.upgrade() {
                s.shutdown();
            }
            glib::Propagation::Proceed
        });
        let weak = shell.downgrade();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(s) = weak.upgrade() {
                s.dark_changed();
            }
        });
        self.window.present();
    }

    fn is_online(&self) -> bool {
        gio::NetworkMonitor::default().is_network_available()
    }

    fn showing_downloads(&self) -> bool {
        crate::features::panels::downloads_showing()
    }

    fn hide_panel(&self) {
        if let Some(ui) = self.ui.get() {
            ui.stage.hide_panel();
        }
    }

    fn screenshot(&self) -> Option<Vec<u8>> {
        crate::screenshot::window_png_bytes(self.window.upcast_ref())
    }

    fn open_settings(&self, shell: &Shell, page: &str) {
        crate::window::settings::dialog(shell).set_visible_page_name(page);
    }

    fn resize_window(&self, width: i32, height: i32) {
        self.window.set_default_size(width, height);
    }

    fn decode_icon(&self, png: &[u8]) -> Option<gdk::Texture> {
        gdk::Texture::from_bytes(&glib::Bytes::from(png)).ok()
    }

    fn load_icon(&self, path: &Path) -> Option<gdk::Texture> {
        gdk::Texture::from_filename(path).ok()
    }
}

/// A tab's page on the stage: the page's widget in an overlay, with the
/// crash / load-error state laid over it when there is one.
///
/// Derefs to the overlay, the widget the stack holds and that lanes lay
/// their own overlays on (the sign-in offer, the agent's pill).
pub struct GtkFrame {
    overlay: gtk::Overlay,
    status: gtk::Box,
    status_page: adw::StatusPage,
    status_button: gtk::Button,
}

impl GtkFrame {
    fn new(shell: &Shell, tab: &TabId, page: &WebKitPage) -> GtkFrame {
        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        let view = page.widget();
        view.set_hexpand(true);
        view.set_vexpand(true);
        overlay.set_child(Some(&view));

        let status_page = adw::StatusPage::new();
        status_page.add_css_class("compact");
        let status_button = gtk::Button::with_label("Reload");
        status_button.add_css_class("pill");
        status_button.set_halign(gtk::Align::Center);
        status_page.set_child(Some(&status_button));
        let status = gtk::Box::new(gtk::Orientation::Vertical, 0);
        status.add_css_class("copper-status");
        status.append(&status_page);
        status_page.set_vexpand(true);
        status.set_visible(false);
        overlay.add_overlay(&status);

        let weak = shell.downgrade();
        let tab = tab.clone();
        status_button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.reload(&tab);
            }
        });

        GtkFrame {
            overlay,
            status,
            status_page,
            status_button,
        }
    }

    /// The overlay the stack holds.
    pub fn overlay(&self) -> &gtk::Overlay {
        &self.overlay
    }
}

impl std::ops::Deref for GtkFrame {
    type Target = gtk::Overlay;

    fn deref(&self) -> &gtk::Overlay {
        &self.overlay
    }
}

impl PageFrame for GtkFrame {
    fn show_status(&self, status: &PageStatus) {
        self.status_page.set_title(&status.title);
        self.status_page.set_description(Some(&status.description));
        self.status_button.set_label(&status.button);
        self.status.set_visible(true);
    }

    fn hide_status(&self) {
        self.status.set_visible(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_shell_core::format;

    /// The calendar says what GLib's own `%H:%M` / `%-d %B %Y` say, as the
    /// panels did before dates moved to copper-shell-core.
    #[test]
    fn glib_calendar_matches_glib_formatting() {
        for unix in [0.0, 951_782_400.0, 1_709_307_660.0, 1_735_689_599.0] {
            let d = glib::DateTime::from_unix_local(unix as i64).unwrap();
            assert_eq!(
                format::clock::<Glib>(unix),
                d.format("%H:%M").unwrap().as_str()
            );
            assert_eq!(
                format::day_number::<Glib>(unix),
                d.year() as i64 * 1000 + d.day_of_year() as i64
            );
            assert_eq!(
                format::day::<Glib>(unix),
                d.format("%-d %B %Y").unwrap().as_str()
            );
        }
    }
}
