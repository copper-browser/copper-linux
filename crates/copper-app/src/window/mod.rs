//! The window: tabs in a column (default, like the Mac) or a row, the slim
//! header with the field, the stage, the card.
//!
//! ```text
//! ToastOverlay
//! └ Overlay ──────────────── card layer (Ctrl+T / Ctrl+K)
//!   └ Box ─ column (tabs on the left) │ Box ─ topbar (tabs on top)
//!                                     │     ─ header (field, tabs on the left)
//!                                     │     ─ stage (pages, progress, find, panels)
//! ```

pub mod column;
pub mod dropzone;
pub mod header;
pub mod peek;
pub mod settings;
pub mod stage;
pub mod tabs;
pub mod topbar;

use crate::palette::{Card, Mode};
use crate::shell::GtkShell;
use crate::shell::Shell;
use crate::spaces::colour::Tint;
use copper_core::settings::TabsPosition;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What needs redrawing (the shell says; [`Ui::refresh`] draws it).
pub use copper_shell_core::Refresh;
pub use stage::Stage;

/// Every widget the shell talks to.
pub struct Ui {
    pub toasts: adw::ToastOverlay,
    pub column: Rc<column::Column>,
    pub header: Rc<header::Header>,
    pub topbar: Rc<topbar::TopBar>,
    pub stage: Rc<Stage>,
    pub card: Rc<Card>,
    shortcuts: gtk::ShortcutController,
    tint: gtk::CssProvider,
    tint_key: RefCell<String>,
    layout: Cell<TabsPosition>,
}

