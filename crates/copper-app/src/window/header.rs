//! The slim header over the page when the tabs are in the column: the one
//! field, centred, and the window's own buttons at the end.
//!
//! At rest the field shows where the page is (`example.com/path`, no
//! scheme); focused (`Ctrl+L` or a click) it holds the whole address,
//! selected. Typing completes inline and lists suggestions under it;
//! nothing is fetched or sent before Return.

use crate::palette::complete::Purpose;
use crate::palette::{FieldCore, Mode, perform};
use crate::shell::Shell;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct Header {
    pub widget: gtk::WindowHandle,
    field: gtk::Box,
    core: Rc<FieldCore>,
    tag: gtk::Label,
    /// The suggestions, laid over the window under the field (an overlay
    /// rather than a popover: one surface, so it is in every screenshot
    /// and never fights the compositor for focus).
    pub dropdown: gtk::Box,
    root: RefCell<Option<gtk::Widget>>,
    editing: Cell<bool>,
}

impl Header {
    pub fn new(shell: &Shell) -> Rc<Header> {
        let bar = gtk::CenterBox::new();
        bar.add_css_class("copper-header");

        let field = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        field.add_css_class("copper-field");
        field.set_valign(gtk::Align::Center);
        let tag = gtk::Label::new(Some("Private"));
        tag.add_css_class("private-tag");
        tag.set_valign(gtk::Align::Center);
        tag.set_visible(false);
        field.append(&tag);
        let core = FieldCore::new(shell);
        field.append(&core.text);
        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(680);
        clamp.set_tightening_threshold(400);
        clamp.set_child(Some(&field));
        clamp.set_hexpand(true);
        clamp.set_margin_start(56);
        clamp.set_margin_end(8);
        bar.set_center_widget(Some(&clamp));
        bar.set_end_widget(Some(&gtk::WindowControls::new(gtk::PackType::End)));

        let dropdown = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dropdown.add_css_class("copper-dropdown");
        dropdown.set_halign(gtk::Align::Start);
        dropdown.set_valign(gtk::Align::Start);
        dropdown.set_visible(false);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(8 * 40);
        scroller.set_child(Some(&core.list.list));
        dropdown.append(&scroller);

        let widget = gtk::WindowHandle::new();
        widget.set_child(Some(&bar));

        let me = Rc::new(Header {
            widget,
            field,
            core,
            tag,
            dropdown,
            root: RefCell::default(),
            editing: Cell::new(false),
        });

        let weak_me = Rc::downgrade(&me);
        me.core.on_rows(move |n| {
            let Some(me) = weak_me.upgrade() else { return };
            if me.editing.get() && n > 0 {
                me.place_dropdown();
            } else {
                me.dropdown.set_visible(false);
            }
        });
        let weak_me = Rc::downgrade(&me);
        me.core.on_submit(move |shell, row, new_tab| {
            let Some(me) = weak_me.upgrade() else { return };
            me.finish(shell);
            perform(shell, row, Mode::Edit, new_tab);
        });
        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        me.core.on_cancel(move || {
            if let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) {
                me.cancel(&s);
            }
        });

        let focus = gtk::EventControllerFocus::new();
        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        focus.connect_enter(move |_| {
            let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) else {
                return;
            };
            if !me.editing.get() {
                me.edit(&s);
            }
        });
        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        focus.connect_leave(move |_| {
            let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) else {
                return;
            };
            // A click on a suggestion lands after the focus leaves.
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                if !me.core.text.has_focus() {
                    me.finish(&s);
                }
            });
        });
        me.core.text.add_controller(focus);
        me
    }

    /// The widget the dropdown is laid over (the window's overlay).
    pub fn set_root(&self, root: &impl IsA<gtk::Widget>) {
        self.root.replace(Some(root.clone().upcast()));
    }

    fn place_dropdown(&self) {
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let Some(b) = self.field.compute_bounds(&root) else {
            return;
        };
        self.dropdown.set_margin_start(b.x() as i32 - 6);
        self.dropdown
            .set_margin_top((b.y() + b.height()) as i32 + 6);
        self.dropdown.set_size_request(b.width() as i32 + 12, -1);
        self.dropdown.set_visible(true);
    }

    /// Start editing the selected tab's address.
    pub fn edit(&self, shell: &Shell) {
        self.editing.set(true);
        self.field.add_css_class("focused");
        let url = shell
            .browser()
            .active()
            .map(|t| t.url.clone())
            .filter(|u| u != "about:blank")
            .unwrap_or_default();
        self.core.begin(shell, Purpose::Edit, &url);
    }

    pub fn text(&self) -> gtk::Text {
        self.core.text.clone()
    }

    pub fn is_editing(&self) -> bool {
        self.editing.get()
    }

    /// Escape: put the address back, give the page the keyboard.
    pub fn cancel(&self, shell: &Shell) {
        self.finish(shell);
        if let Some(p) = shell.active_page() {
            crate::palette::p_focus(&p);
        }
    }

    fn finish(&self, shell: &Shell) {
        if !self.editing.replace(false) {
            return;
        }
        self.field.remove_css_class("focused");
        self.dropdown.set_visible(false);
        self.update(shell);
    }

    /// Show where the selected tab is (unless the user is typing).
    pub fn update(&self, shell: &Shell) {
        let title = shell
            .browser()
            .active()
            .map(|t| t.label())
            .filter(|t| !t.trim().is_empty());
        shell.window().set_title(Some(&match title {
            Some(t) => format!("{t} — Copper"),
            None => "Copper".to_string(),
        }));
        let private = shell.active_is_private();
        self.tag.set_visible(private);
        if private {
            self.field.add_css_class("private");
        } else {
            self.field.remove_css_class("private");
        }
        self.core
            .text
            .set_placeholder_text(Some("Search or enter an address"));
        if self.editing.get() {
            return;
        }
        self.field.set_visible(shell.active_id().is_some());
        let url = shell
            .browser()
            .active()
            .map(|t| t.url.clone())
            .unwrap_or_default();
        let shown = if url.is_empty() || url == "about:blank" {
            String::new()
        } else {
            copper_core::address::pretty_str(&url)
        };
        self.core.set_quiet(&shown);
    }
}
