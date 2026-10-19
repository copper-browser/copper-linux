//! The addon's web view (Mirrors the Mac's `Fork/Addons/AddonHost.swift`:
//! its own `WKWebsiteDataStore`, the `copper-addon:` scheme handler, the
//! bridge relay and the navigation delegate).
//!
//! * **Profile.** Its own persistent profile, `addons` — never the browsing
//!   one, so no cookies or site data are shared (CONTRACT §5.4).
//! * **Scripts.** At document start, top frame only: a page-world line that
//!   defines `window.__copperAddonTransport`, then the core's `BRIDGE_JS`
//!   (bridge v1). The transport posts a DOM event that an isolated world
//!   forwards to the `copperAddon` message handler, which exists in that
//!   isolated world only.
//! * **Messages.** Taken only from this view, while it shows
//!   `copper-addon://<the slot's addon>/`; dispatched by the core on the
//!   cloud's tokio runtime; the reply is evaluated back on the main thread.
//! * **Navigation.** Only `copper-addon://<id>/…`; an `http(s)` link the
//!   person activates opens in a new foreground tab; new windows are refused.
//! * **Inspector.** Only in debug builds or with `COPPER_ADDONS_DEV_DIR`.
//! * **Events.** `visibility`, `theme`, `settings`, and `page` (with the
//!   `page` capability).

use super::core_glue::{self, Active};
use super::policy::{self, Verdict};
use super::{host, scheme};
use crate::shell::{OpenTab, Shell};
use copper_engine::{
    Engine, Frames, InjectionTime, PageEvent, PageOptions, PageView, ProfileId, UserScript, World,
};
use copper_engine_webkitgtk::WebKitPage;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// The message handler's name (`webkit.messageHandlers.copperAddon`).
pub const HANDLER: &str = "copperAddon";
/// The isolated world the handler lives in.
pub const WORLD: &str = "CopperAddonHost";
/// The profile the slot's pages use.
pub const PROFILE: &str = "addons";

/// Page world: the transport `bridge.js` prefers. Defined before the shim,
/// fixed (not writable), and only on addon pages.
const PAGE_TRANSPORT: &str = r#"(() => {
  if (location.protocol !== 'copper-addon:') return;
  const post = (text) => document.dispatchEvent(new CustomEvent('copper-addon-post', { detail: String(text) }));
  try { Object.defineProperty(window, '__copperAddonTransport', { value: post, writable: false, configurable: false }); } catch (_) {}
})();"#;

/// The isolated world: forward what the page posted to the native handler.
const HOST_RELAY: &str = r#"(() => {
  if (location.protocol !== 'copper-addon:') return;
  document.addEventListener('copper-addon-post', (e) => {
    if (location.protocol !== 'copper-addon:') return;
    const d = e.detail;
    if (typeof d === 'string' && d.length <= 4 * 1024 * 1024) window.webkit.messageHandlers.copperAddon.postMessage(d);
  }, true);
})();"#;

thread_local! {
    /// Every live addon view (one per window's slot; one window today).
    static VIEWS: RefCell<Vec<Weak<AddonView>>> = const { RefCell::new(Vec::new()) };
    /// The `page` event last sent, so a tab's every title flicker is one event.
    static LAST_PAGE: RefCell<Value> = const { RefCell::new(Value::Null) };
}

/// One addon page in the slot.
pub struct AddonView {
    page: WebKitPage,
    addon: RefCell<Active>,
    ptr: usize,
    visible: Cell<bool>,
    retired: Cell<bool>,
}

