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

/// A selection the field made itself, in `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Chosen {
    text: String,
    start: i32,
    end: i32,
}

impl Chosen {
    /// `bounds` (GTK's, ordered) are this selection.
    fn holds(&self, bounds: Option<(i32, i32)>) -> bool {
        bounds == Some((self.start.min(self.end), self.start.max(self.end)))
            || (self.start == self.end && bounds.is_none())
    }
}

/// Shift, Control, Alt, Super… on their own.
fn is_modifier(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(
        key,
        Key::Shift_L
            | Key::Shift_R
            | Key::Control_L
            | Key::Control_R
            | Key::Alt_L
            | Key::Alt_R
            | Key::Meta_L
            | Key::Meta_R
            | Key::Super_L
            | Key::Super_R
            | Key::ISO_Level3_Shift
            | Key::Caps_Lock
    )
}

/// A field with inline completion and suggestions.
pub struct FieldCore {
    pub text: gtk::Text,
    pub list: Rc<SuggestionList>,
    purpose: Cell<Purpose>,
    /// What was really typed (without the offered completion).
    typed: RefCell<String>,
    quiet: Cell<bool>,
    /// Bumped on every typed change: a completion queued for one change
    /// never lands on a later one.
    generation: Cell<u64>,
    /// A completion (and the rows) wait for GTK to finish the keystroke.
    deferred: Cell<bool>,
    /// The selection the field made itself (the offered rest, or the whole
    /// address on Ctrl+L) and the text it was made in, until the user
    /// types, clicks or moves the caret.
    chosen: RefCell<Option<Chosen>>,
    /// A pointer press is being handled: what it selects is the user's.
    pressing: Cell<bool>,
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
            generation: Cell::new(0),
            deferred: Cell::new(false),
            chosen: RefCell::default(),
            pressing: Cell::new(false),
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
            FieldCore::changed(&core, &shell);
        });

        // On X11 the selection is also the PRIMARY selection. Typing over
        // it gives PRIMARY up and selecting the next completion claims it
        // again; the X server's word that it was given up can arrive after
        // the new claim, and GTK then drops the new selection, leaving the
        // caret before the offered rest — the next key lands inside it
        // ("news.ycombinator.comnator.com"). The field puts back a
        // selection it made itself whenever that happens.
        let weak_core = Rc::downgrade(&core);
        core.text
            .connect_notify_local(Some("cursor-position"), move |_, _| {
                let Some(core) = weak_core.upgrade() else {
                    return;
                };
                if !core.lost_choice() {
                    return;
                }
                let weak_core = Rc::downgrade(&core);
                glib::idle_add_local_full(glib::Priority::HIGH, move || {
                    if let Some(core) = weak_core.upgrade() {
                        core.restore_choice();
                    }
                    glib::ControlFlow::Break
                });
            });
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak_core = Rc::downgrade(&core);
        click.connect_pressed(move |_, _, _, _| {
            let Some(core) = weak_core.upgrade() else {
                return;
            };
            // A click that focuses the field selects through `begin`, then
            // puts the caret where it landed: that is not a selection to
            // keep. Over once the press has been handled.
            core.chosen.take();
            core.pressing.set(true);
            let weak_core = Rc::downgrade(&core);
            glib::idle_add_local_full(glib::Priority::HIGH, move || {
                if let Some(core) = weak_core.upgrade() {
                    core.pressing.set(false);
                }
                glib::ControlFlow::Break
            });
        });
        core.text.add_controller(click);
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
            // A key acts on the selection the field made, even if X11 took
            // it away a moment ago; after a key that is not walking the
            // rows, what is selected is the user's business.
            core.restore_choice();
            let walks_rows = matches!(key, Key::Down | Key::Up)
                || (ctrl && matches!(key, Key::n | Key::j | Key::p));
            if !walks_rows && !is_modifier(key) {
                core.chosen.take();
            }
            // Moving through the rows or taking the completion acts on what
            // the last keystroke would show, even if it is still waiting.
            if walks_rows || key == Key::Tab {
                core.settle(&shell);
            }
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
                    core.accept_completion(&shell);
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
        self.deferred.set(false);
        self.chosen.take();
        self.generation.set(self.generation.get() + 1);
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
        self.choose(0, -1);
        self.refresh(shell);
    }

    fn changed(core: &Rc<FieldCore>, shell: &Shell) {
        if core.quiet.get() {
            return;
        }
        let now = core.text.text().to_string();
        let before = core.typed.replace(now.clone());
        core.chosen.take();
        let generation = core.generation.get() + 1;
        core.generation.set(generation);
        if !(complete::grew(&before, &now) && core.purpose.get() != Purpose::Palette) {
            core.deferred.set(false);
            core.refresh(shell);
            return;
        }
        // `changed` runs while GTK is still inside the keystroke: typing over
        // the offered rest is delete, insert, then GTK puts the caret after
        // the new character. Touching the text now loses the selection and
        // leaves the caret mid-word, so the next key lands inside the
        // completion. Offer it once the keystroke is done — before the next
        // frame is drawn, after any keys already queued.
        core.deferred.set(true);
        let weak_core = Rc::downgrade(core);
        let weak = shell.downgrade();
        glib::idle_add_local_full(glib::Priority::HIGH_IDLE, move || {
            if let (Some(core), Some(shell)) = (weak_core.upgrade(), weak.upgrade())
                && core.generation.get() == generation
            {
                core.settle(&shell);
            }
            glib::ControlFlow::Break
        });
    }

    /// Do what the last keystroke left waiting: the completion, the rows.
    fn settle(&self, shell: &Shell) {
        if !self.deferred.replace(false) {
            return;
        }
        self.offer_completion(shell);
        self.refresh(shell);
    }

    /// Add the rest of a known address after what was typed, selected, if
    /// the field still holds exactly that with the caret at its end.
    fn offer_completion(&self, shell: &Shell) {
        let now = self.text.text().to_string();
        let caret = self.text.position();
        let selection = self.text.selection_bounds();
        if !complete::may_offer(&now, &self.typed.borrow(), caret, selection) {
            return;
        }
        let (tabs, visited) = shell.suggestions(&now, 6);
        let Some(rest) = complete::inline(&now, &tabs, &visited) else {
            return;
        };
        let start = now.chars().count() as i32;
        let mut at = start;
        self.quiet.set(true);
        self.text.insert_text(&rest, &mut at);
        self.quiet.set(false);
        self.choose(start, -1);
    }

    /// Select `start..end` (characters, `-1` = the end) and remember it as
    /// the field's own selection.
    fn choose(&self, start: i32, end: i32) {
        self.text.select_region(start, end);
        if self.pressing.get() {
            return;
        }
        let len = self.text.text().chars().count() as i32;
        let end = if end < 0 { len } else { end.min(len) };
        self.chosen.replace(Some(Chosen {
            text: self.text.text().to_string(),
            start: start.clamp(0, len),
            end,
        }));
    }

    /// The field's own selection is gone though nothing was typed.
    fn lost_choice(&self) -> bool {
        let chosen = self.chosen.borrow();
        let Some(c) = chosen.as_ref() else {
            return false;
        };
        c.text == self.text.text().as_str() && !c.holds(self.text.selection_bounds())
    }

    /// Put the field's own selection back if it was taken away.
    fn restore_choice(&self) {
        if !self.lost_choice() {
            return;
        }
        let Some((start, end)) = self.chosen.borrow().as_ref().map(|c| (c.start, c.end)) else {
            return;
        };
        tracing::debug!(start, end, "field selection put back");
        self.text.select_region(start, end);
    }

    /// Take the offered completion: the caret after it, nothing opened.
    fn accept_completion(&self, shell: &Shell) {
        // Tab pressed before the main loop got to the completion.
        self.settle(shell);
        let full = self.text.text().to_string();
        self.typed.replace(full.clone());
        self.chosen.take();
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
        // Return pressed before the main loop got to the last keystroke's
        // completion and rows: settle them, so Return acts on what the
        // field would show.
        self.settle(shell);
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

#[cfg(test)]
mod tests {
    use super::Chosen;

    fn chosen(start: i32, end: i32) -> Chosen {
        Chosen {
            text: "news.ycombinator.com".into(),
            start,
            end,
        }
    }

    #[test]
    fn the_fields_own_selection_is_recognised_until_it_is_lost() {
        // The offered rest, as GTK reports it (ordered).
        assert!(chosen(4, 20).holds(Some((4, 20))));
        assert!(chosen(20, 4).holds(Some((4, 20))));
        // X11 dropped it: the caret sits before the rest, nothing selected.
        assert!(!chosen(4, 20).holds(None));
        assert!(!chosen(4, 20).holds(Some((4, 4))));
        assert!(!chosen(0, 20).holds(Some((0, 4))));
        // A caret the field placed itself is held by "no selection".
        assert!(chosen(20, 20).holds(None));
    }
}
