//! Trusted input for `PageView::input` (T-002 / D-006).
//!
//! GTK4 cannot build a `GdkEvent`, and WebKitGTK has no input-synthesis API,
//! but WebKitWebViewBase's own GTK event controllers do the work when their
//! signals are emitted directly:
//!
//! - `GtkEventControllerMotion::enter(x, y)` makes a trusted mouse move;
//! - the touch-only `GtkGestureClick::released(n, x, y)` synthesizes a
//!   trusted move + press + release (`detail` n, `pointerType` "mouse",
//!   click-to-focus, `dblclick` at n = 2) with no GDK event behind it;
//! - `GtkEventControllerFocus::enter`/`leave` give a page focus without GTK
//!   focus (`document.hasFocus()`), used by the backstage on request.
//!
//! Text goes through WebKit editing commands (`InsertText`, `InsertNewline`,
//! `DeleteBackward`…), which fire trusted `beforeinput`/`input` and, for
//! Enter in a form field, a trusted `submit`. Key events themselves are
//! dispatched from script around the command (no API makes them trusted).
//! Everything else goes through `input.js` (untrusted).
//!
//! The controllers are WebKit internals, not API, so each page looks them up
//! when it is created, after a once-per-process check of the signal
//! signatures; anything missing makes that kind of input fall back to script
//! (logged once).

use std::cell::{Cell, OnceCell, RefCell};
use std::time::Duration;

use copper_engine::{
    EngineError, InputEvent, JsonMap, JsonValue, Modifiers, MouseButton, PageView, Result, Trust,
    World,
};
use glib::prelude::*;
use gtk4::prelude::*;
use serde_json::json;
use webkit6::prelude::*;
use webkit6::{OptionMenu, WebView};

use crate::page::WebKitPage;

/// Async function body behind `PageView::input` (script delivery + helpers).
pub(crate) const INPUT_JS: &str = include_str!("input.js");
/// Isolated world the input script runs in.
pub(crate) const INPUT_WORLD: &str = "copper-input";

/// How long to wait for the last trusted event of a click / a move.
const CLICK_SETTLE_MS: u64 = 1000;
const MOVE_SETTLE_MS: u64 = 300;

/// What `PageView::input` delivers through WebKit itself on one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustedInput {
    /// Pointer moves and left clicks go through WebKit's event controllers.
    pub pointer: bool,
    /// Typing and editing keys go through WebKit editing commands.
    pub editing: bool,
    /// The page can be given focus without GTK focus (backstage page focus).
    pub page_focus: bool,
}

impl TrustedInput {
    pub(crate) const NONE: TrustedInput = TrustedInput {
        pointer: false,
        editing: false,
        page_focus: false,
    };
}

/// WebKit's own GTK event controllers on one view.
#[derive(Default)]
pub(crate) struct Hands {
    motion: Option<gtk4::EventControllerMotion>,
    tap: Option<gtk4::GestureClick>,
    focus: Option<gtk4::EventControllerFocus>,
}

impl Hands {
    /// Look up the controllers WebKitWebViewBase installs at construction.
    pub(crate) fn find(view: &WebView) -> Hands {
        // `observe_controllers` turns on bookkeeping in GTK while the list
        // lives; it is dropped at the end of this function.
        let list = view.observe_controllers();
        let mut hands = Hands::default();
        for i in 0..list.n_items() {
            let Some(obj) = list.item(i) else { continue };
            if let Ok(m) = obj.clone().downcast::<gtk4::EventControllerMotion>() {
                hands.motion.get_or_insert(m);
            } else if let Ok(f) = obj.clone().downcast::<gtk4::EventControllerFocus>() {
                hands.focus.get_or_insert(f);
            } else if let Ok(g) = obj.downcast::<gtk4::GestureClick>()
                && g.is_touch_only()
            {
                hands.tap.get_or_insert(g);
            }
        }
        hands
    }

