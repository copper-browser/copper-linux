//! The stage: the selected tab's page, the progress line over it, the link
//! under the pointer, the find bar, and the side panel later lanes fill.

use crate::shell::GtkShell;
use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use copper_core::TabId;
use copper_engine::{FindOptions, PageView};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

pub struct Stage {
    pub widget: gtk::Box,
    stack: gtk::Stack,
    empty: gtk::Box,
    empty_line: gtk::Label,
    progress: gtk::ProgressBar,
    hover: gtk::Label,
    pub find: Rc<FindBar>,
    panel: gtk::Revealer,
    panels: gtk::Stack,
    built: RefCell<HashMap<String, gtk::Widget>>,
    shown: RefCell<Option<TabId>>,
    fade: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Stage {
    pub fn new(shell: &Shell) -> Rc<Stage> {
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);

        let empty = gtk::Box::new(gtk::Orientation::Vertical, 6);
        empty.add_css_class("copper-empty");
        empty.set_valign(gtk::Align::Center);
        empty.set_halign(gtk::Align::Center);
        let empty_line = gtk::Label::new(None);
        empty_line.add_css_class("line");
        empty.append(&empty_line);
        stack.add_named(&empty, Some("empty"));

        let progress = gtk::ProgressBar::new();
        progress.add_css_class("copper-progress");
        progress.set_valign(gtk::Align::Start);
        progress.set_can_target(false);
        progress.set_visible(false);

        let hover = gtk::Label::new(None);
        hover.add_css_class("copper-hover");
        hover.set_halign(gtk::Align::Start);
        hover.set_valign(gtk::Align::End);
        hover.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        hover.set_max_width_chars(80);
        hover.set_can_target(false);
        hover.set_visible(false);

        let find = FindBar::new(shell);

        let overlay = gtk::Overlay::new();
        overlay.add_css_class("copper-stage");
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&progress);
        overlay.add_overlay(&hover);
        overlay.add_overlay(&find.revealer);
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);

        let panels = gtk::Stack::new();
        panels.set_size_request(360, -1);
        let panel = gtk::Revealer::new();
        panel.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
        panel.set_transition_duration(160);
        panel.set_child(Some(&panels));
        panel.set_reveal_child(false);
        // Escape anywhere in a panel (its search field included) puts it away.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = shell.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let plain = !state.intersects(
                gtk::gdk::ModifierType::CONTROL_MASK
                    | gtk::gdk::ModifierType::ALT_MASK
                    | gtk::gdk::ModifierType::SHIFT_MASK,
            );
            if key != gtk::gdk::Key::Escape || !plain {
                return glib::Propagation::Proceed;
            }
            match weak.upgrade() {
                Some(s) if s.ui().stage.panel_open() => {
                    s.ui().stage.hide_panel();
                    if let Some(p) = s.active_page() {
                        crate::palette::p_focus(&p);
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        panel.add_controller(keys);

        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_vexpand(true);
        widget.append(&overlay);
        widget.append(&panel);

        Rc::new(Stage {
            widget,
            stack,
            empty,
            empty_line,
            progress,
            hover,
            find,
            panel,
            panels,
            built: RefCell::default(),
            shown: RefCell::default(),
            fade: Rc::default(),
        })
    }

    pub fn add_page(&self, slot: &PageSlot) {
        if slot.frame.parent().is_none() {
            self.stack.add_named(slot.frame.overlay(), Some(&slot.id.0));
        }
    }

    pub fn remove_page(&self, slot: &PageSlot) {
        if slot.frame.parent().as_ref() == Some(self.stack.upcast_ref()) {
            self.stack.remove(slot.frame.overlay());
        }
    }

    /// Show a tab's page, or the empty ground.
    pub fn show(&self, shell: &Shell, id: Option<&TabId>) {
        let changed = self.shown.borrow().as_ref() != id;
        match id.and_then(|i| shell.slot(i)) {
            Some(slot) => {
                self.add_page(&slot);
                self.stack.set_visible_child(slot.frame.overlay());
                self.set_progress(slot.loading.get(), slot.progress.get());
            }
            None => {
                self.stack.set_visible_child(&self.empty);
                self.set_progress(false, 0.0);
            }
        }
        if changed {
            self.set_hover(None);
            if self.find.is_open() {
                self.find.close(shell);
            }
        }
        self.shown.replace(id.cloned());
        self.update_empty(shell);
    }

    pub fn update_empty(&self, shell: &Shell) {
        let name = shell.browser().current_space().name.clone();
        self.empty_line.set_text(&format!("Nothing open in {name}"));
    }

    /// The thin line: shown while loading, faded out when done.
    pub fn set_progress(&self, loading: bool, fraction: f64) {
        if let Some(id) = self.fade.borrow_mut().take() {
            id.remove();
        }
        if loading {
            self.progress.set_visible(true);
            self.progress.set_opacity(1.0);
            self.progress.set_fraction(fraction.clamp(0.08, 1.0));
        } else if self.progress.is_visible() {
            self.progress.set_fraction(1.0);
            let bar = self.progress.clone();
            let fade = self.fade.clone();
            let id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(220), move || {
                    bar.set_visible(false);
                    bar.set_fraction(0.0);
                    fade.borrow_mut().take();
                });
            self.fade.replace(Some(id));
        }
    }

    /// The address of the link under the pointer, bottom left.
    pub fn set_hover(&self, url: Option<&str>) {
        match url.filter(|u| !u.is_empty()) {
            Some(u) => {
                self.hover.set_text(u);
                self.hover.set_visible(true);
            }
            None => self.hover.set_visible(false),
        }
    }

    /// Show or hide a registered panel ([`crate::actions::register_panel`]).
    pub fn toggle_panel(&self, shell: &Shell, id: &str) {
        let showing =
            self.panel.reveals_child() && self.panels.visible_child_name().as_deref() == Some(id);
        if showing {
            self.panel.set_reveal_child(false);
            return;
        }
        self.show_panel(shell, id);
    }

    pub fn show_panel(&self, shell: &Shell, id: &str) {
        if !self.built.borrow().contains_key(id) {
            let panel = shell
                .registry()
                .borrow()
                .panels
                .iter()
                .find(|p| p.id == id)
                .cloned();
            let Some(panel) = panel else { return };
            let w = (panel.build)(shell);
            self.panels.add_named(&w, Some(id));
            self.built.borrow_mut().insert(id.to_string(), w);
        }
        self.panels.set_visible_child_name(id);
        self.panel.set_reveal_child(true);
    }

    pub fn hide_panel(&self) {
        self.panel.set_reveal_child(false);
    }

    /// A side panel is showing.
    pub fn panel_open(&self) -> bool {
        self.panel.reveals_child()
    }
}

