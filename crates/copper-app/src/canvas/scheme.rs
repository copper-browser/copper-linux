//! The `copper:` URI scheme on WebKit's default web context.
//!
//! The Mac catches `copper://canvas/<id>` on its way into WebKit and loads
//! the bundled page from a file in its place. WebKitGTK has real custom
//! schemes, so here the address is served as it is: the page's own URL is
//! `copper://canvas/<id>`, which the field, the session and the agents see.
//!
//! The engine crate has no URI-scheme API, so this calls WebKitGTK 6.0's C
//! API directly (the library is already linked through
//! `copper-engine-webkitgtk`). Registered once, before the first page.
//! The scheme is marked secure (the board needs `crypto.randomUUID`).
//!
//! Hosts register a handler ([`register_host`]); a request is answered only
//! for a view that is a tab's page (never a popup, the backstage or a page
//! another view opened), and the handler is told which tab asks.

use crate::shell::Shell;
use copper_core::TabId;
use glib::translate::{IntoGlibPtr, ToGlibPtr};
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::rc::Rc;

#[repr(C)]
struct WebKitWebContext {
    _p: [u8; 0],
}
#[repr(C)]
struct WebKitSecurityManager {
    _p: [u8; 0],
}
#[repr(C)]
pub struct WebKitURISchemeRequest {
    _p: [u8; 0],
}

type SchemeCallback = unsafe extern "C" fn(*mut WebKitURISchemeRequest, glib::ffi::gpointer);

#[link(name = "webkitgtk-6.0")]
unsafe extern "C" {
    fn webkit_web_context_get_default() -> *mut WebKitWebContext;
    fn webkit_web_context_register_uri_scheme(
        context: *mut WebKitWebContext,
        scheme: *const c_char,
        callback: Option<SchemeCallback>,
        user_data: glib::ffi::gpointer,
        destroy: glib::ffi::GDestroyNotify,
    );
    fn webkit_web_context_get_security_manager(
        context: *mut WebKitWebContext,
    ) -> *mut WebKitSecurityManager;
    fn webkit_security_manager_register_uri_scheme_as_secure(
        manager: *mut WebKitSecurityManager,
        scheme: *const c_char,
    );
    fn webkit_uri_scheme_request_get_uri(request: *mut WebKitURISchemeRequest) -> *const c_char;
    fn webkit_uri_scheme_request_get_web_view(request: *mut WebKitURISchemeRequest) -> *mut c_void;
    fn webkit_uri_scheme_request_finish(
        request: *mut WebKitURISchemeRequest,
        stream: *mut gio::ffi::GInputStream,
        length: i64,
        content_type: *const c_char,
    );
    fn webkit_uri_scheme_request_finish_error(
        request: *mut WebKitURISchemeRequest,
        error: *mut glib::ffi::GError,
    );
}

/// What a host handler answers: the body and its content type, or a refusal.
pub type Answer = Result<(Vec<u8>, &'static str), String>;

/// A host's handler: the shell, the tab asking, the full URI.
pub type Handler = Rc<dyn Fn(&Shell, &TabId, &str) -> Answer>;

thread_local! {
    static HOSTS: RefCell<HashMap<String, Handler>> = RefCell::new(HashMap::new());
    static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Serve `copper://<host>/…` with `handler`. Main thread.
pub fn register_host(host: &str, handler: Handler) {
    HOSTS.with(|h| h.borrow_mut().insert(host.to_ascii_lowercase(), handler));
}

/// Register the scheme on the default web context (once).
pub fn install() {
    if REGISTERED.with(|r| r.replace(true)) {
        return;
    }
    let scheme = CString::new("copper").expect("no NUL");
    // SAFETY: plain calls into WebKitGTK on the main thread; the scheme
    // string is copied by WebKit; the callback is a static function and
    // takes no user data.
    unsafe {
        let ctx = webkit_web_context_get_default();
        if ctx.is_null() {
            tracing::error!("no default WebKit web context; copper:// pages are off");
            return;
        }
        let sm = webkit_web_context_get_security_manager(ctx);
        if !sm.is_null() {
            webkit_security_manager_register_uri_scheme_as_secure(sm, scheme.as_ptr());
        }
        webkit_web_context_register_uri_scheme(
            ctx,
            scheme.as_ptr(),
            Some(on_request),
            std::ptr::null_mut(),
            None,
        );
    }
    tracing::info!("copper:// scheme registered");
}

unsafe extern "C" fn on_request(request: *mut WebKitURISchemeRequest, _data: glib::ffi::gpointer) {
    // SAFETY: WebKit hands a valid request for the duration of this call;
    // the URI is a NUL-terminated string it owns.
    let uri = unsafe {
        let p = webkit_uri_scheme_request_get_uri(request);
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    // SAFETY: as above; the view pointer is only compared, never dereferenced.
    let view = unsafe { webkit_uri_scheme_request_get_web_view(request) } as usize;
    let answer = answer(&uri, view);
    // SAFETY: the request is finished exactly once, here.
    unsafe { finish(request, answer) };
}

fn answer(uri: &str, view: usize) -> Answer {
    let shell = crate::canvas::shell().ok_or("no window")?;
    let host = url::Url::parse(uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    let handler = HOSTS
        .with(|h| h.borrow().get(&host).cloned())
        .ok_or_else(|| format!("nothing at copper://{host}"))?;
    let tab = tab_of_view(&shell, view).ok_or("copper:// pages open only in a tab")?;
    handler(&shell, &tab, uri)
}

/// The tab whose live page is the view at `ptr`.
pub fn tab_of_view(shell: &Shell, ptr: usize) -> Option<TabId> {
    if ptr == 0 {
        return None;
    }
    shell
        .pages_map()
        .borrow()
        .iter()
        .find(|(_, slot)| slot.page.widget().as_ptr() as usize == ptr)
        .map(|(id, _)| id.clone())
}

/// # Safety
/// `request` must be a live request not yet finished.
unsafe fn finish(request: *mut WebKitURISchemeRequest, answer: Answer) {
    match answer {
        Ok((body, mime)) => {
            let len = body.len() as i64;
            let bytes = glib::Bytes::from_owned(body);
            let stream = gio::MemoryInputStream::from_bytes(&bytes);
            let stream: gio::InputStream = stream.upcast();
            let ct = CString::new(mime).unwrap_or_default();
            let ptr: *mut gio::ffi::GInputStream = stream.to_glib_none().0;
            // SAFETY: WebKit takes its own reference to the stream.
            unsafe { webkit_uri_scheme_request_finish(request, ptr, len, ct.as_ptr()) };
        }
        Err(text) => {
            tracing::debug!(%text, "copper:// refused");
            let error = glib::Error::new(gio::IOErrorEnum::NotFound, &text);
            let ptr: *mut glib::ffi::GError = error.into_glib_ptr();
            // SAFETY: WebKit copies the error; we free ours after.
            unsafe {
                webkit_uri_scheme_request_finish_error(request, ptr);
                glib::ffi::g_error_free(ptr);
            }
        }
    }
}