impl Ui {
    pub fn build(shell: &Shell) -> Ui {
        install_base_css();
        let tint = gtk::CssProvider::new();
        gtk::style_context_add_provider_for_display(
            &gtk::gdk::Display::default().expect("a display"),
            &tint,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );

        let column = column::Column::new(shell);
        let header = header::Header::new(shell);
        let topbar = topbar::TopBar::new(shell);
        let stage = Stage::new(shell);
        let card = Card::new(shell);

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.set_hexpand(true);
        main.append(&topbar.widget);
        main.append(&header.widget);
        main.append(&stage.widget);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.add_css_class("copper-root");
        body.append(&column.widget);
        body.append(&main);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&body));
        overlay.add_overlay(&header.dropdown);
        header.set_root(&overlay);
        overlay.add_overlay(&card.layer);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&overlay));
        shell.window().set_content(Some(&toasts));

        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_propagation_phase(gtk::PropagationPhase::Capture);
        shortcuts.set_scope(gtk::ShortcutScope::Global);
        shell.window().add_controller(shortcuts.clone());

        // Ctrl let go, wherever the keyboard is (a page included): a
        // most-recent Ctrl+Tab walk lands where it stopped.
        let ctrl = gtk::EventControllerKey::new();
        ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = shell.downgrade();
        ctrl.connect_key_released(move |_, key, _, _| {
            use gtk::gdk::Key;
            if (key == Key::Control_L || key == Key::Control_R)
                && let Some(s) = weak.upgrade()
            {
                s.tab_cycle_ended();
            }
        });
        shell.window().add_controller(ctrl);

        // Escape belongs to whatever is on top (card, find, field); only
        // when nothing claims it does it stop a loading page.
        let esc = gtk::ShortcutController::new();
        esc.set_propagation_phase(gtk::PropagationPhase::Bubble);
        esc.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(window_action("win.escape")),
        ));
        shell.window().add_controller(esc);

        Ui {
            toasts,
            column,
            header,
            topbar,
            stage,
            card,
            shortcuts,
            tint,
            tint_key: RefCell::default(),
            layout: Cell::new(TabsPosition::Left),
        }
    }

    /// A capture-phase shortcut for `action` (`<Control>t`).
    pub fn add_shortcut(&self, accel: &str, action: &str) {
        let Some(trigger) = gtk::ShortcutTrigger::parse_string(accel) else {
            tracing::warn!(accel, "bad shortcut");
            return;
        };
        self.shortcuts.add_shortcut(gtk::Shortcut::new(
            Some(trigger),
            Some(window_action(action)),
        ));
    }

    /// Tabs down the left or across the top.
    pub fn set_layout(&self, shell: &Shell, p: TabsPosition) {
        self.layout.set(p);
        let left = p == TabsPosition::Left;
        self.column.widget.set_visible(left);
        self.header.widget.set_visible(left);
        self.topbar.widget.set_visible(!left);
        let _ = shell;
    }

    pub fn layout(&self) -> TabsPosition {
        self.layout.get()
    }

    /// Bring every widget in line with the shell.
    pub fn refresh(&self, shell: &Shell, r: &Refresh) {
        if r.all || r.spaces {
            self.apply_tint(shell);
            self.column.rebuild_space(shell);
            self.topbar.rebuild_space(shell);
        }
        if r.all || r.tabs {
            self.column.rebuild_tabs(shell);
            self.topbar.rebuild_tabs(shell);
        } else {
            if r.selection {
                self.column.mark_live(shell);
                self.topbar.rebuild_tabs(shell);
            }
            for id in &r.changed {
                self.column.update_tab(shell, id);
                self.topbar.update_tab(shell, id);
            }
        }
        if r.all || r.chrome || r.selection || r.tabs {
            let private = shell.active_is_private();
            let widgets: [&gtk::Widget; 2] = [
                self.column.widget.upcast_ref(),
                self.topbar.widget.upcast_ref(),
            ];
            for w in widgets {
                if private {
                    w.add_css_class("private");
                } else {
                    w.remove_css_class("private");
                }
            }
            self.header.update(shell);
            self.column.update_nav(shell);
            self.topbar.update_nav(shell);
        }
        if r.all || r.selection || r.spaces {
            self.stage.update_empty(shell);
        }
    }

    /// Write the space's tint (and the private and neutral ones) as CSS.
    fn apply_tint(&self, shell: &Shell) {
        let dark = shell.is_dark();
        let space = shell.browser().current_space().clone();
        let column = Tint::of(&space, dark);
        let private = crate::private::tint(dark);
        let bar = crate::style::bar_tint(dark);
        let css =
            crate::style::tint_css(&column, &private, &bar, crate::style::accent(&column), dark);
        if *self.tint_key.borrow() != css {
            self.tint.load_from_string(&css);
            self.tint_key.replace(css);
        }
    }

    /// `Ctrl+L`: the header field (tabs in the column) or the card.
    pub fn edit_address(&self, shell: &Shell) {
        if self.layout.get() == TabsPosition::Left && shell.active_id().is_some() {
            self.card.close(shell);
            self.header.edit(shell);
        } else if shell.active_id().is_some() {
            self.card.open(shell, Mode::Edit);
        } else {
            self.card.open(shell, Mode::Launch);
        }
    }

    /// Escape: close what is on top.
    pub fn escape(&self, shell: &Shell) {
        if self.card.is_open() {
            self.card.close(shell);
        } else if self.stage.find.is_open() {
            self.stage.find.close(shell);
        } else if self.header.is_editing() {
            self.header.cancel(shell);
        } else if self.stage.panel_open() {
            self.stage.hide_panel();
            if let Some(p) = shell.active_page() {
                crate::palette::p_focus(&p);
            }
        } else {
            shell.stop_active();
        }
    }

    /// The header field's text (tests, automation).
    pub fn header_text(&self) -> gtk::Text {
        self.header.text()
    }

    /// The card's text.
    pub fn card_text(&self) -> gtk::Text {
        self.card.text()
    }

    /// The find bar's text.
    pub fn find_text(&self) -> gtk::Text {
        self.stage.find.text()
    }

    /// One quiet line at the foot of the window.
    pub fn toast(&self, text: &str) {
        let t = adw::Toast::new(text);
        t.set_timeout(2);
        self.toasts.add_toast(t);
    }

    /// "Cleared N tabs · Undo", for as long as Undo is offered
    /// (`copper_shell_core::clear`).
    pub fn cleared_toast(&self, count: usize) {
        let t = adw::Toast::new(&copper_shell_core::clear::cleared_words(count));
        t.set_button_label(Some("Undo"));
        t.set_action_name(Some("win.undo-clear"));
        t.set_timeout(copper_shell_core::clear::UNDO_FOR.as_secs() as u32);
        self.toasts.add_toast(t);
    }
}

/// What a window shortcut does while a dialog is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderDialog {
    /// Run the action anyway.
    Run,
    /// Close the dialog instead (`Ctrl+W`).
    CloseDialog,
    /// Leave the key to the dialog.
    Pass,
}

/// A dialog (Edit Space, Settings, a permission question…) has the keyboard:
/// the window's shortcuts must not reach the tabs, the card or the page
/// behind it. `Ctrl+W` closes the dialog; `Ctrl+Q` still quits; every other
/// key is the dialog's (typed into its fields, Escape closes it).
pub fn under_dialog(action: &str) -> UnderDialog {
    match action {
        "win.quit" => UnderDialog::Run,
        "win.close-tab" => UnderDialog::CloseDialog,
        _ => UnderDialog::Pass,
    }
}

