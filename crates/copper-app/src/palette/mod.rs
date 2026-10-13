//! The one field, and the card it becomes (Mac: `Omnibox.swift`,
//! `Fork/CommandPalette.swift`, `Fork/CommandBar.swift`).
//!
//! * [`FieldCore`] — a text field with inline completion (the rest of a
//!   visited address, selected, taken by Tab or Return) and a list of
//!   suggestions under it. Used by the slim header (tabs in the column) and
//!   by the card.
//! * [`Card`] — the floating card: `Ctrl+T` (where to, in a new tab),
//!   `Ctrl+Shift+N` (the same, private), `Ctrl+K` (open tabs, commands,
//!   places), and `Ctrl+L` when the tabs are on top.
//!
//! Nothing typed leaves the machine before Return: rows come from memory
//! and the disk (history, open tabs, bookmarks, cached icons).

pub mod complete;

use crate::actions;
use crate::shell::Shell;
use complete::{Action, Inputs, Kind, OpenRow, Purpose, Row};
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What the card is open for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The selected tab's address.
    Edit,
    /// Somewhere new, in a new tab.
    Launch,
    /// Somewhere new, in a new private tab.
    LaunchPrivate,
    /// Tabs, commands and places.
    Palette,
}

impl Mode {
    fn purpose(self) -> Purpose {
        match self {
            Mode::Edit => Purpose::Edit,
            Mode::Launch | Mode::LaunchPrivate => Purpose::Launch,
            Mode::Palette => Purpose::Palette,
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Mode::Edit | Mode::Launch => "Search or enter an address",
            Mode::LaunchPrivate => crate::private::PLACEHOLDER,
            Mode::Palette => "Search tabs, commands and places",
        }
    }
}

/// The suggestion rows under a field.
pub struct SuggestionList {
    pub list: gtk::ListBox,
    rows: RefCell<Vec<Row>>,
}

impl SuggestionList {
    fn new() -> Rc<SuggestionList> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.add_css_class("copper-suggestions");
        list.set_activate_on_single_click(true);
        list.set_focusable(false);
        Rc::new(SuggestionList {
            list,
            rows: RefCell::default(),
        })
    }

    fn set(&self, shell: &Shell, rows: Vec<Row>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for r in &rows {
            self.list.append(&row_widget(shell, r));
        }
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
        self.rows.replace(rows);
    }

    fn len(&self) -> usize {
        self.rows.borrow().len()
    }

    fn selected(&self) -> Option<Row> {
        let i = self.list.selected_row()?.index();
        self.rows.borrow().get(i as usize).cloned()
    }

    fn row(&self, i: usize) -> Option<Row> {
        self.rows.borrow().get(i).cloned()
    }

    fn step(&self, by: i32) {
        let n = self.len() as i32;
        if n == 0 {
            return;
        }
        let at = self.list.selected_row().map(|r| r.index()).unwrap_or(-1);
        let next = (at + by).rem_euclid(n);
        if let Some(r) = self.list.row_at_index(next) {
            self.list.select_row(Some(&r));
        }
    }
}

/// One suggestion row: mark, title, quiet address, what Return does.
fn row_widget(shell: &Shell, r: &Row) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("copper-suggestion");
    // The field keeps the keyboard; rows are picked with the arrows.
    row.set_focusable(false);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let mark: gtk::Widget = match (r.kind, &r.icon, &r.url) {
        (Kind::Command, Some(icon), _) => gtk::Image::from_icon_name(icon).upcast(),
        (Kind::Search, _, _) => gtk::Image::from_icon_name("system-search-symbolic").upcast(),
        (_, _, Some(url)) => {
            let live = match &r.action {
                Action::Switch(id) => shell.favicons().borrow().get(id).cloned(),
                _ => shell.open_icon(url),
            };
            match live.or_else(|| shell.cached_icon(url)) {
                Some(t) => crate::window::tabs::icon_image(&t, 16).upcast(),
                None => crate::window::letter_chip(&crate::shell::host_of(url), 16).upcast(),
            }
        }
        _ => gtk::Image::from_icon_name("web-browser-symbolic").upcast(),
    };
    mark.set_valign(gtk::Align::Center);
    mark.add_css_class("mark");
    b.append(&mark);
    let title = gtk::Label::new(Some(&r.title));
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_xalign(0.0);
    title.add_css_class("title");
    title.set_hexpand(false);
    title.set_max_width_chars(48);
    b.append(&title);
    if !r.detail.is_empty() {
        let detail = gtk::Label::new(Some(&r.detail));
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
        detail.set_xalign(0.0);
        detail.add_css_class("detail");
        detail.set_hexpand(true);
        b.append(&detail);
    } else {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        b.append(&spacer);
    }
    let hint = gtk::Label::new(Some(&r.hint));
    hint.add_css_class("hint");
    b.append(&hint);
    row.set_child(Some(&b));
    row.update_property(&[gtk::accessible::Property::Label(&format!(
        "{} {}, {}",
        r.title, r.detail, r.hint
    ))]);
    row
}

