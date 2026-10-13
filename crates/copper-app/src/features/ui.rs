//! The lane's look: the side panels (History, Bookmarks, Downloads, Hidden)
//! in the shell's own palette (`@copper_ground`, `@copper_ink`), and the
//! few widgets they share. Separation is spacing and one hairline; the only
//! colour is the page's own icons.

use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::Cell;

const CSS: &str = r#"
.copper-panel { background-color: @copper_ground; color: @copper_ink; box-shadow: inset 1px 0 alpha(@copper_ink, .09); }
.copper-panel .panel-head { min-height: 46px; padding: 0 8px 0 16px; }
.copper-panel .panel-title { font-size: 15px; font-weight: 600; }
.copper-panel .panel-subtitle { font-size: 12px; opacity: .6; }
.copper-panel button.flat-icon {
  min-width: 26px; min-height: 26px; padding: 0; border-radius: 7px;
  background: transparent; box-shadow: none; color: alpha(@copper_ink, .62);
  transition: background-color 100ms ease-out, opacity 100ms ease-out;
}
.copper-panel button.flat-icon:hover { background-color: alpha(@copper_ink, .07); color: @copper_ink; }
.copper-panel entry.panel-search {
  margin: 0 12px 6px 12px; min-height: 32px; border-radius: 9px; padding: 0 8px;
  background-color: alpha(@copper_ink, .055); box-shadow: none; border: none; outline: none;
}
.copper-panel entry.panel-search:focus-within { background-color: alpha(@copper_ink, .075); box-shadow: inset 0 0 0 1px alpha(@copper_ink, .12); }
.copper-panel .panel-caption { font-size: 12px; font-weight: 600; opacity: .55; margin: 14px 16px 4px 16px; }
.copper-panel list { background: transparent; }
.copper-panel row.panel-row {
  min-height: 42px; padding: 6px 8px 6px 10px; margin: 0 6px; border-radius: 9px;
  background: transparent; transition: background-color 80ms ease-out;
}
.copper-panel row.panel-row:hover { background-color: alpha(@copper_ink, .05); }
.copper-panel row.panel-row:focus-visible { outline: 2px solid alpha(@accent_color, .6); outline-offset: -2px; }
.copper-panel row.panel-row:selected { background-color: alpha(@copper_ink, .07); }
.copper-panel row.panel-row .row-title { font-size: 13.5px; }
.copper-panel row.panel-row .row-detail { font-size: 12px; opacity: .58; font-feature-settings: "tnum"; }
.copper-panel row.panel-row .row-side { font-size: 12px; opacity: .55; font-feature-settings: "tnum"; }
.copper-panel row.panel-row .row-folder { font-size: 13.5px; font-weight: 600; }
.copper-panel row.panel-row.missing .row-title { opacity: .5; }
.copper-panel row.panel-row button.flat-icon { opacity: 0; }
.copper-panel row.panel-row:hover button.flat-icon, .copper-panel row.panel-row:focus-within button.flat-icon,
.copper-panel row.panel-row button.flat-icon.keep { opacity: 1; }
.copper-panel button.pill {
  min-height: 28px; padding: 0 12px; border-radius: 99px; font-size: 12.5px; font-weight: 500;
  background-color: alpha(@copper_ink, .07); color: @copper_ink; box-shadow: none; border: none;
}
.copper-panel button.pill:hover { background-color: alpha(@copper_ink, .11); }
.copper-panel button.pill.filled { background-color: @copper_ink; color: @copper_ground; }
.copper-panel button.pill.filled:hover { background-color: alpha(@copper_ink, .86); }
.copper-panel button.pill.danger { color: #b3261e; }
.copper-panel .panel-foot { min-height: 46px; padding: 0 10px 0 16px; box-shadow: inset 0 1px alpha(@copper_ink, .08); }
.copper-panel .panel-foot .count { font-size: 12px; opacity: .58; font-feature-settings: "tnum"; }
.copper-panel .panel-empty { font-size: 13.5px; opacity: .55; margin: 48px 28px; }
.copper-panel .panel-card { margin: 4px 12px; padding: 4px 0; border-radius: 12px; background-color: alpha(@copper_ink, .035); }
.copper-panel .panel-card row.panel-row { margin: 0 4px; }
.copper-panel .panel-note { font-size: 12px; opacity: .6; margin: 6px 18px 0 18px; }
.copper-panel .bar trough { min-height: 2px; background: alpha(@copper_ink, .08); border: none; box-shadow: none; }
.copper-panel .bar progress { min-height: 2px; background-color: alpha(@copper_ink, .7); border: none; }
.copper-panel .bar { margin-top: 5px; }
.copper-panel .file-mark { opacity: .72; }
.copper-panel .chip { background-color: alpha(@copper_ink, .08); color: alpha(@copper_ink, .62); }
.copper-settings-note { font-size: 12.5px; opacity: .65; }
"#;

/// Load the lane's stylesheet once.
pub fn install_css() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.with(|d| d.replace(true)) {
        return;
    }
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

/// A panel's frame: a head with its title and a close button, a body, a foot.
pub struct Frame {
    pub root: gtk::Box,
    pub title: gtk::Label,
    pub subtitle: gtk::Label,
    pub body: gtk::Box,
    pub foot: gtk::Box,
}

pub fn frame(title: &str, close: impl Fn() + 'static) -> Frame {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("copper-panel");
    root.set_size_request(360, -1);
    root.set_hexpand(false);
    root.set_vexpand(true);

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    head.add_css_class("panel-head");
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 0);
    titles.set_valign(gtk::Align::Center);
    titles.set_hexpand(true);
    let t = gtk::Label::new(Some(title));
    t.add_css_class("panel-title");
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.set_accessible_role(gtk::AccessibleRole::Heading);
    titles.append(&t);
    let sub = gtk::Label::new(None);
    sub.add_css_class("panel-subtitle");
    sub.set_xalign(0.0);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub.set_visible(false);
    titles.append(&sub);
    head.append(&titles);
    let x = icon_button("window-close-symbolic", "Close");
    x.add_css_class("keep");
    x.connect_clicked(move |_| close());
    head.append(&x);
    root.append(&head);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.set_vexpand(true);
    root.append(&body);

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    foot.add_css_class("panel-foot");
    root.append(&foot);
    Frame {
        root,
        title: t,
        subtitle: sub,
        body,
        foot,
    }
}