impl AddonView {
    pub fn new(shell: &Shell, active: &Active) -> Rc<AddonView> {
        let profile = ProfileId::new(PROFILE)
            .and_then(|id| shell.engine().profile(&id, false))
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "the addons profile is unavailable; using an ephemeral one");
                let id = ProfileId::new(PROFILE).expect("a valid profile id");
                shell
                    .engine()
                    .profile(&id, true)
                    .expect("an ephemeral profile")
            });
        let page = shell.engine().new_page(
            &profile,
            PageOptions {
                developer_extras: developer_mode(),
                ..Default::default()
            },
        );
        let widget = page.widget();
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        let ptr = policy::view_ptr(&widget);
        let view = Rc::new(AddonView {
            page,
            addon: RefCell::new(active.clone()),
            ptr,
            visible: Cell::new(false),
            retired: Cell::new(false),
        });
        view.install_scripts();
        view.install_rules(shell);
        view.listen(shell);
        VIEWS.with(|v| {
            let mut v = v.borrow_mut();
            v.retain(|w| w.strong_count() > 0);
            v.push(Rc::downgrade(&view));
        });
        scheme::set_view(ptr);
        view.load();
        tracing::info!(id = %active.id, version = %active.version, "addon view made");
        view
    }

    pub fn widget(&self) -> gtk::Widget {
        self.page.widget().clone().upcast()
    }

    pub fn page(&self) -> &WebKitPage {
        &self.page
    }

    pub fn addon_id(&self) -> String {
        self.addon.borrow().id.clone()
    }

    pub fn addon(&self) -> Active {
        self.addon.borrow().clone()
    }

    /// The slot's addon again, maybe changed: a new bundle reloads the
    /// page; new settings are sent as a `settings` event.
    pub fn update(&self, active: &Active) {
        let before = self.addon.replace(active.clone());
        scheme::set_view(self.ptr);
        if before.sha256 != active.sha256 || before.dir != active.dir {
            tracing::info!(id = %active.id, version = %active.version, "addon updated; reloading");
            self.load();
        } else if before.settings != active.settings {
            self.emit("settings", active.settings.clone());
        }
    }

    /// The addon left the slot: stop answering it and let the page go.
    pub fn retire(&self) {
        self.retired.set(true);
        if VIEWS.with(|v| {
            v.borrow()
                .iter()
                .filter_map(Weak::upgrade)
                .all(|v| v.retired.get())
        }) {
            scheme::set_view(0);
        }
        self.page.load_html("", Some("about:blank"));
    }

    pub fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
    }

    fn load(&self) {
        let id = self.addon.borrow().id.clone();
        self.page.load(&format!("{}://{id}/", scheme::SCHEME));
    }

    fn install_scripts(&self) {
        let mut transport = UserScript::new(PAGE_TRANSPORT);
        transport.injection_time = InjectionTime::DocumentStart;
        transport.frames = Frames::TopOnly;
        self.page.add_user_script(&transport);
        let mut shim = UserScript::new(core_glue::bridge_js());
        shim.injection_time = InjectionTime::DocumentStart;
        shim.frames = Frames::TopOnly;
        self.page.add_user_script(&shim);
        let world = World::isolated(WORLD);
        self.page.add_message_handler(HANDLER, &world);
        let mut relay = UserScript::new(HOST_RELAY);
        relay.injection_time = InjectionTime::DocumentStart;
        relay.frames = Frames::TopOnly;
        relay.world = world;
        self.page.add_user_script(&relay);
    }

    /// CONTRACT §5.4: stay inside the addon; web links go to a tab.
    fn install_rules(self: &Rc<Self>, shell: &Shell) {
        let weak = Rc::downgrade(self);
        let shell_weak = shell.downgrade();
        policy::on_decide(&self.widget(), move |nav| {
            let Some(view) = weak.upgrade() else {
                return Verdict::Refuse;
            };
            let id = view.addon_id();
            if nav.new_window {
                if policy::is_web(&nav.uri)
                    && nav.user_gesture
                    && let Some(s) = shell_weak.upgrade()
                {
                    open_tab(&s, &nav.uri);
                }
                return Verdict::Refuse;
            }
            if policy::in_slot(&nav.uri, &id) && !view.retired.get() {
                return Verdict::Default;
            }
            if nav.uri == "about:blank" && view.retired.get() {
                return Verdict::Default;
            }
            if policy::is_web(&nav.uri)
                && nav.link
                && nav.user_gesture
                && let Some(s) = shell_weak.upgrade()
            {
                open_tab(&s, &nav.uri);
            } else {
                tracing::debug!(uri = %nav.uri, "addon navigation refused");
            }
            Verdict::Refuse
        });
    }

    fn listen(self: &Rc<Self>, shell: &Shell) {
        let mut events = self.page.events();
        let weak = Rc::downgrade(self);
        let shell_weak = shell.downgrade();
        glib::spawn_future_local(async move {
            while let Some(ev) = events.next().await {
                let Some(view) = weak.upgrade() else { break };
                match ev {
                    PageEvent::ScriptMessage { name, body } if name == HANDLER => {
                        let text = match body {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        view.received(text);
                    }
                    // Ctrl- and middle-clicks the engine turned into a tab.
                    PageEvent::NewWindow(req) => {
                        if let (Some(url), Some(s)) = (req.url, shell_weak.upgrade())
                            && policy::is_web(&url)
                            && req.user_gesture
                        {
                            open_tab(&s, &url);
                        }
                    }
                    PageEvent::Crashed(reason) => {
                        tracing::warn!(?reason, id = %view.addon_id(), "addon page crashed; reloading");
                        let w = Rc::downgrade(&view);
                        glib::timeout_add_local_once(
                            std::time::Duration::from_secs(1),
                            move || {
                                if let Some(v) = w.upgrade()
                                    && !v.retired.get()
                                {
                                    v.load();
                                }
                            },
                        );
                    }
                    _ => {}
                }
            }
        });
    }

    /// One bridge message from the page.
    fn received(self: &Rc<Self>, text: String) {
        if self.retired.get() {
            return;
        }
        // CONTRACT §5.1: only from the slot's own view, showing its addon.
        let active = core_glue::active();
        let ok = scheme_view() == self.ptr
            && active
                .as_ref()
                .is_some_and(|a| a.id == self.addon_id() && a.sha256 == self.addon.borrow().sha256)
            && self
                .page
                .url()
                .is_some_and(|u| policy::in_slot(&u, &self.addon_id()));
        if !ok {
            tracing::warn!(url = ?self.page.url(), "bridge message refused: not the slot's addon");
            return;
        }
        let Some(active) = active else { return };
        let weak = Rc::downgrade(self);
        let sha = active.sha256.clone();
        let host = host::Host::new(&active);
        core_glue::dispatch(&active, text, host, move |js| {
            let Some(view) = weak.upgrade() else { return };
            if view.retired.get() || view.addon.borrow().sha256 != sha {
                return;
            }
            view.run(&js);
        });
    }

    /// Run `js` in the page world (replies and events).
    fn run(&self, js: &str) {
        let fut = self.page.eval(js, &World::Page);
        glib::spawn_future_local(async move {
            if let Err(e) = fut.await {
                tracing::debug!(error = %e, "addon script failed");
            }
        });
    }

    /// `window.__copperAddon.emit(name, payload)`.
    pub fn emit(&self, name: &str, payload: Value) {
        if self.retired.get() {
            return;
        }
        self.run(&event_js(name, &payload));
    }
}