type Submit = Rc<dyn Fn(&Shell, &Row, bool)>;
type RowsChanged = Rc<dyn Fn(usize)>;

/// A field with inline completion and suggestions.
pub struct FieldCore {
    pub text: gtk::Text,
    pub list: Rc<SuggestionList>,
    purpose: Cell<Purpose>,
    /// What was really typed (without the offered completion).
    typed: RefCell<String>,
    quiet: Cell<bool>,
    /// The rows changed (show / hide the list).
    on_rows: RefCell<Option<RowsChanged>>,
    submit: RefCell<Option<Submit>>,
    cancel: RefCell<Option<Rc<dyn Fn()>>>,
}

impl FieldCore {
    pub fn new(shell: &Shell) -> Rc<FieldCore> {
        let text = gtk::Text::new();
        text.set_hexpand(true);
        text.set_propagate_text_width(false);
        text.update_property(&[gtk::accessible::Property::Label("Address and search")]);
        let core = Rc::new(FieldCore {
            text,
            list: SuggestionList::new(),
            purpose: Cell::new(Purpose::Edit),
            typed: RefCell::default(),
            quiet: Cell::new(false),
            on_rows: RefCell::default(),
            submit: RefCell::default(),
            cancel: RefCell::default(),
        });

        let weak_core = Rc::downgrade(&core);
        let weak = shell.downgrade();
        core.text.connect_changed(move |_| {
            let (Some(core), Some(shell)) = (weak_core.upgrade(), weak.upgrade()) else {
                return;
            };
            core.changed(&shell);
        });

        let weak_core = Rc::downgrade(&core);
        let weak = shell.downgrade();
        core.text.connect_activate(move |_| {
            let (Some(core), Some(shell)) = (weak_core.upgrade(), weak.upgrade()) else {
                return;
            };
            core.activate(&shell, false);
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak_core = Rc::downgrade(&core);
        let weak = shell.downgrade();
        keys.connect_key_pressed(move |_, key, _, state| {
            let (Some(core), Some(shell)) = (weak_core.upgrade(), weak.upgrade()) else {
                return glib::Propagation::Proceed;
            };
            use gtk::gdk::Key;
            let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            match key {
                Key::Down => {
                    core.list.step(1);
                    glib::Propagation::Stop
                }
                Key::Up => {
                    core.list.step(-1);
                    glib::Propagation::Stop
                }
                Key::n | Key::j if ctrl => {
                    core.list.step(1);
                    glib::Propagation::Stop
                }
                Key::p if ctrl => {
                    core.list.step(-1);
                    glib::Propagation::Stop
                }
                Key::Tab if !ctrl => {
                    core.accept_completion();
                    glib::Propagation::Stop
                }
                Key::Escape => {
                    if let Some(c) = core.cancel.borrow().clone() {
                        c();
                    }
                    glib::Propagation::Stop
                }
                Key::Return | Key::KP_Enter if state.contains(gtk::gdk::ModifierType::ALT_MASK) => {
                    core.activate(&shell, true);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        core.text.add_controller(keys);

        let weak_core = Rc::downgrade(&core);
        let weak = shell.downgrade();
        core.list.list.connect_row_activated(move |_, row| {
            let (Some(core), Some(shell)) = (weak_core.upgrade(), weak.upgrade()) else {
                return;
            };
            if let Some(r) = core.list.row(row.index() as usize) {
                core.fire(&shell, &r, false);
            }
        });
        core
    }

    pub fn on_rows(&self, f: impl Fn(usize) + 'static) {
        self.on_rows.replace(Some(Rc::new(f)));
    }

    pub fn on_submit(&self, f: impl Fn(&Shell, &Row, bool) + 'static) {
        self.submit.replace(Some(Rc::new(f)));
    }

    pub fn on_cancel(&self, f: impl Fn() + 'static) {
        self.cancel.replace(Some(Rc::new(f)));
    }

    /// Set the text without offering anything (showing an address).
    pub fn set_quiet(&self, text: &str) {
        self.quiet.set(true);
        self.text.set_text(text);
        self.typed.replace(text.to_string());
        self.quiet.set(false);
    }

    /// Start editing for `purpose`, with `text` selected.
    pub fn begin(&self, shell: &Shell, purpose: Purpose, text: &str) {
        self.purpose.set(purpose);
        self.set_quiet(text);
        self.text.grab_focus();
        self.text.select_region(0, -1);
        self.refresh(shell);
    }

    fn changed(&self, shell: &Shell) {
        if self.quiet.get() {
            return;
        }
        let now = self.text.text().to_string();
        let before = self.typed.replace(now.clone());
        let at_end = self.text.position() as usize == now.chars().count();
        if complete::grew(&before, &now) && at_end && self.purpose.get() != Purpose::Palette {
            let (tabs, visited) = shell.suggestions(&now, 6);
            if let Some(rest) = complete::inline(&now, &tabs, &visited) {
                let start = now.chars().count() as i32;
                self.quiet.set(true);
                self.text.set_text(&format!("{now}{rest}"));
                self.text.select_region(start, -1);
                self.quiet.set(false);
            }
        }
        self.refresh(shell);
    }

    /// Take the offered completion: the caret after it, nothing opened.
    fn accept_completion(&self) {
        let full = self.text.text().to_string();
        self.typed.replace(full.clone());
        self.text.set_position(-1);
        self.text.select_region(-1, -1);
    }

    /// Rebuild the rows for the current text.
    pub fn refresh(&self, shell: &Shell) {
        let typed = self.typed.borrow().clone();
        let full = self.text.text().to_string();
        let purpose = self.purpose.get();
        // A selected address the user has not touched (Ctrl+L on a page)
        // offers nothing until they type.
        let untouched = purpose == Purpose::Edit
            && self
                .text
                .selection_bounds()
                .is_some_and(|(a, b)| a == 0 && b as usize == full.chars().count())
            && typed == full;
        let rows = if untouched {
            Vec::new()
        } else {
            complete::rows(purpose, &inputs(shell, &typed, &full, purpose))
        };
        let n = rows.len();
        self.list.set(shell, rows);
        if let Some(f) = self.on_rows.borrow().clone() {
            f(n);
        }
    }

    fn activate(&self, shell: &Shell, new_tab: bool) {
        let full = self.text.text().to_string();
        let row = if self.list.len() > 0 {
            self.list.selected()
        } else {
            None
        };
        let row = match row {
            Some(r) => r,
            None => {
                let (tabs, visited) = shell.suggestions(&full, 6);
                let known = complete::known_place(&full, &tabs, &visited).map(|k| k.url.clone());
                let Some(dest) = known.or_else(|| shell.destination(&full)) else {
                    return;
                };
                Row {
                    kind: Kind::Address,
                    title: full.clone(),
                    detail: String::new(),
                    hint: String::new(),
                    action: Action::Go(dest),
                    url: None,
                    icon: None,
                }
            }
        };
        self.fire(shell, &row, new_tab);
    }

    fn fire(&self, shell: &Shell, row: &Row, new_tab: bool) {
        if let Some(f) = self.submit.borrow().clone() {
            f(shell, row, new_tab);
        }
    }
}

/// What the ranking reads, gathered from the shell.
fn inputs<'a>(shell: &Shell, typed: &'a str, full: &'a str, purpose: Purpose) -> Inputs<'a> {
    let engine = shell.settings().search_engine();
    let destination = shell.destination(full);
    let is_address = copper_core::address::url_from_typed(full).is_some();
    let (tabs, visited) = shell.suggestions(typed, 6);
    let bookmarks = if typed.trim().is_empty() {
        Vec::new()
    } else {
        shell.bookmarks().search(typed)
    };
    let mut commands: Vec<complete::CommandRow> = Vec::new();
    let mut open: Vec<OpenRow> = Vec::new();
    if purpose == Purpose::Palette {
        let mut all = actions::commands(shell);
        all.extend(actions::space_commands(shell));
        commands = all
            .into_iter()
            .map(|c| complete::CommandRow {
                id: c.id,
                name: c.name,
                icon: c.icon,
                shortcut: c.shortcut,
            })
            .collect();
        if typed.trim().is_empty() {
            open = open_rows(shell);
        }
    }
    Inputs {
        typed,
        full,
        engine: engine.name,
        destination,
        is_address,
        tabs,
        visited,
        bookmarks,
        commands,
        open,
    }
}

/// Every open tab, most recently used first: this space, then the others.
fn open_rows(shell: &Shell) -> Vec<OpenRow> {
    let b = shell.browser();
    let current = b.current_space_id().to_string();
    let active = b.active_id();
    let mut here: Vec<&copper_core::Tab> = b
        .pins()
        .into_iter()
        .chain(b.row(&current))
        .filter(|t| Some(&t.id) != active.as_ref())
        .collect();
    here.sort_by(|a, b| b.seen.0.total_cmp(&a.seen.0));
    let mut out: Vec<OpenRow> = here
        .into_iter()
        .map(|t| OpenRow {
            id: t.id.clone(),
            title: t.label(),
            url: t.url.clone(),
            elsewhere: None,
        })
        .collect();
    for sp in b.spaces().filter(|s| s.id != current) {
        let mut row = b.row(&sp.id);
        row.sort_by(|a, b| b.seen.0.total_cmp(&a.seen.0));
        out.extend(row.into_iter().map(|t| OpenRow {
            id: t.id.clone(),
            title: t.label(),
            url: t.url.clone(),
            elsewhere: Some(sp.name.clone()),
        }));
    }
    out
}

/// Do what a row says.
pub fn perform(shell: &Shell, row: &Row, mode: Mode, new_tab: bool) {
    match &row.action {
        Action::Switch(id) => {
            let space = shell.browser().tab(id).map(|t| t.space.clone());
            let pinned = shell.browser().tab(id).is_some_and(|t| t.is_pinned());
            if let Some(space) = space
                && !pinned
                && space != shell.browser().current_space_id()
            {
                shell.switch_space(&space);
            }
            shell.select(id);
        }
        Action::Command(id) => {
            if !actions::run_command(shell, id) {
                for c in actions::space_commands(shell) {
                    if c.id == *id {
                        (c.run)(shell);
                    }
                }
            }
        }
        Action::Go(url) => match mode {
            Mode::Edit if !new_tab && shell.active_id().is_some() => {
                let id = shell.active_id().expect("checked");
                shell.navigate(&id, url);
            }
            Mode::LaunchPrivate => {
                shell.open(
                    url,
                    crate::shell::OpenTab {
                        foreground: true,
                        private: true,
                        ..Default::default()
                    },
                );
            }
            _ => {
                let private = mode == Mode::Edit && shell.active_is_private();
                shell.open(
                    url,
                    crate::shell::OpenTab {
                        foreground: true,
                        private,
                        ..Default::default()
                    },
                );
            }
        },
    }
}

/// The floating card.
pub struct Card {
    /// The layer over the window (scrim + card).
    pub layer: gtk::Box,
    card: gtk::Box,
    field: Rc<FieldCore>,
    tag: gtk::Label,
    rule: gtk::Separator,
    scroller: gtk::ScrolledWindow,
    mode: Cell<Mode>,
    open: Cell<bool>,
    /// Opened by the shell because the space was empty, not by the user.
    auto: Cell<bool>,
}

impl Card {
    pub fn new(shell: &Shell) -> Rc<Card> {
        let layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        layer.add_css_class("copper-scrim");
        layer.set_hexpand(true);
        layer.set_vexpand(true);
        layer.set_visible(false);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("copper-card");
        card.set_halign(gtk::Align::Center);
        card.set_valign(gtk::Align::Start);
        card.set_size_request(620, -1);
        card.update_property(&[gtk::accessible::Property::Label("Open")]);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        head.add_css_class("card-head");
        let glass = gtk::Image::from_icon_name("system-search-symbolic");
        glass.add_css_class("glass");
        head.append(&glass);
        let tag = gtk::Label::new(Some("Private"));
        tag.add_css_class("private-tag");
        tag.set_visible(false);
        let field = FieldCore::new(shell);
        field.text.add_css_class("card-text");
        head.append(&field.text);
        head.append(&tag);
        card.append(&head);

        let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
        rule.add_css_class("card-rule");
        card.append(&rule);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(8 * 44);
        scroller.set_child(Some(&field.list.list));
        card.append(&scroller);
        layer.append(&card);

        let me = Rc::new(Card {
            layer,
            card,
            field,
            tag,
            rule,
            scroller,
            mode: Cell::new(Mode::Launch),
            open: Cell::new(false),
            auto: Cell::new(false),
        });

        let weak_me = Rc::downgrade(&me);
        me.field.on_rows(move |n| {
            if let Some(me) = weak_me.upgrade() {
                me.rule.set_visible(n > 0);
                me.scroller.set_visible(n > 0);
            }
        });
        let weak_me = Rc::downgrade(&me);
        me.field.on_submit(move |shell, row, new_tab| {
            let Some(me) = weak_me.upgrade() else { return };
            let mode = me.mode.get();
            me.close(shell);
            perform(shell, row, mode, new_tab);
        });
        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        me.field.on_cancel(move || {
            if let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) {
                me.close(&s);
            }
        });

        // A click outside the card closes it.
        let click = gtk::GestureClick::new();
        let weak_me = Rc::downgrade(&me);
        let weak = shell.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let (Some(me), Some(s)) = (weak_me.upgrade(), weak.upgrade()) else {
                return;
            };
            let inside = me
                .card
                .compute_bounds(&me.layer)
                .is_some_and(|r| r.contains_point(&gtk::graphene::Point::new(x as f32, y as f32)));
            if !inside {
                g.set_state(gtk::EventSequenceState::Claimed);
                me.close(&s);
            }
        });
        me.layer.add_controller(click);
        me
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn text(&self) -> gtk::Text {
        self.field.text.clone()
    }

    pub fn mode(&self) -> Mode {
        self.mode.get()
    }

    /// Open because the space has nothing in it; a tab arriving closes it.
    pub fn open_empty(&self, shell: &Shell) {
        self.open(shell, Mode::Launch);
        self.auto.set(true);
    }

    /// Opened for an empty space and not touched since.
    pub fn is_auto(&self) -> bool {
        self.open.get() && self.auto.get() && self.field.text.text().is_empty()
    }

    pub fn open(&self, shell: &Shell, mode: Mode) {
        self.auto.set(false);
        self.mode.set(mode);
        self.open.set(true);
        let private =
            mode == Mode::LaunchPrivate || (mode == Mode::Edit && shell.active_is_private());
        self.tag.set_visible(private);
        if private {
            self.card.add_css_class("private");
        } else {
            self.card.remove_css_class("private");
        }
        // Over a page the window dims; over an empty space it does not.
        let over = shell.active_id().is_some();
        if over {
            self.layer.add_css_class("over");
        } else {
            self.layer.remove_css_class("over");
        }
        let h = shell.window().height().max(600);
        self.card.set_margin_top((h as f64 * 0.2) as i32);
        self.field
            .text
            .set_placeholder_text(Some(mode.placeholder()));
        let text = match mode {
            Mode::Edit => shell
                .browser()
                .active()
                .map(|t| t.url.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        self.layer.set_visible(true);
        self.field.begin(shell, mode.purpose(), &text);
    }

    pub fn toggle(&self, shell: &Shell, mode: Mode) {
        if self.open.get() && self.mode.get() == mode {
            self.close(shell);
        } else {
            self.open(shell, mode);
        }
    }

    pub fn close(&self, shell: &Shell) {
        if !self.open.replace(false) {
            return;
        }
        self.layer.set_visible(false);
        self.field.set_quiet("");
        if let Some(p) = shell.active_page() {
            p_focus(&p);
        }
    }
}

/// Give the page the keyboard.
pub fn p_focus(page: &copper_engine_webkitgtk::WebKitPage) {
    page.widget().grab_focus();
}
