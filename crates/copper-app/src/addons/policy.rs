//! Where an addon's web view (and its sign-in window) may go (Mac:
//! `Fork/Addons/AddonHost.swift`, its `WKNavigationDelegate`).
//!
//! The engine crate has no navigation-policy hook, so [`on_decide`] connects
//! WebKitGTK's own `decide-policy` signal on the view widget and reads the
//! decision through the C API (the library is already linked through
//! `copper-engine-webkitgtk`, as for `canvas::scheme`). The engine's own
//! handler runs first: a Ctrl- or middle-click it has already turned into a
//! new-tab request never reaches here.
//!
//! The rules themselves are plain functions ([`in_slot`], [`is_web`],
//! [`redirect_matches`], [`clamp_width`]) so they are tested without a
//! display. Which files a `copper-addon:` address may name, and as what
//! type, is the core's (`copper_core::addons::scheme`).

use glib::prelude::*;
use glib::translate::ToGlibPtr;
use std::ffi::{CStr, c_char, c_uint, c_void};

/// The pane's width limits and default, in pixels (Mac `AgentPane.swift`,
/// `agent.paneWidth`, 340…620, default 400).
pub const MIN_WIDTH: i32 = 340;
pub const MAX_WIDTH: i32 = 620;
pub const DEFAULT_WIDTH: i32 = 400;

/// A stored or dragged width, inside the limits (anything unreadable is the
/// default).
pub fn clamp_width(w: Option<f64>) -> i32 {
    match w {
        Some(w) if w.is_finite() => {
            (w.round() as i64).clamp(MIN_WIDTH as i64, MAX_WIDTH as i64) as i32
        }
        _ => DEFAULT_WIDTH,
    }
}

/// `uri` is the addon `id`'s own page: `copper-addon://<id>/…`, the host
/// exactly the id (no user, no port).
pub fn in_slot(uri: &str, id: &str) -> bool {
    let Ok(u) = url::Url::parse(uri) else {
        return false;
    };
    u.scheme() == "copper-addon"
        && u.username().is_empty()
        && u.password().is_none()
        && u.port().is_none()
        && u.host_str()
            .is_some_and(|h| !id.is_empty() && h.eq_ignore_ascii_case(id))
}

/// The id an addon address names (`copper-addon://<id>/…`), lower-cased.
pub fn addon_id(uri: &str) -> Option<String> {
    let u = url::Url::parse(uri).ok()?;
    if u.scheme() != "copper-addon" {
        return None;
    }
    u.host_str().map(str::to_ascii_lowercase)
}

/// An ordinary web address (what a link in the pane may open in a tab).
pub fn is_web(uri: &str) -> bool {
    url::Url::parse(uri).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host())
}

/// `uri` is the sign-in redirect the addon waits for.
pub fn redirect_matches(uri: &str, prefix: &str) -> bool {
    !prefix.is_empty() && uri.starts_with(prefix)
}

// MARK: - decide-policy

/// One navigation WebKit asks about.
#[derive(Debug, Clone)]
pub struct Navigation {
    pub uri: String,
    /// A new window (`target=_blank`, `window.open`) rather than this view.
    pub new_window: bool,
    /// A link was clicked (not script, not a form, not a redirect).
    pub link: bool,
    pub user_gesture: bool,
    /// A server redirect of a navigation already under way.
    pub redirect: bool,
}

/// What to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// WebKit's own default.
    Default,
    /// Go ahead.
    Allow,
    /// Stop it.
    Refuse,
}

#[repr(C)]
struct WebKitPolicyDecision {
    _p: [u8; 0],
}
#[repr(C)]
struct WebKitNavigationAction {
    _p: [u8; 0],
}
#[repr(C)]
struct WebKitURIRequest {
    _p: [u8; 0],
}

const NAVIGATION_ACTION: i32 = 0;
const NEW_WINDOW_ACTION: i32 = 1;
const LINK_CLICKED: c_uint = 0;

#[link(name = "webkitgtk-6.0")]
unsafe extern "C" {
    fn webkit_navigation_policy_decision_get_navigation_action(
        decision: *mut WebKitPolicyDecision,
    ) -> *mut WebKitNavigationAction;
    fn webkit_navigation_action_get_request(
        action: *mut WebKitNavigationAction,
    ) -> *mut WebKitURIRequest;
    fn webkit_navigation_action_get_navigation_type(action: *mut WebKitNavigationAction) -> c_uint;
    fn webkit_navigation_action_is_user_gesture(action: *mut WebKitNavigationAction) -> i32;
    fn webkit_navigation_action_is_redirect(action: *mut WebKitNavigationAction) -> i32;
    fn webkit_uri_request_get_uri(request: *mut WebKitURIRequest) -> *const c_char;
    fn webkit_policy_decision_ignore(decision: *mut WebKitPolicyDecision);
    fn webkit_policy_decision_use(decision: *mut WebKitPolicyDecision);
}

