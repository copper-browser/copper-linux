//! Bookmarks (Ctrl+Shift+O; Mac: `Bookmarks.swift` `BookmarksPanel`): the
//! whole tree, folders opening in place, searchable by title or address;
//! a line can go. Ctrl+D keeps the page in front (Mac ⇧⌘B,
//! `bookmarkCurrent`).

use super::{Refresh, close, escape_closes, go, site_mark};
use crate::features::ui;
use crate::shell::Shell;
use copper_core::bookmarks::Bookmark;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

/// The page in front, at the end of the list.
pub fn bookmark_current(shell: &Shell) {
    let Some(id) = shell.active_id() else { return };
    let (url, title) = match shell.browser().tab(&id) {
        Some(t) => (t.url.clone(), t.title.clone()),
        None => return,
    };
    if url.is_empty() || url.starts_with("about:") {
        return;
    }
    if shell.bookmarks().contains_url(&url) {
        shell.toast("Already a bookmark");
        return;
    }
    shell.bookmarks_mut().add(&url, &title);
    save(shell);
    shell.toast("Bookmarked");
    super::refresh(shell, "bookmarks");
}

/// Write `bookmarks.json` (debounced).
pub fn save(shell: &Shell) {
    let roots = shell.bookmarks().roots().to_vec();
    shell.saver().schedule(roots);
}

struct BookmarksPanel {
    frame: ui::Frame,
    search: gtk::SearchEntry,
    content: gtk::Box,
    count: gtk::Label,
    open: RefCell<HashSet<String>>,
}

pub fn build(shell: &Shell) -> (gtk::Widget, Refresh) {
    let weak = shell.downgrade();
    let frame = ui::frame("Bookmarks", move || {
        if let Some(s) = weak.upgrade() {
            close(&s);
        }
    });
    let search = ui::search("Search bookmarks");
    frame.body.append(&search);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.body.append(&ui::scroller(&content));
    let count = ui::count("");
    count.set_hexpand(true);
    frame.foot.append(&count);
    let add = ui::pill("Add this page", false);
    let weak = shell.downgrade();
    add.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            bookmark_current(&s);
        }
    });
    frame.foot.append(&add);
    escape_closes(shell, &frame.root);

    let panel = Rc::new(BookmarksPanel {
        frame,
        search,
        content,
        count,
        open: RefCell::default(),
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
        if let Some(s) = weak.upgrade()
            && let Some(hit) = s.bookmarks().search(&p.search.text()).first().cloned()
            && let Some(url) = hit.bookmark.url
        {
            close(&s);
            go(&s, &url);
        }
    });
    let root = panel.frame.root.clone().upcast();
    let p = panel.clone();
    let refresh: Refresh = Rc::new(move |s: &Shell| {
        p.draw(s);
        p.search.grab_focus();
    });
    (root, refresh)
}

impl BookmarksPanel {
    fn draw(self: &Rc<Self>, shell: &Shell) {
        ui::clear(&self.content);
        let total = shell.bookmarks().count();
        self.count.set_text(&match total {
            1 => "1 bookmark".to_string(),
            n => format!("{n} bookmarks"),
        });
        let typed = self.search.text().trim().to_string();
        let list = ui::list();
        let weak = shell.downgrade();
        let p = self.clone();
        list.connect_row_activated(move |_, row| {
            let Some(s) = weak.upgrade() else { return };
            let name = row.widget_name().to_string();
            if let Some(folder) = name.strip_prefix("folder:") {
                let folder = folder.to_string();
                let mut open = p.open.borrow_mut();
                if !open.remove(&folder) {
                    open.insert(folder);
                }
                drop(open);
                p.draw(&s);
            } else if let Some(url) = name.strip_prefix("site:") {
                close(&s);
                go(&s, url);
            }
        });
        if typed.is_empty() {
            if total == 0 {
                self.content.append(&ui::empty(
                    "Nothing kept yet. Add the page in front with Ctrl+D.",
                ));
                return;
            }
            let roots = shell.bookmarks().roots().to_vec();
            self.outline(shell, &list, &roots, 0);
        } else {
            let hits = shell.bookmarks().search(&typed);
            if hits.is_empty() {
                self.content.append(&ui::empty("Nothing matches."));
                return;
            }
            for hit in hits {
                let where_ = if hit.path.is_empty() {
                    hit.bookmark.url.clone().unwrap_or_default()
                } else {
                    hit.path.join(" › ")
                };
                list.append(&self.site(shell, &hit.bookmark, &where_, 0));
            }
        }
        self.content.append(&list);
    }