    /// What this page can deliver trusted, given the signal self-check.
    pub(crate) fn available(&self) -> TrustedInput {
        let sig = signatures();
        let t = TrustedInput {
            pointer: sig.pointer && self.motion.is_some() && self.tap.is_some(),
            editing: true,
            page_focus: sig.focus && self.focus.is_some(),
        };
        let mut missing = Vec::new();
        if self.motion.is_none() {
            missing.push("GtkEventControllerMotion");
        }
        if self.tap.is_none() {
            missing.push("touch-only GtkGestureClick");
        }
        if self.focus.is_none() {
            missing.push("GtkEventControllerFocus");
        }
        if !missing.is_empty() {
            warn_once(&format!(
                "WebKit web view has no {}; that input falls back to script (isTrusted false)",
                missing.join(", ")
            ));
        }
        t
    }

    fn move_to(&self, x: f64, y: f64) {
        if let Some(m) = &self.motion {
            m.emit_by_name::<()>("enter", &[&x, &y]);
        }
    }

    fn click(&self, count: u32, x: f64, y: f64) {
        if let Some(g) = &self.tap {
            for n in 1..=count as i32 {
                g.emit_by_name::<()>("released", &[&n, &x, &y]);
            }
        }
    }

    /// Page focus on (`enter`) or off (`leave`), GTK focus untouched.
    pub(crate) fn page_focus(&self, on: bool) {
        if signatures().focus
            && let Some(f) = &self.focus
        {
            f.emit_by_name::<()>(if on { "enter" } else { "leave" }, &[]);
        }
    }
}

/// The start-up self-check of the controller signals `Hands` emits: the
/// emission must match their signatures exactly (glib panics otherwise).
#[derive(Clone, Debug)]
struct Signatures {
    pointer: bool,
    focus: bool,
}

/// Run the once-per-process signal check now (the engine does this at
/// start-up, so a broken WebKit/GTK pairing is logged before any input).
pub(crate) fn self_check() {
    let _ = signatures();
}

fn signatures() -> Signatures {
    thread_local! {
        static SIG: OnceCell<Signatures> = const { OnceCell::new() };
    }
    SIG.with(|c| {
        c.get_or_init(|| {
            let f64t = f64::static_type();
            let enter = signal_is::<gtk4::EventControllerMotion>("enter", &[f64t, f64t]);
            let released = signal_is::<gtk4::GestureClick>(
                "released",
                &[i32::static_type(), f64t, f64t],
            );
            let focus = signal_is::<gtk4::EventControllerFocus>("enter", &[])
                && signal_is::<gtk4::EventControllerFocus>("leave", &[]);
            let sig = Signatures {
                pointer: enter && released,
                focus,
            };
            if !sig.pointer {
                warn_once("GTK event-controller signals changed shape; pointer input falls back to script (isTrusted false)");
            }
            sig
        })
        .clone()
    })
}

fn signal_is<T: StaticType>(name: &str, params: &[glib::Type]) -> bool {
    // Signals exist once the class is initialized; hold a class reference.
    let Some(_class) = glib::Class::<glib::Object>::from_type(T::static_type()) else {
        return false;
    };
    let Some(id) = glib::subclass::signal::SignalId::lookup(name, T::static_type()) else {
        return false;
    };
    let q = id.query();
    q.return_type().type_() == glib::Type::UNIT
        && q.param_types().len() == params.len()
        && q.param_types()
            .iter()
            .zip(params)
            .all(|(p, want)| p.type_() == *want)
}