/// Decide every navigation and new-window request of the WebKit view
/// `widget` with `f`. Responses are left to WebKit.
pub fn on_decide(widget: &gtk4::Widget, f: impl Fn(&Navigation) -> Verdict + 'static) {
    if widget.type_().name() != "WebKitWebView" {
        tracing::warn!(
            kind = widget.type_().name(),
            "not a WebKit view: no navigation rules"
        );
        return;
    }
    widget.connect_local("decide-policy", false, move |values| {
        let decision = values.get(1)?.get::<glib::Object>().ok()?;
        let kind = values.get(2)?;
        // SAFETY: the third argument of `decide-policy` is a
        // `WebKitPolicyDecisionType` enum value.
        let kind = unsafe { glib::gobject_ffi::g_value_get_enum(kind.to_glib_none().0) };
        if kind != NAVIGATION_ACTION && kind != NEW_WINDOW_ACTION {
            return Some(false.to_value());
        }
        let ptr = decision.as_ptr() as *mut WebKitPolicyDecision;
        // SAFETY: navigation and new-window decisions are
        // `WebKitNavigationPolicyDecision`s; the action, its request and
        // the URI belong to the decision, which `decision` keeps alive for
        // this call. Nothing is kept past it.
        let nav = unsafe {
            let action = webkit_navigation_policy_decision_get_navigation_action(ptr);
            if action.is_null() {
                return Some(false.to_value());
            }
            let request = webkit_navigation_action_get_request(action);
            let uri = if request.is_null() {
                String::new()
            } else {
                let p = webkit_uri_request_get_uri(request);
                if p.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            };
            Navigation {
                uri,
                new_window: kind == NEW_WINDOW_ACTION,
                link: webkit_navigation_action_get_navigation_type(action) == LINK_CLICKED,
                user_gesture: webkit_navigation_action_is_user_gesture(action) != 0,
                redirect: webkit_navigation_action_is_redirect(action) != 0,
            }
        };
        let verdict = f(&nav);
        // SAFETY: the decision is live for this call; it is answered once.
        unsafe {
            match verdict {
                Verdict::Default => return Some(false.to_value()),
                Verdict::Allow => webkit_policy_decision_use(ptr),
                Verdict::Refuse => webkit_policy_decision_ignore(ptr),
            }
        }
        Some(true.to_value())
    });
}

/// The WebKit view behind `widget`, as a plain pointer (only ever compared).
pub fn view_ptr(widget: &gtk4::Widget) -> usize {
    widget.as_ptr() as *mut c_void as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_stay_inside_the_limits() {
        assert_eq!(clamp_width(None), 400);
        assert_eq!(clamp_width(Some(f64::NAN)), 400);
        assert_eq!(clamp_width(Some(100.0)), 340);
        assert_eq!(clamp_width(Some(339.6)), 340);
        assert_eq!(clamp_width(Some(512.4)), 512);
        assert_eq!(clamp_width(Some(9000.0)), 620);
        assert_eq!(clamp_width(Some(f64::INFINITY)), 400);
    }

    #[test]
    fn the_slot_keeps_to_its_own_addon() {
        let id = "example.hello";
        assert!(in_slot("copper-addon://example.hello/", id));
        assert!(in_slot("copper-addon://example.hello/index.html?x=1#y", id));
        assert!(in_slot("copper-addon://EXAMPLE.hello/app.js", id));
        assert!(!in_slot("copper-addon://example.other/", id));
        assert!(!in_slot("copper-addon://example.hello.evil/", id));
        assert!(!in_slot("copper-addon://user@example.hello/", id));
        assert!(!in_slot("copper-addon://example.hello:8080/", id));
        assert!(!in_slot("https://example.hello/", id));
        assert!(!in_slot("about:blank", id));
        assert!(!in_slot("copper-addon://example.hello/", ""));
        assert_eq!(
            addon_id("copper-addon://Example.Hello/x").as_deref(),
            Some("example.hello")
        );
        assert_eq!(addon_id("https://example.com/"), None);
    }

    #[test]
    fn web_links_and_redirects() {
        assert!(is_web("https://example.com/a"));
        assert!(is_web("http://127.0.0.1:8080/"));
        assert!(!is_web("javascript:alert(1)"));
        assert!(!is_web("file:///etc/passwd"));
        assert!(!is_web("copper-addon://example.hello/"));
        assert!(!is_web("data:text/html,hi"));
        let prefix = "http://127.0.0.1:53682/oauth/callback";
        assert!(redirect_matches(
            "http://127.0.0.1:53682/oauth/callback?code=abc&state=s",
            prefix
        ));
        assert!(!redirect_matches("http://127.0.0.1:53682/other", prefix));
        assert!(!redirect_matches("https://example.com/", ""));
    }
}
