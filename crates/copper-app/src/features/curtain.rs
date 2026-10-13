//! Hide anything (Mac: `Curtain.swift`, `Hidden.swift`, `Browser.swift`
//! `toggleHiding` / `undoHiding` / `redress`).
//!
//! Ctrl+Shift+H: point at a cookie bar, an overlay, a rail of nonsense —
//! a click takes it off, for good, on this site. What is hidden lives in
//! `hidden.json` (host without `www.` → selectors, the Mac's file) and goes
//! back on as a **user style sheet** for that site, which WebKit applies
//! before the document's first paint — nothing is ever seen appearing and
//! vanishing. Every page carries one sheet per site that has something
//! hidden, limited to that site's addresses (`*://site/*`,
//! `*://www.site/*`), so a page that navigates to a hidden site is dressed
//! before it draws.
//!
//! While picking: Ctrl+Z puts the last one back, Esc (or Ctrl+Shift+H)
//! stops. The Hidden panel lists what is hidden on the site in front;
//! resting on a row shows that one thing, outlined.

use super::ui;
use super::util::{engine_script, fire, site_key};
use crate::actions::{Command, Panel, register_action, register_command, register_panel};
use crate::shell::Shell;
use copper_core::TabId;
use copper_core::crashes::{HiddenFile, Veil};
use copper_core::date::SwiftDate;
use copper_engine::{Frames, JsonValue, PageView, StyleSheetId, UserStyleSheet, World};
use copper_engine_webkitgtk::WebKitPage;
use gtk4 as gtk;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The message handler the picker posts to (`officeVeil`).
pub const HANDLER: &str = "officeVeil";

/// One page's sheets, by site.
struct Dressed {
    view: glib::WeakRef<gtk::Widget>,
    sheets: HashMap<String, StyleSheetId>,
}