fn warn_once(msg: &str) {
    thread_local! {
        static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    SEEN.with(|s| {
        if !s.borrow().iter().any(|m| m == msg) {
            tracing::warn!("copper input self-check: {msg}");
            s.borrow_mut().push(msg.to_owned());
        }
    });
}

// ---------------------------------------------------------------------------
// Delivery

pub(crate) async fn deliver(page: WebKitPage, ev: InputEvent) -> Result<Trust> {
    // Escape closes a file chooser the page has open, as it would for a
    // person looking at the dialog; the page hears `cancel`.
    if let InputEvent::Key { key, .. } = &ev
        && key == "Escape"
        && page.shared().cancel_file_chooser()
    {
        return Ok(Trust::Trusted);
    }
    // A `<select>` popup the backstage kept off screen takes keys the way a
    // real popup would; anything else dismisses it first.
    if let Some(menu) = page.shared().open_menu() {
        match &ev {
            InputEvent::Key { key, text, .. } => {
                menu.key(key, text.as_deref());
                return Ok(Trust::Trusted);
            }
            InputEvent::Type(text) => {
                menu.type_text(text);
                return Ok(Trust::Trusted);
            }
            _ => menu.cancel(),
        }
    }
    let mode = page.trusted_input();
    match ev {
        InputEvent::Move { x, y, modifiers } if modifiers == Modifiers::NONE && mode.pointer => {
            pointer(&page, x, y, None).await
        }
        InputEvent::Click {
            x,
            y,
            button: MouseButton::Left,
            count,
            modifiers,
        } if modifiers == Modifiers::NONE && mode.pointer => {
            pointer(&page, x, y, Some(count.clamp(1, 3))).await
        }
        InputEvent::Type(text) if mode.editing => type_text(&page, &text).await,
        InputEvent::Key {
            key,
            text,
            modifiers,
        } if mode.editing => {
            let key = Stroke {
                key,
                text,
                modifiers,
            };
            let trust = stroke(&page, &key, None).await?;
            op(&page, json!({"keys": {"up": key.json(), "down": null}})).await?;
            Ok(trust.unwrap_or(Trust::Script))
        }
        other => {
            let ev = serde_json::to_value(&other)
                .map_err(|e| EngineError::InvalidArgument(e.to_string()))?;
            op(&page, ev).await?;
            Ok(Trust::Script)
        }
    }
}

/// Run one `input.js` operation in the input world.
async fn op(page: &WebKitPage, ev: JsonValue) -> Result<JsonValue> {
    let mut args = JsonMap::new();
    args.insert("ev".into(), ev);
    let out = page
        .call_async(INPUT_JS, &args, &World::isolated(INPUT_WORLD))
        .await?;
    if out.get("ok").and_then(JsonValue::as_bool) == Some(true) {
        Ok(out)
    } else {
        let why = out
            .get("error")
            .and_then(JsonValue::as_str)
            .unwrap_or("input was not delivered");
        Err(EngineError::NotFound(why.to_owned()))
    }
}

/// A move (`click == None`) or a left click with `click` presses, emitted on
/// the view's own controllers.
async fn pointer(page: &WebKitPage, x: f64, y: f64, click: Option<u32>) -> Result<Trust> {
    let (until, count, settle_ms) = match click {
        Some(n) => ("click", n, CLICK_SETTLE_MS),
        None => ("mousemove", 1, MOVE_SETTLE_MS),
    };
    let armed = op(
        page,
        json!({"arm": {"x": x, "y": y, "until": until, "count": count}}),
    )
    .await?;
    let (wx, wy) = widget_point(page.view(), x, y);
    let hands = &page.shared().hands;
    match click {
        Some(n) => hands.click(n, wx, wy),
        None => hands.move_to(wx, wy),
    }
    if armed.get("watch").and_then(JsonValue::as_bool) == Some(true) {
        // WebKit queues mouse events in the UI process until the web process
        // acknowledges the previous one, so a later script call can overtake
        // them; wait for the action's last event instead. A page that is
        // stuck (a dialog, a long task) is not waited for past a second more.
        let settle = op(page, json!({"settle": {"ms": settle_ms}}));
        match glib::future_with_timeout(Duration::from_millis(settle_ms + 1000), settle).await {
            Ok(Ok(v)) if v.get("seen").and_then(JsonValue::as_bool) == Some(true) => {}
            Ok(Ok(_)) => {
                tracing::debug!("trusted {until} at {x},{y}: no event within {settle_ms} ms")
            }
            // The click navigated away or the page is busy: it was delivered.
            Ok(Err(e)) => tracing::debug!("trusted {until} at {x},{y}: settle: {e}"),
            Err(_) => tracing::debug!("trusted {until} at {x},{y}: page did not answer"),
        }
    }
    Ok(Trust::Trusted)
}

/// CSS px (viewport) to widget px: WebKit's page zoom scales the page.
fn widget_point(view: &WebView, x: f64, y: f64) -> (f64, f64) {
    let zoom = match WebViewExt::settings(view) {
        Some(s) if s.is_zoom_text_only() => 1.0,
        _ => view.zoom_level(),
    };
    (x * zoom, y * zoom)
}

/// One key press as `input.js` sees it.
struct Stroke {
    key: String,
    text: Option<String>,
    modifiers: Modifiers,
}

impl Stroke {
    fn json(&self) -> JsonValue {
        json!({"key": self.key, "text": self.text, "modifiers": self.modifiers})
    }

    fn named(key: &str) -> Stroke {
        Stroke {
            key: key.to_owned(),
            text: None,
            modifiers: Modifiers::NONE,
        }
    }
}

async fn type_text(page: &WebKitPage, text: &str) -> Result<Trust> {
    let mut trust: Option<Trust> = None;
    let mut up: Option<JsonValue> = None;
    for ch in text.chars() {
        let key = match ch {
            '\r' => continue,
            '\n' => Stroke::named("Enter"),
            '\t' => Stroke::named("Tab"),
            c => Stroke {
                key: c.to_string(),
                text: Some(c.to_string()),
                modifiers: Modifiers::NONE,
            },
        };
        if let Some(t) = stroke(page, &key, up.take()).await? {
            trust = Some(trust.map_or(t, |a| a.and(t)));
        }
        up = Some(key.json());
    }
    if let Some(u) = up {
        op(page, json!({"keys": {"up": u, "down": null}})).await?;
    }
    Ok(trust.unwrap_or(Trust::Script))
}

/// keyup for the previous key (if any) and keydown for `key` from script,
/// then, unless the page cancelled the keydown, the key's effect: a WebKit
/// editing command (trusted) or its default action from script. `None` when
/// the page cancelled it.
async fn stroke(page: &WebKitPage, key: &Stroke, up: Option<JsonValue>) -> Result<Option<Trust>> {
    let r = op(page, json!({"keys": {"up": up, "down": key.json()}})).await?;
    if r.get("proceed").and_then(JsonValue::as_bool) != Some(true) {
        return Ok(None);
    }
    let editable = r.get("editable").and_then(JsonValue::as_bool) == Some(true);
    match editing_command(&key.key, key.text.as_deref(), key.modifiers, editable) {
        Some((command, Some(arg))) => {
            page.view()
                .execute_editing_command_with_argument(command, &arg);
            Ok(Some(Trust::Trusted))
        }
        Some((command, None)) => {
            page.view().execute_editing_command(command);
            Ok(Some(Trust::Trusted))
        }
        None => {
            op(page, json!({"fallback": key.json()})).await?;
            Ok(Some(Trust::Script))
        }
    }
}

/// The WebKit editing command a key press maps to (names from WebKit's
/// editor command table), with its argument.
pub(crate) fn editing_command(
    key: &str,
    text: Option<&str>,
    m: Modifiers,
    editable: bool,
) -> Option<(&'static str, Option<String>)> {
    let pick = |plain: &'static str, extend: &'static str| if m.shift { extend } else { plain };
    if m.alt {
        return None;
    }
    if m.ctrl || m.meta {
        let command = match key.to_ascii_lowercase().as_str() {
            "a" if !m.shift => "SelectAll",
            "z" => pick("Undo", "Redo"),
            "y" if !m.shift => "Redo",
            "backspace" if editable => "DeleteWordBackward",
            "delete" if editable => "DeleteWordForward",
            "arrowleft" if editable => pick("MoveWordLeft", "MoveWordLeftAndModifySelection"),
            "arrowright" if editable => pick("MoveWordRight", "MoveWordRightAndModifySelection"),
            "home" if editable => pick(
                "MoveToBeginningOfDocument",
                "MoveToBeginningOfDocumentAndModifySelection",
            ),
            "end" if editable => pick(
                "MoveToEndOfDocument",
                "MoveToEndOfDocumentAndModifySelection",
            ),
            _ => return None,
        };
        return Some((command, None));
    }
    let command = match key {
        "Enter" if editable => "InsertNewline",
        "Backspace" if editable => "DeleteBackward",
        "Delete" if editable => "DeleteForward",
        "ArrowLeft" if editable => pick("MoveLeft", "MoveLeftAndModifySelection"),
        "ArrowRight" if editable => pick("MoveRight", "MoveRightAndModifySelection"),
        "ArrowUp" if editable => pick("MoveUp", "MoveUpAndModifySelection"),
        "ArrowDown" if editable => pick("MoveDown", "MoveDownAndModifySelection"),
        "Home" if editable => pick(
            "MoveToBeginningOfLine",
            "MoveToBeginningOfLineAndModifySelection",
        ),
        "End" if editable => pick("MoveToEndOfLine", "MoveToEndOfLineAndModifySelection"),
        "Enter" | "Backspace" | "Delete" | "Tab" | "Escape" => return None,
        _ => {
            let t = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))?;
            return Some(("InsertText", Some(t.to_owned())));
        }
    };
    Some((command, None))
}

