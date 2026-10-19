//! Hide anything in the GTK window (Mac: `Curtain.swift`, `Hidden.swift`):
//! Ctrl+Shift+H and its palette row, Esc and Ctrl+Z while picking, and the
//! Hidden panel. What is hidden, the sheets on every page and the picker's
//! messages are copper-shell-core's [`copper_shell_core::curtain`],
//! re-exported here.

pub use copper_shell_core::curtain::*;

use super::ui;
use crate::actions::{Command, Panel, register_action, register_command, register_panel};
use crate::platform::Gtk;
use crate::shell::GtkShell;
use crate::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::rc::Rc;

/// Load `hidden.json`; the shortcut, the palette rows, the panel, the keys
/// that matter while picking.
pub fn register(shell: &Shell) {
    start(shell);
    register_action(shell, "hide-anything", &["<Control><Shift>h"], toggle);
    register_command(
        shell,
        Command::new(
            "hide",
            "Hide Something on This Page",
            "view-conceal-symbolic",
            toggle,
        )
        .shortcut("Ctrl+Shift+H")
        .when(|s| here(s).is_some()),
    );
    register_panel(
        shell,
        Panel {
            id: "hidden".into(),
            title: "Hidden on This Site".into(),
            icon: "view-conceal-symbolic".into(),
            build: Rc::new(HiddenPanel::build),
        },
    );
    keys(shell);
}

/// Esc and Ctrl+Z belong to the picker while it is out, before the page or
/// the shell's own Escape sees them.
fn keys(shell: &Shell) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = shell.downgrade();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let Some(shell) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let Some(id) = shell.active_id() else {
            return glib::Propagation::Proceed;
        };
        if !picking::<Gtk>(&id) {
            return glib::Propagation::Proceed;
        }
        let ctrl = mods.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if key == gtk::gdk::Key::Escape {
            stop(&shell, &id);
            return glib::Propagation::Stop;
        }
        if ctrl && matches!(key, gtk::gdk::Key::z | gtk::gdk::Key::Z) {
            undo(&shell);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    shell.window().add_controller(keys);
}

/// The Hidden panel: what is hidden on the site in front, and the way back.
struct HiddenPanel {
    frame: ui::Frame,
    list: gtk::ListBox,
    content: gtk::Box,
    restore_all: gtk::Button,
}

impl HiddenPanel {
    fn build(shell: &Shell) -> gtk::Widget {
        let weak = shell.downgrade();
        let frame = ui::frame("Hidden", move || {
            if let Some(s) = weak.upgrade() {
                unpeek(&s);
                s.ui().stage.hide_panel();
            }
        });
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_vexpand(true);
        frame.body.append(&ui::scroller(&content));
        let list = ui::list();

        let hide = ui::pill("Hide something…", true);
        let weak = shell.downgrade();
        hide.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                toggle(&s);
            }
        });
        frame.foot.append(&hide);
        let restore_all = ui::pill("Restore all", false);
        let weak = shell.downgrade();
        restore_all.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade()
                && let Some(site) = here(&s)
            {
                self::restore_all(&s, &site);
            }
        });
        frame.foot.append(&restore_all);

        // Leaving the list puts the page back the way it was.
        let motion = gtk::EventControllerMotion::new();
        let weak = shell.downgrade();
        motion.connect_leave(move |_| {
            if let Some(s) = weak.upgrade() {
                unpeek(&s);
            }
        });
        list.add_controller(motion);

        let panel = Rc::new(HiddenPanel {
            frame,
            list,
            content,
            restore_all,
        });
        let weak = shell.downgrade();
        let p = panel.clone();
        panel.frame.root.connect_map(move |_| {
            if let Some(s) = weak.upgrade() {
                p.refresh(&s);
            }
        });
        let weak = shell.downgrade();
        panel.frame.root.connect_unmap(move |_| {
            if let Some(s) = weak.upgrade() {
                unpeek(&s);
            }
        });
        let p = panel.clone();
        on_change::<Gtk>(Rc::new(move |s: &Shell| p.refresh(s)));
        panel.frame.root.clone().upcast()
    }

    fn refresh(&self, shell: &Shell) {
        if !self.frame.root.is_mapped() {
            return;
        }
        let site = here(shell);
        self.frame
            .title
            .set_text(site.as_deref().unwrap_or("This page"));
        ui::clear(&self.content);
        ui::clear_list(&self.list);
        let veils = site.as_deref().map(veils::<Gtk>).unwrap_or_default();
        self.restore_all.set_visible(!veils.is_empty());
        let Some(site) = site.filter(|_| !veils.is_empty()) else {
            self.frame.subtitle.set_visible(false);
            self.content.append(&ui::empty("Nothing is hidden here."));
            return;
        };
        self.frame
            .subtitle
            .set_text("Rest on a line to see what it hides");
        self.frame.subtitle.set_visible(true);
        for v in veils {
            let mark = gtk::Image::from_icon_name("view-conceal-symbolic");
            mark.set_pixel_size(16);
            mark.add_css_class("file-mark");
            let r = ui::row(&mark, &v.label, v.note.as_deref().unwrap_or(""));
            let back = ui::pill("Restore", false);
            let weak = shell.downgrade();
            let (s1, sel1) = (site.clone(), v.selector.clone());
            back.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    restore(&s, &s1, &sel1);
                }
            });
            r.end.append(&back);
            let motion = gtk::EventControllerMotion::new();
            let weak = shell.downgrade();
            let (s2, sel2) = (site.clone(), v.selector.clone());
            motion.connect_enter(move |_, _, _| {
                if let Some(s) = weak.upgrade() {
                    peek(&s, &s2, &sel2);
                }
            });
            r.row.add_controller(motion);
            self.list.append(&r.row);
        }
        self.content.append(&self.list);
    }
}
