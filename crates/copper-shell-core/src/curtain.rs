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

//!
//! The shortcut, the palette rows, the keys while picking and the Hidden
//! panel are the app's (GTK: `copper-app`'s `features::curtain`); this is
//! what is hidden, the sheets on every page and the picker's messages.

use crate::shell::Shell;
use crate::util::{engine_script, fire, site_key};
use crate::{PageOf, Platform, ShellView};
use copper_core::TabId;
use copper_core::crashes::{HiddenFile, Veil};
use copper_core::date::SwiftDate;
use copper_engine::{
    Frames, JsonValue, PageId, PageView, StyleSheetId, UserStyleSheet, WeakPage, World,
};
use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The message handler the picker posts to (`officeVeil`).
pub const HANDLER: &str = "officeVeil";

/// What is hidden changed, or the page in front moved: a view of it (the
/// Hidden panel) refreshes.
pub type Listener<P> = Rc<dyn Fn(&Shell<P>)>;

/// One page's sheets, by site.
struct Dressed<V: PageView> {
    id: PageId,
    page: V::Weak,
    sheets: HashMap<String, StyleSheetId>,
}

struct State<P: Platform> {
    hidden: HiddenFile,
    pages: Vec<Dressed<PageOf<P>>>,
    picking: HashSet<TabId>,
    /// The selector shown while its row is hovered.
    peeking: Option<(String, String)>,
    listener: Option<Listener<P>>,
}

impl<P: Platform> Default for State<P> {
    fn default() -> Self {
        State {
            hidden: HiddenFile::default(),
            pages: Vec::new(),
            picking: HashSet::new(),
            peeking: None,
            listener: None,
        }
    }
}

thread_local! {
    /// The `State<P>` (one platform per process), made on first use.
    static STATE: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
}

fn with<P: Platform, R>(f: impl FnOnce(&mut State<P>) -> R) -> R {
    STATE.with(|s| {
        let mut slot = s.borrow_mut();
        if !slot.as_ref().is_some_and(|a| a.is::<State<P>>()) {
            *slot = Some(Box::new(State::<P>::default()));
        }
        let state = slot
            .as_mut()
            .and_then(|a| a.downcast_mut::<State<P>>())
            .expect("made just above");
        f(state)
    })
}

/// Load `hidden.json`.
pub fn start<P: Platform>(shell: &Shell<P>) {
    match shell.store().load::<HiddenFile>() {
        Ok(l) => with::<P, _>(|s| s.hidden = l.value),
        Err(e) => tracing::warn!(error = %e, "hidden.json"),
    }
}

/// Follow what is hidden on the site in front (the Hidden panel). One
/// listener; a new one replaces the last.
pub fn on_change<P: Platform>(listener: Listener<P>) {
    with::<P, _>(|s| s.listener = Some(listener));
}

/// The site in front, when it has one.
pub fn here<P: Platform>(shell: &Shell<P>) -> Option<String> {
    let id = shell.active_id()?;
    let url = shell.browser().tab(&id).map(|t| t.url.clone())?;
    site_key(&url)
}

/// What is hidden on `site`.
pub fn veils<P: Platform>(site: &str) -> Vec<Veil> {
    with::<P, _>(|s| s.hidden.get(site).cloned().unwrap_or_default())
}