// ---------------------------------------------------------------------------
// <select> popups kept off screen

/// A `<select>` popup a held page opened, kept instead of shown. Keys move
/// its highlight, Enter/Space/Tab pick, Escape (or any pointer action)
/// dismisses it, printable text jumps to a matching option.
pub(crate) struct OpenMenu {
    menu: OptionMenu,
    original: Option<u32>,
    current: Cell<Option<u32>>,
    typed: RefCell<String>,
}

impl OpenMenu {
    pub(crate) fn new(menu: &OptionMenu) -> OpenMenu {
        let original =
            (0..menu.n_items()).find(|&i| menu.item(i).is_some_and(|mut it| it.is_selected()));
        OpenMenu {
            menu: menu.clone(),
            original,
            current: Cell::new(original),
            typed: RefCell::default(),
        }
    }

    pub(crate) fn is(&self, menu: &OptionMenu) -> bool {
        self.menu == *menu
    }

    fn usable(&self, i: u32) -> bool {
        self.menu
            .item(i)
            .is_some_and(|mut it| it.is_enabled() && !it.is_group_label())
    }

    fn label(&self, i: u32) -> String {
        self.menu
            .item(i)
            .and_then(|mut it| it.label())
            .map(|l| l.trim().to_lowercase())
            .unwrap_or_default()
    }

