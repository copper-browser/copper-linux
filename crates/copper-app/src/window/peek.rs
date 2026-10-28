//! The Calendar pin's card (Mac: `Fork/CalendarPeek.swift`, `Side.swift`
//! `PinSquare`): rest the pointer on a Google Calendar pin for half a
//! second and the next meetings show beside it; the card goes a quarter
//! second after the pointer has left both the pin and the card
//! ([`calendar::settle`]).
//!
//! The card is a popover that doesn't hide on outside clicks, so it never
//! takes the keyboard from the page, and nothing in it can be focused.
//! What it lists is copper-shell-core's [`calendar`].

use crate::platform::Glib;
use crate::shell::GtkShell;
use crate::shell::{OpenTab, Shell};
use copper_core::TabId;
use copper_shell_core::calendar::{self, Card, Meeting};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita::prelude::PreferencesDialogExt;
use std::cell::RefCell;

/// The card's width (the Mac's 290).
const WIDTH: i32 = 290;

#[derive(Default)]
struct Peek {
    /// The calendar pin under the pointer.
    pin: Option<TabId>,
    /// The pointer is on the card.
    card: bool,
    /// The card, while it is up, and the pin it is beside.
    open: Option<(gtk::Popover, TabId)>,
    timer: Option<glib::SourceId>,
}

thread_local! {
    static PEEK: RefCell<Peek> = RefCell::default();
}

/// Watch pin `root` (tab `id`): a rest on it opens the card while the tab
/// is on Google Calendar.
pub fn watch(shell: &Shell, root: &gtk::Widget, id: &TabId) {
    let motion = gtk::EventControllerMotion::new();
    let (weak, tab) = (shell.downgrade(), id.clone());
    motion.connect_enter(move |_, _, _| {
        if let Some(s) = weak.upgrade() {
            rest(&s, &tab);
        }
    });
    let (weak, tab) = (shell.downgrade(), id.clone());
    motion.connect_leave(move |_| {
        if let Some(s) = weak.upgrade() {
            left(&s, &tab);
        }
    });
    root.add_controller(motion);
}

/// The pointer came onto pin `id`. The self-test calls this and [`left`]
/// itself: headless weston has no pointer.
pub fn rest(shell: &Shell, id: &TabId) {
    if !shell.tab_view(id).is_some_and(|t| calendar::peeks(&t.url)) {
        return;
    }
    PEEK.with_borrow_mut(|p| p.pin = Some(id.clone()));
    settle(shell);
}

/// The pointer left pin `id`.
pub fn left(shell: &Shell, id: &TabId) {
    PEEK.with_borrow_mut(|p| {
        if p.pin.as_ref() == Some(id) {
            p.pin = None;
        }
    });
    settle(shell);
}

/// Open the card once the pointer has rested, or put it away once it has
/// left; whatever was waiting to happen is called off.
fn settle(shell: &Shell) {
    PEEK.with_borrow_mut(|p| {
        if let Some(t) = p.timer.take() {
            t.remove();
        }
        let showing = p
            .open
            .as_ref()
            .filter(|(pop, _)| pop.is_mapped())
            .map(|(_, t)| t);
        let Some(open) = calendar::settle(p.pin.as_ref(), p.card, showing) else {
            return;
        };
        let weak = shell.downgrade();
        p.timer = Some(glib::timeout_add_local_once(
            if open {
                calendar::REST
            } else {
                calendar::LEAVE
            },
            move || {
                PEEK.with_borrow_mut(|p| p.timer = None);
                match weak.upgrade() {
                    Some(s) if open => show(&s),
                    _ => hide(),
                }
            },
        ));
    });
}

fn show(shell: &Shell) {
    let Some(tab) = PEEK.with_borrow(|p| p.pin.clone()) else {
        return;
    };
    // Hung on the column, which outlives its squares (the pins are drawn
    // again as tabs change): a redraw under the card leaves it up.
    let column = shell.ui().column.widget.clone();
    let Some(at) = shell
        .ui()
        .column
        .pin(&tab)
        .and_then(|square| square.compute_bounds(&column))
    else {
        return;
    };
    hide();
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.set_size_request(WIDTH, -1);
    let pop = gtk::Popover::new();
    pop.add_css_class("calendar-peek");
    pop.set_autohide(false);
    pop.set_can_focus(false);
    pop.set_position(gtk::PositionType::Right);
    pop.set_child(Some(&body));
    pop.set_parent(&column);
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
        at.x() as i32,
        at.y() as i32,
        at.width() as i32,
        at.height() as i32,
    )));
    let motion = gtk::EventControllerMotion::new();
    let weak = shell.downgrade();
    motion.connect_enter(move |_, _, _| {
        PEEK.with_borrow_mut(|p| p.card = true);
        if let Some(s) = weak.upgrade() {
            settle(&s);
        }
    });
    let weak = shell.downgrade();
    motion.connect_leave(move |_| {
        PEEK.with_borrow_mut(|p| p.card = false);
        if let Some(s) = weak.upgrade() {
            settle(&s);
        }
    });
    pop.add_controller(motion);
    let later = body.downgrade();
    let card = calendar::open(shell, move |s, card| {
        if let Some(body) = later.upgrade().filter(|b| b.is_mapped()) {
            fill(s, &body, &card);
        }
    });
    fill(shell, &body, &card);
    pop.popup();
    PEEK.with_borrow_mut(|p| p.open = Some((pop, tab)));
}

