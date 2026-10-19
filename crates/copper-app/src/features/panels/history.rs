//! History (Ctrl+Y; Mac: `Recall.swift` `HistoryPanel`): everywhere you
//! have been, newest first, grouped by day, searchable; a line can go; the
//! foot clears history, sign-ins or the cache — three separate things,
//! worded so nobody has to guess which one signs them out of their bank.

use super::{Refresh, close, escape_closes, go, site_mark};
use crate::features::format;
use crate::features::settings::privacy;
use crate::features::ui;
use crate::platform::Glib;
use crate::shell::Shell;
use copper_core::history::Trace;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Rows drawn at most; search reaches the rest.
const SHOWN: usize = 400;

struct HistoryPanel {
    frame: ui::Frame,
    search: gtk::SearchEntry,
    content: gtk::Box,
    count: gtk::Label,
    clear: gtk::Button,
    clearing: Cell<bool>,
}

pub fn build(shell: &Shell) -> (gtk::Widget, Refresh) {
    let weak = shell.downgrade();
    let frame = ui::frame("History", move || {
        if let Some(s) = weak.upgrade() {
            close(&s);
        }
    });
    let search = ui::search("Search everywhere you have been");
    frame.body.append(&search);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.body.append(&ui::scroller(&content));
    let count = ui::count("");
    count.set_hexpand(true);
    frame.foot.append(&count);
    let clear = ui::pill("Clear…", false);
    frame.foot.append(&clear);
    escape_closes(shell, &frame.root);

    let panel = Rc::new(HistoryPanel {
        frame,
        search,
        content,
        count,
        clear,
        clearing: Cell::new(false),
    });
    let weak = shell.downgrade();
    let p = panel.clone();
    panel.search.connect_search_changed(move |_| {
        if let Some(s) = weak.upgrade() {
            p.draw(&s);
        }
    });
    let weak = shell.downgrade();
    let p = panel.clone();
    panel.search.connect_activate(move |_| {
        // Return opens the first line.
        if let Some(s) = weak.upgrade()
            && let Some(t) = s.history().search(&p.search.text()).first().cloned()
        {
            close(&s);
            go(&s, &t.url);
        }
    });
    let weak = shell.downgrade();
    let p = panel.clone();
    panel.clear.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            p.clearing.set(!p.clearing.get());
            p.draw(&s);
        }
    });
    let root = panel.frame.root.clone().upcast();
    let p = panel.clone();
    let refresh: Refresh = Rc::new(move |s: &Shell| {
        p.clearing.set(false);
        p.draw(s);
        p.search.grab_focus();
    });
    (root, refresh)
}

impl HistoryPanel {
    fn draw(self: &Rc<Self>, shell: &Shell) {
        ui::clear(&self.content);
        if self.clearing.get() {
            self.draw_sweeps(shell);
            return;
        }
        self.clear.set_label("Clear…");
        self.search.set_visible(true);
        let typed = self.search.text().to_string();
        let traces = shell.history().search(&typed);
        self.count.set_text(&match traces.len() {
            1 => "1 page".to_string(),
            n => format!("{n} pages"),
        });
        if traces.is_empty() {
            self.content.append(&ui::empty(if typed.trim().is_empty() {
                "Nothing yet."
            } else {
                "Nothing matches."
            }));
            return;
        }
        let mut day = i64::MIN;
        let mut list: Option<gtk::ListBox> = None;
        for t in traces.iter().take(SHOWN) {
            let unix = t.last.unix();
            let d = format::day_number::<Glib>(unix);
            if d != day {
                day = d;
                self.content
                    .append(&ui::caption(&format::day::<Glib>(unix)));
                let l = ui::list();
                let weak = shell.downgrade();
                l.connect_row_activated(move |_, row| {
                    if let Some(s) = weak.upgrade() {
                        let url = row.widget_name().to_string();
                        close(&s);
                        go(&s, &url);
                    }
                });
                self.content.append(&l);
                list = Some(l);
            }
            if let Some(l) = &list {
                l.append(&self.row(shell, t));
            }
        }
        if traces.len() > SHOWN {
            let more = ui::caption(&format!(
                "The newest {SHOWN} — search to find the other {}",
                traces.len() - SHOWN
            ));
            self.content.append(&more);
        }
    }

    /// A title, where it came from, and when — the three things you scan
    /// for, in the order you scan them.
    fn row(self: &Rc<Self>, shell: &Shell, t: &Trace) -> gtk::ListBoxRow {
        let title = if t.title.trim().is_empty() {
            &t.key
        } else {
            &t.title
        };
        let mark = site_mark(shell, &t.url, &t.key);
        let r = ui::row(&mark, title, &t.key);
        r.row.set_widget_name(&t.url);
        r.row.set_tooltip_text(Some(&t.url));
        let time = gtk::Label::new(Some(&format::clock::<Glib>(t.last.unix())));
        time.add_css_class("row-side");
        r.end.append(&time);
        let forget = ui::icon_button("window-close-symbolic", "Remove from history");
        let weak = shell.downgrade();
        let key = t.key.clone();
        let p = self.clone();
        forget.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                if s.history_mut().forget(&key) {
                    s.history_changed();
                }
                p.draw(&s);
            }
        });
        r.end.append(&forget);
        r.row
    }

    fn draw_sweeps(self: &Rc<Self>, shell: &Shell) {
        self.clear.set_label("Back");
        self.search.set_visible(false);
        self.count.set_text("Choose what to clear");
        self.content.append(&ui::caption(
            "Each one on its own — nothing else goes with it",
        ));
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("panel-card");
        let list = ui::list();
        let p = self.clone();
        type Sweep = (&'static str, &'static str, &'static str, Rc<dyn Fn(&Shell)>);
        let sweeps: [Sweep; 3] = [
            (
                "History",
                "Everywhere you have been",
                "Clear",
                Rc::new(move |s: &Shell| {
                    privacy::clear_history(s);
                    p.clearing.set(false);
                    p.draw(s);
                }),
            ),
            (
                "Cookies and sign-ins",
                "Signs you out of every site",
                "Sign out of everything",
                Rc::new(privacy::clear_sites),
            ),
            (
                "Cache",
                "Only what was fetched to draw pages",
                "Clear",
                Rc::new(privacy::clear_cache),
            ),
        ];
        for (title, detail, act, f) in sweeps {
            let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let r = ui::row(&mark, title, detail);
            r.row.set_activatable(false);
            let b = ui::pill(act, false);
            let weak = shell.downgrade();
            b.connect_clicked(move |_| {
                if let Some(s) = weak.upgrade() {
                    f(&s);
                }
            });
            r.end.append(&b);
            list.append(&r.row);
        }
        card.append(&list);
        self.content.append(&card);
    }
}