    fn select(&self, i: u32) {
        self.menu.select_item(i);
        self.current.set(Some(i));
    }

    /// Move the highlight `steps` usable items forward (or back), stopping
    /// at the ends like a real popup.
    fn step(&self, forward: bool, steps: u32) {
        let n = self.menu.n_items();
        let mut at = self.current.get();
        for _ in 0..steps {
            let mut next = at;
            let mut i = at;
            loop {
                i = match (i, forward) {
                    (None, true) => Some(0),
                    (None, false) => n.checked_sub(1),
                    (Some(i), true) if i + 1 < n => Some(i + 1),
                    (Some(i), false) if i > 0 => Some(i - 1),
                    _ => None,
                };
                match i {
                    Some(c) if self.usable(c) => {
                        next = Some(c);
                        break;
                    }
                    Some(_) => continue,
                    None => break,
                }
            }
            if next == at {
                break;
            }
            at = next;
        }
        if let Some(i) = at
            && at != self.current.get()
        {
            self.select(i);
        }
    }

    fn edge(&self, last: bool) {
        let n = self.menu.n_items();
        let found = if last {
            (0..n).rev().find(|&i| self.usable(i))
        } else {
            (0..n).find(|&i| self.usable(i))
        };
        if let Some(i) = found {
            self.select(i);
        }
    }