/// The JS that fires one bridge event (CONTRACT §5.1): the core's
/// script-safe `emit`, skipped while the shim isn't there yet.
pub fn event_js(name: &str, payload: &Value) -> String {
    format!(
        "window.__copperAddon && {};",
        core_glue::emit_js(name, payload)
    )
}

/// Send an event to every live addon view.
pub fn emit_all(name: &str, payload: Value) {
    for v in live() {
        v.emit(name, payload.clone());
    }
}

fn live() -> Vec<Rc<AddonView>> {
    VIEWS.with(|v| {
        let mut v = v.borrow_mut();
        v.retain(|w| w.strong_count() > 0);
        v.iter()
            .filter_map(Weak::upgrade)
            .filter(|v| !v.retired.get())
            .collect()
    })
}

fn scheme_view() -> usize {
    live().first().map_or(0, |v| v.ptr)
}

/// The inspector is there only for people building addons.
pub fn developer_mode() -> bool {
    cfg!(debug_assertions) || core_glue::dev_dir().is_some()
}

/// Open `url` in a new foreground tab, next to the current one.
pub fn open_tab(shell: &Shell, url: &str) {
    shell.open(
        url,
        OpenTab {
            foreground: true,
            after: shell.active_id(),
            ..Default::default()
        },
    );
}

/// The active tab's `{url, title}` (`null` for none, and for private tabs).
pub fn current_page(shell: &Shell) -> Value {
    if shell.active_is_private() {
        return Value::Null;
    }
    let Some(id) = shell.active_id() else {
        return Value::Null;
    };
    match shell.tab_view(&id) {
        Some(t) if !t.url.is_empty() => json!({ "url": t.url, "title": t.label }),
        _ => Value::Null,
    }
}

/// Theme, page and settings events, from the shell to every addon view.
pub fn watch_shell(shell: &Shell) {
    adw::StyleManager::default().connect_dark_notify(|sm| {
        emit_all("theme", json!({ "dark": sm.is_dark() }));
    });
    let mut rx = shell.hub().subscribe();
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        use crate::control::ShellEvent as E;
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let Some(s) = weak.upgrade() else { break };
            let relevant = match &ev {
                E::Selected(_) | E::SpaceSwitched(_) => true,
                E::TabChanged(id) | E::Loaded(id) | E::TabClosed(id) => {
                    s.active_id().as_ref() == Some(id) || matches!(ev, E::TabClosed(_))
                }
                _ => false,
            };
            if relevant {
                page_changed(&s);
            }
        }
    });
}

/// The active tab changed: views with the `page` capability hear of it.
fn page_changed(shell: &Shell) {
    let now = current_page(shell);
    let same = LAST_PAGE.with(|l| {
        let mut l = l.borrow_mut();
        if *l == now {
            true
        } else {
            *l = now.clone();
            false
        }
    });
    if same {
        return;
    }
    for v in live() {
        if core_glue::granted(&v.addon(), "page") {
            v.emit("page", now.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_script_safe() {
        let js = event_js(
            "page",
            &json!({"title": "</script><b>\u{2028}", "url": "https://example.com/"}),
        );
        assert!(js.starts_with("window.__copperAddon && window.__copperAddon.emit(\"page\","));
        assert!(!js.contains("</script>"));
        assert!(!js.contains('\u{2028}'));
        assert!(js.contains("\\u003c/script\\u003e"));
    }
}