/// Find in page: a small card at the top right of the page.
pub struct FindBar {
    pub revealer: gtk::Revealer,
    text: gtk::Text,
    count: gtk::Label,
    open: Cell<bool>,
    generation: Cell<u64>,
}

impl FindBar {
    fn new(shell: &Shell) -> Rc<FindBar> {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        bar.add_css_class("find-bar");
        let text = gtk::Text::new();
        text.set_placeholder_text(Some("Find in page"));
        text.update_property(&[gtk::accessible::Property::Label("Find in page")]);
        text.set_hexpand(true);
        bar.append(&text);
        let count = gtk::Label::new(None);
        count.add_css_class("count");
        bar.append(&count);
        let prev = gtk::Button::from_icon_name("go-up-symbolic");
        prev.set_tooltip_text(Some("Previous (Shift+Ctrl+G)"));
        prev.set_action_name(Some("win.find-previous"));
        let next = gtk::Button::from_icon_name("go-down-symbolic");
        next.set_tooltip_text(Some("Next (Ctrl+G)"));
        next.set_action_name(Some("win.find-next"));
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Done"));
        for b in [&prev, &next, &close] {
            b.set_focus_on_click(false);
            bar.append(b);
        }
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        revealer.set_transition_duration(120);
        revealer.set_halign(gtk::Align::End);
        revealer.set_valign(gtk::Align::Start);
        revealer.set_child(Some(&bar));
        revealer.set_visible(false);

        let me = Rc::new(FindBar {
            revealer,
            text,
            count,
            open: Cell::new(false),
            generation: Cell::new(0),
        });

        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        me.text.connect_changed(move |_| {
            if let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) {
                me.search(&s);
            }
        });
        let weak = shell.downgrade();
        me.text.connect_activate(move |_| {
            if let Some(s) = weak.upgrade() {
                s.ui().stage.find.step(&s, true);
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = shell.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(s) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            use gtk::gdk::Key;
            match key {
                Key::Escape => {
                    s.ui().stage.find.close(&s);
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter
                    if state.contains(gtk::gdk::ModifierType::SHIFT_MASK) =>
                {
                    s.ui().stage.find.step(&s, false);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        me.text.add_controller(keys);
        let weak = shell.downgrade();
        close.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.ui().stage.find.close(&s);
            }
        });
        me
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn text(&self) -> gtk::Text {
        self.text.clone()
    }

    pub fn open(&self, shell: &Shell) {
        if shell.active_page().is_none() {
            return;
        }
        self.open.set(true);
        self.revealer.set_visible(true);
        self.revealer.set_reveal_child(true);
        self.text.grab_focus();
        self.text.select_region(0, -1);
        if !self.text.text().is_empty() {
            self.search(shell);
        }
    }

    pub fn close(&self, shell: &Shell) {
        if !self.open.replace(false) {
            return;
        }
        self.revealer.set_reveal_child(false);
        self.revealer.set_visible(false);
        self.count.set_text("");
        if let Some(p) = shell.active_page() {
            p.find_finish();
            crate::palette::p_focus(&p);
        }
    }

    pub fn step(&self, shell: &Shell, forward: bool) {
        if !self.open.get() {
            self.open(shell);
            return;
        }
        if let Some(p) = shell.active_page() {
            if forward {
                p.find_next();
            } else {
                p.find_previous();
            }
        }
    }

    fn search(&self, shell: &Shell) {
        let Some(page) = shell.active_page() else {
            return;
        };
        let query = self.text.text().to_string();
        let gen_ = self.generation.get() + 1;
        self.generation.set(gen_);
        if query.is_empty() {
            page.find_finish();
            self.count.set_text("");
            return;
        }
        let fut = page.find(
            &query,
            FindOptions {
                wrap_around: true,
                ..Default::default()
            },
        );
        let weak = shell.downgrade();
        glib::spawn_future_local(async move {
            let found = fut.await;
            let Some(s) = weak.upgrade() else { return };
            let me = &s.ui().stage.find;
            if me.generation.get() != gen_ {
                return;
            }
            let text = match found {
                Ok(0) | Err(_) => "No matches".to_string(),
                Ok(1) => "1 match".to_string(),
                Ok(n) => format!("{n} matches"),
            };
            me.count.set_text(&text);
        });
    }
}