    fn type_ahead(&self, ch: char) {
        let mut typed = self.typed.borrow_mut();
        typed.extend(ch.to_lowercase());
        let n = self.menu.n_items();
        let start = self.current.get().map_or(0, |c| c + 1);
        let order = (0..n).map(|k| (start + k) % n.max(1));
        let hit = order
            .clone()
            .find(|&i| self.usable(i) && self.label(i).starts_with(typed.as_str()))
            .or_else(|| {
                // A fresh letter starts a new search.
                let one: String = ch.to_lowercase().collect();
                *typed = one.clone();
                order
                    .clone()
                    .find(|&i| self.usable(i) && self.label(i).starts_with(one.as_str()))
            });
        drop(typed);
        if let Some(i) = hit {
            self.select(i);
        }
    }

    /// Pick the highlighted option and close.
    pub(crate) fn accept(&self) {
        if let Some(i) = self.current.get() {
            self.menu.activate_item(i);
        }
        self.menu.close();
    }

    /// Close without changing the value.
    pub(crate) fn cancel(&self) {
        if let Some(i) = self.original {
            self.menu.select_item(i);
        }
        self.menu.close();
    }

    pub(crate) fn key(&self, key: &str, text: Option<&str>) {
        match key {
            "ArrowDown" => self.step(true, 1),
            "ArrowUp" => self.step(false, 1),
            "PageDown" => self.step(true, 10),
            "PageUp" => self.step(false, 10),
            "Home" => self.edge(false),
            "End" => self.edge(true),
            "Enter" | " " | "Spacebar" | "Tab" => self.accept(),
            "Escape" => self.cancel(),
            _ => {
                if let Some(t) = text {
                    for ch in t.chars().filter(|c| !c.is_control()) {
                        self.type_ahead(ch);
                    }
                }
            }
        }
    }

    pub(crate) fn type_text(&self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\n' | '\t' => {
                    self.accept();
                    return;
                }
                c if !c.is_control() => self.type_ahead(c),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    };

    #[test]
    fn editing_commands() {
        let none = Modifiers::NONE;
        assert_eq!(
            editing_command("a", Some("a"), none, false),
            Some(("InsertText", Some("a".into())))
        );
        assert_eq!(
            editing_command("Enter", None, none, true),
            Some(("InsertNewline", None))
        );
        assert_eq!(editing_command("Enter", Some("\r"), none, false), None);
        assert_eq!(
            editing_command("Backspace", None, none, true),
            Some(("DeleteBackward", None))
        );
        assert_eq!(editing_command("Backspace", None, none, false), None);
        assert_eq!(editing_command("Tab", Some("\t"), none, true), None);
        assert_eq!(
            editing_command("a", Some("a"), CTRL, false),
            Some(("SelectAll", None))
        );
        assert_eq!(
            editing_command(
                "Z",
                None,
                Modifiers {
                    shift: true,
                    ..CTRL
                },
                true
            ),
            Some(("Redo", None))
        );
        assert_eq!(editing_command("c", Some("c"), CTRL, true), None);
        assert_eq!(
            editing_command(
                "Home",
                None,
                Modifiers {
                    shift: true,
                    ..none
                },
                true
            ),
            Some(("MoveToBeginningOfLineAndModifySelection", None))
        );
        let alt = Modifiers { alt: true, ..none };
        assert_eq!(editing_command("a", Some("a"), alt, true), None);
    }

    #[test]
    fn trust_combines() {
        assert_eq!(Trust::Trusted.and(Trust::Trusted), Trust::Trusted);
        assert_eq!(Trust::Trusted.and(Trust::Script), Trust::Mixed);
        assert_eq!(Trust::Script.and(Trust::Script), Trust::Script);
    }
}