/// Put the card away.
pub fn hide() {
    let Some((pop, _)) = PEEK.with_borrow_mut(|p| {
        p.card = false;
        p.open.take()
    }) else {
        return;
    };
    pop.popdown();
    // Not inside its own signal handlers.
    glib::idle_add_local_once(move || pop.unparent());
}

/// The card's words, for the self-test: the open card's lines, top to
/// bottom; none while it is away.
pub fn lines() -> Vec<String> {
    let Some(pop) = PEEK.with_borrow(|p| p.open.as_ref().map(|(pop, _)| pop.clone())) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut stack = pop.child().into_iter().collect::<Vec<_>>();
    while let Some(w) = stack.pop() {
        if let Some(l) = w.downcast_ref::<gtk::Label>() {
            out.push(l.text().to_string());
        } else if let Some(b) = w.downcast_ref::<gtk::Button>() {
            out.push(format!("[{}]", b.label().unwrap_or_default()));
            continue;
        }
        let mut child = w.last_child();
        while let Some(c) = child {
            child = c.prev_sibling();
            stack.push(c);
        }
    }
    out
}

/// The open card, for the self-test's picture.
pub fn popover() -> Option<gtk::Popover> {
    PEEK.with_borrow(|p| p.open.as_ref().map(|(pop, _)| pop.clone()))
}

fn fill(shell: &Shell, body: &gtk::Box, card: &Card) {
    while let Some(c) = body.first_child() {
        body.remove(&c);
    }
    body.append(&label(calendar::HEADING, &["heading"]));
    match card {
        Card::NoFeeds => {
            let note = label(calendar::NO_FEEDS, &["note"]);
            note.set_wrap(true);
            body.append(&note);
            let open = small_button(calendar::OPEN_SETTINGS, false);
            open.set_halign(gtk::Align::Start);
            open.add_css_class("settings");
            let weak = shell.downgrade();
            open.connect_clicked(move |_| {
                hide();
                if let Some(s) = weak.upgrade() {
                    crate::window::settings::dialog(&s)
                        .set_visible_page_name(crate::features::settings::calendar::PAGE);
                }
            });
            body.append(&open);
        }
        Card::Loading => body.append(&label(calendar::LOADING, &["quiet"])),
        Card::Meetings { list, error } => {
            if list.is_empty() && error.is_none() {
                body.append(&label(calendar::NOTHING, &["quiet"]));
            }
            let now = copper_core::date::now_unix() as i64;
            for (i, m) in list.iter().enumerate() {
                if i > 0 {
                    let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
                    rule.add_css_class("rule");
                    body.append(&rule);
                }
                body.append(&row(shell, m, now));
            }
            if let Some(e) = error {
                let l = label(e, &["error"]);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                l.set_tooltip_text(Some(e));
                body.append(&l);
            }
        }
    }
}

/// One meeting: its calendar's dot, the title on one line, when, and Join.
fn row(shell: &Shell, m: &Meeting, now: i64) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("meeting");
    let ink = gtk::gdk::RGBA::parse(m.color.as_str()).unwrap_or(gtk::gdk::RGBA::BLUE);
    let dot = gtk::DrawingArea::new();
    dot.set_content_width(7);
    dot.set_content_height(7);
    dot.set_valign(gtk::Align::Center);
    dot.set_draw_func(move |_, cr, w, h| {
        cr.set_source_rgba(
            ink.red() as f64,
            ink.green() as f64,
            ink.blue() as f64,
            ink.alpha() as f64,
        );
        cr.arc(
            w as f64 / 2.0,
            h as f64 / 2.0,
            3.5,
            0.0,
            std::f64::consts::TAU,
        );
        let _ = cr.fill();
    });
    row.append(&dot);
    let words = gtk::Box::new(gtk::Orientation::Vertical, 2);
    words.set_hexpand(true);
    let title = label(&m.title, &["title"]);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    words.append(&title);
    let when = label(&calendar::when::<Glib>(m, now), &["when"]);
    if m.start <= now {
        when.add_css_class("now");
    }
    when.set_ellipsize(gtk::pango::EllipsizeMode::End);
    words.append(&when);
    row.append(&words);
    if let Some(link) = &m.join {
        let join = small_button(calendar::JOIN, true);
        join.set_valign(gtk::Align::Center);
        join.set_tooltip_text(Some(&crate::shell::host_of(link)));
        let (weak, link) = (shell.downgrade(), link.clone());
        join.connect_clicked(move |_| {
            hide();
            if let Some(s) = weak.upgrade() {
                s.open(
                    &link,
                    OpenTab {
                        foreground: true,
                        ..Default::default()
                    },
                );
            }
        });
        row.append(&join);
    }
    row
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    // The card's width is its own: a long title ellipsizes, a note wraps.
    l.set_width_chars(1);
    l.set_max_width_chars(1);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A button that never takes focus from the page.
fn small_button(text: &str, filled: bool) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.set_focusable(false);
    b.set_focus_on_click(false);
    b.add_css_class("pill");
    if filled {
        b.add_css_class("suggested-action");
    }
    b
}