#[derive(Default)]
struct State {
    hidden: HiddenFile,
    pages: Vec<Dressed>,
    picking: HashSet<TabId>,
    /// The selector shown while its row is hovered.
    peeking: Option<(String, String)>,
    panel: Option<Rc<HiddenPanel>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// Load `hidden.json`; the shortcut, the palette rows, the panel, the keys
/// that matter while picking.
pub fn start(shell: &Shell) {
    match shell.store().load::<HiddenFile>() {
        Ok(l) => with(|s| s.hidden = l.value),
        Err(e) => tracing::warn!(error = %e, "hidden.json"),
    }
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
        if !with(|s| s.picking.contains(&id)) {
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

/// The site in front, when it has one.
pub fn here(shell: &Shell) -> Option<String> {
    let id = shell.active_id()?;
    let url = shell.browser().tab(&id).map(|t| t.url.clone())?;
    site_key(&url)
}

/// What is hidden on `site`.
pub fn veils(site: &str) -> Vec<Veil> {
    with(|s| s.hidden.get(site).cloned().unwrap_or_default())
}

/// The stylesheet that hides `site`'s things (tests, peeking).
pub fn css(site: &str, spared: Option<&str>) -> String {
    with(|s| copper_shield::curtain::stylesheet(&s.hidden, Some(site), spared))
}

/// A user sheet for `site` holding `css`: main frame, that site only.
fn sheet(site: &str, css: String) -> UserStyleSheet {
    let mut sheet = UserStyleSheet::new(css);
    sheet.frames = Frames::TopOnly;
    sheet.world = World::Page;
    sheet.allow_list = vec![format!("*://{site}/*"), format!("*://www.{site}/*")];
    sheet
}

/// A new page: the picker, its message handler, one sheet per site with
/// something hidden.
pub fn dress(_shell: &Shell, _id: &TabId, page: &WebKitPage) {
    page.add_user_script(&engine_script(&copper_pagejs::curtain::picker()));
    page.add_message_handler(HANDLER, &World::Page);
    let sites: Vec<(String, String)> = with(|s| {
        s.hidden
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(site, _)| {
                (
                    site.clone(),
                    copper_shield::curtain::stylesheet(&s.hidden, Some(site), None),
                )
            })
            .collect()
    });
    let mut sheets = HashMap::new();
    for (site, css) in sites {
        if !css.is_empty() {
            sheets.insert(site.clone(), page.add_user_style_sheet(&sheet(&site, css)));
        }
    }
    with(|s| {
        s.pages.retain(|d| d.view.upgrade().is_some());
        s.pages.push(Dressed {
            view: page.widget().downgrade(),
            sheets,
        });
    });
}

/// A page is gone: forget its sheets.
pub fn page_gone(view: &glib::WeakRef<gtk::Widget>) {
    let gone = view.upgrade();
    with(|s| {
        s.pages.retain(|d| match (d.view.upgrade(), &gone) {
            (None, _) => false,
            (Some(v), Some(g)) => &v != g,
            (Some(_), None) => true,
        })
    });
}

/// The tab moved to another document: its picker is gone with the old one.
pub fn navigated(shell: &Shell, tab: &TabId) {
    with(|s| s.picking.remove(tab));
    if shell.active_id().as_ref() == Some(tab) {
        refresh_panel(shell);
    }
}

/// Put `site`'s sheet on every page again (after a hide, a restore, a
/// peek). `spared` is left showing.
fn restyle(shell: &Shell, site: &str, spared: Option<&str>) {
    let css = css(site, spared);
    let pages: Vec<WebKitPage> = shell
        .pages_map()
        .borrow()
        .values()
        .map(|s| s.page.clone())
        .collect();
    for page in pages {
        let view = page.widget();
        with(|s| {
            let Some(d) = s
                .pages
                .iter_mut()
                .find(|d| d.view.upgrade().as_ref() == Some(&view))
            else {
                return;
            };
            if let Some(old) = d.sheets.remove(site) {
                page.remove_user_style_sheet(old);
            }
            if !css.is_empty() {
                d.sheets.insert(
                    site.to_string(),
                    page.add_user_style_sheet(&sheet(site, css.clone())),
                );
            }
        });
    }
}

fn save(shell: &Shell) {
    let file = with(|s| s.hidden.clone());
    shell.saver().schedule(file);
}

/// Ctrl+Shift+H: start pointing at things, or stop.
pub fn toggle(shell: &Shell) {
    let Some(id) = shell.active_id() else { return };
    if with(|s| s.picking.contains(&id)) {
        stop(shell, &id);
        return;
    }
    if here(shell).is_none() {
        shell.toast("Nothing to hide on this page");
        return;
    }
    let Some(page) = shell.ensure_page(&id) else {
        return;
    };
    with(|s| s.picking.insert(id.clone()));
    fire(&page, copper_pagejs::curtain::start());
    shell.ui().stage.hide_panel();
    shell.toast("Click what to hide — Esc when you are done");
}

/// Stop pointing.
pub fn stop(shell: &Shell, id: &TabId) {
    with(|s| s.picking.remove(id));
    if let Some(slot) = shell.slot(id) {
        fire(&slot.page, copper_pagejs::curtain::stop());
    }
}

/// Whether the tab is picking (tests).
pub fn picking(id: &TabId) -> bool {
    with(|s| s.picking.contains(id))
}

/// A message from a page's picker.
pub fn message(shell: &Shell, tab: &TabId, body: &JsonValue) {
    if let Some(reason) = body.get("trouble").and_then(JsonValue::as_str) {
        shell.toast(&format!("Couldn’t hide that — {reason}"));
        return;
    }
    if body.get("off").and_then(JsonValue::as_bool) == Some(true) {
        with(|s| s.picking.remove(tab));
        return;
    }
    let Some(selector) = body.get("selector").and_then(JsonValue::as_str) else {
        return;
    };
    let label = body
        .get("label")
        .and_then(JsonValue::as_str)
        .unwrap_or(selector);
    let note = body.get("note").and_then(JsonValue::as_str).unwrap_or("");
    let url = shell
        .browser()
        .tab(tab)
        .map(|t| t.url.clone())
        .unwrap_or_default();
    let Some(site) = site_key(&url) else { return };
    hide(shell, &site, selector, label, note);
    shell.toast("Hidden — Ctrl+Z puts it back");
}

/// Hide `selector` on `site`, here and next time.
pub fn hide(shell: &Shell, site: &str, selector: &str, label: &str, note: &str) {
    let added = with(|s| {
        let list = s.hidden.entry(site.to_string()).or_default();
        if list.iter().any(|v| v.selector == selector) {
            return false;
        }
        list.push(Veil {
            selector: selector.to_string(),
            label: label.to_string(),
            note: Some(note.to_string()).filter(|n| !n.is_empty()),
            date: SwiftDate::now(),
            extra: Default::default(),
        });
        true
    });
    if added {
        save(shell);
        restyle(shell, site, None);
        refresh_panel(shell);
    }
}

/// Ctrl+Z while picking: the last thing taken off comes back.
pub fn undo(shell: &Shell) {
    let Some(site) = here(shell) else { return };
    let back = with(|s| {
        let list = s.hidden.get_mut(&site)?;
        let last = list.pop();
        if list.is_empty() {
            s.hidden.remove(&site);
        }
        last
    });
    let Some(back) = back else { return };
    save(shell);
    restyle(shell, &site, None);
    refresh_panel(shell);
    shell.toast(&format!("{} is back", back.label));
}

/// Put one back.
pub fn restore(shell: &Shell, site: &str, selector: &str) {
    with(|s| {
        if let Some(list) = s.hidden.get_mut(site) {
            list.retain(|v| v.selector != selector);
            if list.is_empty() {
                s.hidden.remove(site);
            }
        }
        if s.peeking.as_ref().is_some_and(|(_, sel)| sel == selector) {
            s.peeking = None;
        }
    });
    save(shell);
    restyle(shell, site, None);
    unpeek_page(shell, site);
    refresh_panel(shell);
}

/// Put everything on `site` back.
pub fn restore_all(shell: &Shell, site: &str) {
    with(|s| {
        s.hidden.remove(site);
        s.peeking = None;
    });
    save(shell);
    restyle(shell, site, None);
    unpeek_page(shell, site);
    refresh_panel(shell);
    shell.toast("Everything is back");
}

/// Resting on a row: that one thing shows, outlined, scrolled to.
fn peek(shell: &Shell, site: &str, selector: &str) {
    let same = with(|s| {
        let same = s.peeking.as_ref() == Some(&(site.to_string(), selector.to_string()));
        s.peeking = Some((site.to_string(), selector.to_string()));
        same
    });
    if same {
        return;
    }
    restyle(shell, site, Some(selector));
    if let Some(page) = shell.active_page() {
        fire(
            &page,
            copper_pagejs::curtain::peek(&css(site, Some(selector)), selector),
        );
    }
}

/// Leaving the rows puts the page back.
fn unpeek(shell: &Shell) {
    let Some((site, _)) = with(|s| s.peeking.take()) else {
        return;
    };
    restyle(shell, &site, None);
    unpeek_page(shell, &site);
}

fn unpeek_page(shell: &Shell, site: &str) {
    if let Some(page) = shell.active_page() {
        fire(&page, copper_pagejs::curtain::unpeek(&css(site, None)));
    }
}

fn refresh_panel(shell: &Shell) {
    if let Some(p) = with(|s| s.panel.clone()) {
        p.refresh(shell);
    }
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
        with(|s| s.panel = Some(panel.clone()));
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
        let veils = site.as_deref().map(veils).unwrap_or_default();
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