/// The stylesheet that hides `site`'s things (tests, peeking).
pub fn css<P: Platform>(site: &str, spared: Option<&str>) -> String {
    with::<P, _>(|s| copper_shield::curtain::stylesheet(&s.hidden, Some(site), spared))
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
pub fn dress<P: Platform>(_shell: &Shell<P>, _id: &TabId, page: &PageOf<P>) {
    page.add_user_script(&engine_script(&copper_pagejs::curtain::picker()));
    page.add_message_handler(HANDLER, &World::Page);
    let sites: Vec<(String, String)> = with::<P, _>(|s| {
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
    with::<P, _>(|s| {
        s.pages.retain(|d| d.page.upgrade().is_some());
        s.pages.push(Dressed {
            id: page.page_id(),
            page: page.downgrade(),
            sheets,
        });
    });
}

/// The page `gone` went away (closed, slept, crashed and replaced): forget
/// its sheets, and those of any other page already gone.
pub fn page_gone<P: Platform>(gone: PageId) {
    with::<P, _>(|s| {
        s.pages
            .retain(|d| d.id != gone && d.page.upgrade().is_some())
    });
}

/// The tab moved to another document: its picker is gone with the old one.
pub fn navigated<P: Platform>(shell: &Shell<P>, tab: &TabId) {
    with::<P, _>(|s| s.picking.remove(tab));
    if shell.active_id().as_ref() == Some(tab) {
        refresh_panel(shell);
    }
}

/// Put `site`'s sheet on every page again (after a hide, a restore, a
/// peek). `spared` is left showing.
fn restyle<P: Platform>(shell: &Shell<P>, site: &str, spared: Option<&str>) {
    let css = css::<P>(site, spared);
    let pages: Vec<PageOf<P>> = shell
        .pages_map()
        .borrow()
        .values()
        .map(|s| s.page.clone())
        .collect();
    for page in pages {
        let id = page.page_id();
        with::<P, _>(|s| {
            let Some(d) = s
                .pages
                .iter_mut()
                .find(|d| d.id == id && d.page.upgrade().is_some())
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

fn save<P: Platform>(shell: &Shell<P>) {
    let file = with::<P, _>(|s| s.hidden.clone());
    shell.saver().schedule(file);
}

/// Ctrl+Shift+H: start pointing at things, or stop.
pub fn toggle<P: Platform>(shell: &Shell<P>) {
    let Some(id) = shell.active_id() else { return };
    if with::<P, _>(|s| s.picking.contains(&id)) {
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
    with::<P, _>(|s| s.picking.insert(id.clone()));
    fire::<P, _>(&page, copper_pagejs::curtain::start());
    shell.view().hide_panel();
    shell.toast("Click what to hide — Esc when you are done");
}

/// Stop pointing.
pub fn stop<P: Platform>(shell: &Shell<P>, id: &TabId) {
    with::<P, _>(|s| s.picking.remove(id));
    if let Some(slot) = shell.slot(id) {
        fire::<P, _>(&slot.page, copper_pagejs::curtain::stop());
    }
}

/// Whether the tab is picking (the app's keys while picking, tests).
pub fn picking<P: Platform>(id: &TabId) -> bool {
    with::<P, _>(|s| s.picking.contains(id))
}

/// A message from a page's picker.
pub fn message<P: Platform>(shell: &Shell<P>, tab: &TabId, body: &JsonValue) {
    if let Some(reason) = body.get("trouble").and_then(JsonValue::as_str) {
        shell.toast(&format!("Couldn’t hide that — {reason}"));
        return;
    }
    if body.get("off").and_then(JsonValue::as_bool) == Some(true) {
        with::<P, _>(|s| s.picking.remove(tab));
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
pub fn hide<P: Platform>(shell: &Shell<P>, site: &str, selector: &str, label: &str, note: &str) {
    let added = with::<P, _>(|s| {
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
pub fn undo<P: Platform>(shell: &Shell<P>) {
    let Some(site) = here(shell) else { return };
    let back = with::<P, _>(|s| {
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
pub fn restore<P: Platform>(shell: &Shell<P>, site: &str, selector: &str) {
    with::<P, _>(|s| {
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
pub fn restore_all<P: Platform>(shell: &Shell<P>, site: &str) {
    with::<P, _>(|s| {
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
pub fn peek<P: Platform>(shell: &Shell<P>, site: &str, selector: &str) {
    let same = with::<P, _>(|s| {
        let same = s.peeking.as_ref() == Some(&(site.to_string(), selector.to_string()));
        s.peeking = Some((site.to_string(), selector.to_string()));
        same
    });
    if same {
        return;
    }
    restyle(shell, site, Some(selector));
    if let Some(page) = shell.active_page() {
        fire::<P, _>(
            &page,
            copper_pagejs::curtain::peek(&css::<P>(site, Some(selector)), selector),
        );
    }
}

/// Leaving the rows puts the page back.
pub fn unpeek<P: Platform>(shell: &Shell<P>) {
    let Some((site, _)) = with::<P, _>(|s| s.peeking.take()) else {
        return;
    };
    restyle(shell, &site, None);
    unpeek_page(shell, &site);
}

fn unpeek_page<P: Platform>(shell: &Shell<P>, site: &str) {
    if let Some(page) = shell.active_page() {
        fire::<P, _>(&page, copper_pagejs::curtain::unpeek(&css::<P>(site, None)));
    }
}

fn refresh_panel<P: Platform>(shell: &Shell<P>) {
    if let Some(listener) = with::<P, _>(|s| s.listener.clone()) {
        listener(shell);
    }
}