/// A small borderless icon button.
pub fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat-icon");
    b.set_tooltip_text(Some(tip));
    b.update_property(&[gtk::accessible::Property::Label(tip)]);
    b.set_valign(gtk::Align::Center);
    b
}

/// A rounded text button; `filled` for the one thing to do.
pub fn pill(label: &str, filled: bool) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("pill");
    if filled {
        b.add_css_class("filled");
    }
    b.set_valign(gtk::Align::Center);
    b
}

/// The search field at the top of a list.
pub fn search(placeholder: &str) -> gtk::SearchEntry {
    let e = gtk::SearchEntry::new();
    e.add_css_class("panel-search");
    e.set_placeholder_text(Some(placeholder));
    e.update_property(&[gtk::accessible::Property::Label(placeholder)]);
    e
}

/// A day or section caption.
pub fn caption(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("panel-caption");
    l.set_xalign(0.0);
    l
}

/// What an empty list says.
pub fn empty(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("panel-empty");
    l.set_wrap(true);
    l.set_justify(gtk::Justification::Center);
    l.set_halign(gtk::Align::Center);
    l.set_valign(gtk::Align::Start);
    l
}

/// A scrolling column (never sideways).
pub fn scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    s.set_vexpand(true);
    s.set_child(Some(child));
    s
}

/// A list of rows in the panel's style.
pub fn list() -> gtk::ListBox {
    let l = gtk::ListBox::new();
    l.set_selection_mode(gtk::SelectionMode::None);
    l.set_activate_on_single_click(true);
    l
}

/// One row: a mark, a title and a detail line, something at the end.
pub struct Row {
    pub row: gtk::ListBoxRow,
    pub title: gtk::Label,
    pub detail: gtk::Label,
    pub texts: gtk::Box,
    pub end: gtk::Box,
}

pub fn row(mark: &impl IsA<gtk::Widget>, title: &str, detail: &str) -> Row {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("panel-row");
    let h = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let m = mark.as_ref();
    m.set_valign(gtk::Align::Center);
    h.append(m);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    let t = gtk::Label::new(Some(title));
    t.add_css_class("row-title");
    t.set_xalign(0.0);
    t.set_ellipsize(gtk::pango::EllipsizeMode::End);
    t.set_width_chars(1);
    // Natural widths stay small: a long name ellipsizes rather than
    // widening the side panel.
    t.set_max_width_chars(24);
    texts.append(&t);
    let d = gtk::Label::new(Some(detail));
    d.add_css_class("row-detail");
    d.set_xalign(0.0);
    d.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    d.set_width_chars(1);
    d.set_max_width_chars(24);
    d.set_visible(!detail.is_empty());
    texts.append(&d);
    h.append(&texts);
    let end = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    end.set_valign(gtk::Align::Center);
    h.append(&end);
    row.set_child(Some(&h));
    row.update_property(&[gtk::accessible::Property::Label(title)]);
    Row {
        row,
        title: t,
        detail: d,
        texts,
        end,
    }
}

/// Remove every child of a box.
pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// Remove every row of a list.
pub fn clear_list(l: &gtk::ListBox) {
    l.remove_all();
}

/// A muted count in a foot.
pub fn count(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("count");
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_width_chars(1);
    l.set_max_width_chars(16);
    l
}
