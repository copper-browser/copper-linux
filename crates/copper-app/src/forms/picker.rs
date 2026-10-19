//! The account list under a sign-in box (Mac `Browser.Suggesting`): a
//! small list anchored to the box the caret is in. It never takes the
//! keyboard focus from the page, and nothing is filled until a row is
//! clicked; the secret is read only then.

use super::{Forms, Row, RowKind, source_title};
use crate::shell::Shell;
use crate::shell::pages::PageSlot;
use copper_core::TabId;
use copper_pagejs::messages::FieldRect;
use copper_shell_core::forms::PickerView;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// The list on a page.
pub struct Picker {
    root: gtk::Box,
    frame: glib::WeakRef<gtk::Overlay>,
    list: gtk::ListBox,
    users: Vec<String>,
    busy: Rc<Cell<bool>>,
    rows: usize,
}

const ROW_HEIGHT: f64 = 46.0;

impl Picker {
    /// Click row `i` (self-test), on the next turn of the loop.
    pub fn press(&self, i: usize) {
        if let Some(r) = self.list.row_at_index(i as i32) {
            glib::idle_add_local_once(move || r.emit_activate());
        }
    }
}

impl PickerView for Picker {
    fn remove(&self) {
        if let Some(f) = self.frame.upgrade() {
            f.remove_overlay(&self.root);
        }
    }

    fn users(&self) -> Vec<String> {
        self.users.clone()
    }

    fn busy(&self) -> bool {
        self.busy.get()
    }

    /// Hang under the box (or over it, near the foot of the page).
    fn place(&self, rect: FieldRect, zoom: f64) {
        let frame = self.frame.upgrade();
        let (fw, fh) = frame
            .as_ref()
            .map(|f| (f.width() as f64, f.height() as f64))
            .unwrap_or((0.0, 0.0));
        let width = (rect.w * zoom).clamp(240.0, 380.0);
        let height = self.rows as f64 * ROW_HEIGHT + 10.0;
        let mut x = (rect.x * zoom).max(6.0);
        if fw > 0.0 && x + width > fw - 6.0 {
            x = (fw - width - 6.0).max(6.0);
        }
        let below = (rect.y + rect.h) * zoom + 4.0;
        let y = if fh > 0.0 && below + height > fh - 6.0 && rect.y * zoom - height - 4.0 > 6.0 {
            rect.y * zoom - height - 4.0
        } else {
            below.max(6.0)
        };
        self.root.set_size_request(width as i32, -1);
        self.root.set_margin_start(x as i32);
        self.root.set_margin_top(y as i32);
    }
}

fn row_widget(row: &Row) -> gtk::ListBoxRow {
    let c = &row.credential;
    let r = gtk::ListBoxRow::new();
    r.set_focusable(false);
    r.set_activatable(true);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let icon = gtk::Image::from_icon_name(match row.kind {
        RowKind::Login => "avatar-default-symbolic",
        RowKind::Code => "preferences-system-time-symbolic",
    });
    icon.add_css_class("key");
    b.append(&icon);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    let user = match row.kind {
        RowKind::Login if c.user.is_empty() => "No username".to_string(),
        RowKind::Login => c.user.clone(),
        RowKind::Code if c.user.is_empty() => "One-time code".to_string(),
        RowKind::Code => format!("Code for {}", c.user),
    };
    let top = gtk::Label::new(Some(&user));
    top.add_css_class("user");
    top.set_xalign(0.0);
    top.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let where_ = format!("{} · {}", c.host, source_title(c.source));
    let bottom = gtk::Label::new(Some(&where_));
    bottom.add_css_class("where");
    bottom.set_xalign(0.0);
    bottom.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&top);
    text.append(&bottom);
    text.set_hexpand(true);
    b.append(&text);
    r.set_child(Some(&b));
    r.update_property(&[gtk::accessible::Property::Label(&format!(
        "{user}, {where_}"
    ))]);
    r
}

pub fn show(
    _shell: &Shell,
    forms: &Rc<Forms>,
    tab: &TabId,
    slot: &PageSlot,
    rect: FieldRect,
    rows: Vec<Row>,
) -> Picker {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("copper-forms-picker");
    root.set_halign(gtk::Align::Start);
    root.set_valign(gtk::Align::Start);
    root.set_focusable(false);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_activate_on_single_click(true);
    list.set_focusable(false);
    list.update_property(&[gtk::accessible::Property::Label("Saved accounts")]);
    for r in &rows {
        list.append(&row_widget(r));
    }
    root.append(&list);
    let busy = Rc::new(Cell::new(false));
    {
        let rows = rows.clone();
        let weak = Rc::downgrade(forms);
        let tab = tab.clone();
        let busy = busy.clone();
        list.connect_row_activated(move |_, r| {
            if busy.get() {
                return;
            }
            let Some(row) = rows.get(r.index() as usize).cloned() else {
                return;
            };
            busy.set(true);
            if let Some(b) = r.child().and_downcast::<gtk::Box>()
                && let Some(text) = b.last_child().and_downcast::<gtk::Box>()
                && let Some(l) = text.last_child().and_downcast::<gtk::Label>()
            {
                l.set_text("Filling…");
            }
            if let Some(f) = weak.upgrade() {
                f.pick(&tab, row);
            }
        });
    }
    slot.frame.add_overlay(&root);
    let frame = glib::WeakRef::new();
    frame.set(Some(slot.frame.overlay()));
    let users = rows
        .iter()
        .map(|r| match r.kind {
            RowKind::Login => r.credential.user.clone(),
            RowKind::Code => format!("code:{}", r.credential.user),
        })
        .collect();
    let p = Picker {
        root,
        frame,
        list,
        users,
        busy,
        rows: rows.len(),
    };
    p.place(rect, copper_engine::PageView::zoom(&slot.page));
    p
}