    fn outline(
        self: &Rc<Self>,
        shell: &Shell,
        list: &gtk::ListBox,
        nodes: &[Bookmark],
        depth: i32,
    ) {
        for n in nodes {
            if n.is_folder() {
                let open = self.open.borrow().contains(&n.id);
                let icon = gtk::Image::from_icon_name(if open {
                    "folder-open-symbolic"
                } else {
                    "folder-symbolic"
                });
                icon.set_pixel_size(16);
                icon.add_css_class("file-mark");
                let kids = Bookmark::count_of(n.kids());
                let r = ui::row(&icon, &n.title, "");
                r.title.remove_css_class("row-title");
                r.title.add_css_class("row-folder");
                r.row.set_widget_name(&format!("folder:{}", n.id));
                r.row.set_margin_start(6 + depth * 18);
                let c = gtk::Label::new(Some(&kids.to_string()));
                c.add_css_class("row-side");
                r.end.append(&c);
                r.row
                    .update_property(&[gtk::accessible::Property::Label(&format!(
                        "{}, folder, {} {}",
                        n.title,
                        kids,
                        if open { "open" } else { "closed" }
                    ))]);
                list.append(&r.row);
                if open {
                    if n.kids().is_empty() {
                        let r =
                            ui::row(&gtk::Box::new(gtk::Orientation::Horizontal, 0), "Empty", "");
                        r.row.set_activatable(false);
                        r.row.set_margin_start(6 + (depth + 1) * 18);
                        r.title.set_opacity(0.5);
                        list.append(&r.row);
                    }
                    let kids = n.kids().to_vec();
                    self.outline(shell, list, &kids, depth + 1);
                }
            } else {
                let url = n.url.clone().unwrap_or_default();
                let detail = copper_core::address::pretty_str(&url);
                list.append(&self.site(shell, n, &detail, depth));
            }
        }
    }

    fn site(
        self: &Rc<Self>,
        shell: &Shell,
        b: &Bookmark,
        detail: &str,
        depth: i32,
    ) -> gtk::ListBoxRow {
        let url = b.url.clone().unwrap_or_default();
        let mark = site_mark(shell, &url, &crate::shell::host_of(&url));
        let r = ui::row(&mark, &b.title, detail);
        r.row.set_widget_name(&format!("site:{url}"));
        r.row.set_tooltip_text(Some(&url));
        r.row.set_margin_start(6 + depth * 18);
        let forget = ui::icon_button("window-close-symbolic", "Remove bookmark");
        let weak = shell.downgrade();
        let id = b.id.clone();
        let p = self.clone();
        forget.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                if s.bookmarks_mut().remove(&id) {
                    save(&s);
                }
                p.draw(&s);
            }
        });
        r.end.append(&forget);
        r.row
    }
}

/// Sites under some nodes (folders counted by what they hold).
trait CountOf {
    fn count_of(nodes: &[Bookmark]) -> usize;
}

impl CountOf for Bookmark {
    fn count_of(nodes: &[Bookmark]) -> usize {
        nodes
            .iter()
            .map(|n| {
                if n.is_folder() {
                    Self::count_of(n.kids())
                } else {
                    1
                }
            })
            .sum()
    }
}