/// A shortcut's action: activate `win.<name>` on the window, unless a
/// dialog is up ([`under_dialog`]).
fn window_action(action: &str) -> gtk::CallbackAction {
    let action = action.to_string();
    gtk::CallbackAction::new(move |widget, _| {
        let dialog = widget
            .downcast_ref::<adw::ApplicationWindow>()
            .and_then(|w| w.visible_dialog());
        if let Some(dialog) = dialog {
            match under_dialog(&action) {
                UnderDialog::Run => {}
                UnderDialog::CloseDialog => {
                    dialog.close();
                    return glib::Propagation::Stop;
                }
                UnderDialog::Pass => return glib::Propagation::Proceed,
            }
        }
        if widget.activate_action(&action, None).is_ok() {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    })
}

fn install_base_css() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(crate::style::BASE);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("a display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

/// A letter on a soft square (the mark of a tab without an icon).
pub fn letter_chip(text: &str, size: i32) -> gtk::Label {
    let letter: String = text
        .trim_start_matches("www.")
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "·".into());
    let l = gtk::Label::new(Some(&letter));
    l.add_css_class("chip");
    if size > 18 {
        l.add_css_class("big");
    }
    l.set_size_request(size, size);
    l.set_halign(gtk::Align::Center);
    l.set_valign(gtk::Align::Center);
    l
}

/// A window menu (the column's foot, the tab row's end).
pub fn window_menu(shell: &Shell) -> gio::Menu {
    let menu = gio::Menu::new();
    let new = gio::Menu::new();
    new.append(Some("New Tab"), Some("win.new-tab"));
    new.append(Some("New Private Tab"), Some("win.new-private-tab"));
    new.append(Some("New Space"), Some("win.new-space"));
    menu.append_section(None, &new);

    let tabs = gio::Menu::new();
    tabs.append(Some("Tabs on the Left"), Some("win.layout::left"));
    tabs.append(Some("Tabs on Top"), Some("win.layout::top"));
    menu.append_section(None, &tabs);

    let look = gio::Menu::new();
    look.append(Some("Light"), Some("win.look::light"));
    look.append(Some("Dark"), Some("win.look::dark"));
    look.append(Some("Match System"), Some("win.look::system"));
    menu.append_section(Some("Appearance"), &look);

    let extra = shell.registry().borrow().menu_items.clone();
    if !extra.is_empty() {
        let lanes = gio::Menu::new();
        for (label, action) in extra {
            lanes.append(Some(&label), Some(&action));
        }
        menu.append_section(None, &lanes);
    }

    let app = gio::Menu::new();
    app.append(Some("Settings"), Some("win.settings"));
    app.append(Some("Quit"), Some("win.quit"));
    menu.append_section(None, &app);
    menu
}

/// Back, forward, reload (stop while loading).
pub struct Nav {
    pub widget: gtk::Box,
    back: gtk::Button,
    forward: gtk::Button,
    reload: gtk::Button,
}

impl Nav {
    pub fn new() -> Nav {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        widget.add_css_class("nav");
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_action_name(Some("win.back"));
        back.set_tooltip_text(Some("Back"));
        let forward = gtk::Button::from_icon_name("go-next-symbolic");
        forward.set_action_name(Some("win.forward"));
        forward.set_tooltip_text(Some("Forward"));
        let reload = gtk::Button::from_icon_name("view-refresh-symbolic");
        reload.set_action_name(Some("win.reload"));
        reload.set_tooltip_text(Some("Reload"));
        for b in [&back, &forward, &reload] {
            b.set_valign(gtk::Align::Center);
            b.set_focus_on_click(false);
            widget.append(b);
        }
        Nav {
            widget,
            back,
            forward,
            reload,
        }
    }

    pub fn update(&self, shell: &Shell) {
        use copper_engine::PageView;
        let page = shell.active_page();
        let (b, f, loading) = page
            .as_ref()
            .map(|p| (p.can_go_back(), p.can_go_forward(), p.is_loading()))
            .unwrap_or((false, false, false));
        self.back.set_sensitive(b);
        self.forward.set_sensitive(f);
        self.reload.set_sensitive(shell.active_id().is_some());
        if loading {
            self.reload.set_icon_name("process-stop-symbolic");
            self.reload.set_action_name(Some("win.stop"));
            self.reload.set_tooltip_text(Some("Stop"));
        } else {
            self.reload.set_icon_name("view-refresh-symbolic");
            self.reload.set_action_name(Some("win.reload"));
            self.reload.set_tooltip_text(Some("Reload"));
        }
    }
}

impl Default for Nav {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{UnderDialog, under_dialog};

    #[test]
    fn a_dialog_keeps_the_window_shortcuts_to_itself() {
        for action in [
            "win.new-tab",
            "win.palette",
            "win.reopen-tab",
            "win.focus-field",
            "win.settings",
            "win.escape",
            "win.show-downloads",
        ] {
            assert_eq!(under_dialog(action), UnderDialog::Pass, "{action}");
        }
        assert_eq!(under_dialog("win.close-tab"), UnderDialog::CloseDialog);
        assert_eq!(under_dialog("win.quit"), UnderDialog::Run);
    }
}
