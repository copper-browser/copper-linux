//! `copper-addon://<id>/<path>`: the files of the addon in the slot (Mac:
//! `Fork/Addons/AddonHost.swift`, `AddonFiles`, a `WKURLSchemeHandler`
//! copied from `SceneFiles`).
//!
//! The engine crate has no URI-scheme API yet, so — as `canvas::scheme`
//! does for `copper:` — this calls WebKitGTK 6.0's C API directly (the
//! library is already linked through `copper-engine-webkitgtk`). Registered
//! once on the default web context, before the first page.
//!
//! A request is answered only for the slot's own view, only for the addon
//! the slot shows, and only from that addon's installed bundle folder:
//!
//! * this file checks the view and the addon id; the core's `scheme::serve`
//!   ([`core_glue::serve`]) does the rest, the same as on every platform:
//!   `copper-addon://<id>/` is the manifest's entry, any other path must be
//!   a plain bundle path naming a file in the folder (no symlinks outside a
//!   developer folder), the right `Content-Type`, and anything else a 404;
//! * HTML gets the slot's Content-Security-Policy `<meta>` as the first
//!   thing in its `<head>`, and every answer the CSP, `nosniff` and
//!   `Cache-Control: no-cache` headers, so a new version shows at once.
//!
//! Files are read off the main loop, then the request is finished.

use super::core_glue;
use super::policy;
use glib::translate::{FromGlibPtrNone, ToGlibPtr};
use gtk4::prelude::*;
use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_uint, c_void};

/// The scheme.
pub const SCHEME: &str = "copper-addon";

#[repr(C)]
struct WebKitWebContext {
    _p: [u8; 0],
}
#[repr(C)]
struct WebKitSecurityManager {
    _p: [u8; 0],
}
/// The same opaque type `canvas::scheme` declares: both modules declare
/// `webkit_web_context_register_uri_scheme`, and its callback's type must be
/// the very same one in both (rustc's `clashing_extern_declarations`).
use crate::canvas::scheme::WebKitURISchemeRequest;
#[repr(C)]
struct WebKitURISchemeResponse {
    _p: [u8; 0],
}
#[repr(C)]
struct SoupMessageHeaders {
    _p: [u8; 0],
}

type SchemeCallback = unsafe extern "C" fn(*mut WebKitURISchemeRequest, glib::ffi::gpointer);

/// `SOUP_MESSAGE_HEADERS_RESPONSE`.
const SOUP_HEADERS_RESPONSE: c_uint = 1;

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
    fn webkit_uri_scheme_request_finish_with_response(
        request: *mut WebKitURISchemeRequest,
        response: *mut WebKitURISchemeResponse,
    );
    fn webkit_uri_scheme_response_new(
        stream: *mut gio::ffi::GInputStream,
        length: i64,
    ) -> *mut WebKitURISchemeResponse;
    fn webkit_uri_scheme_response_set_status(
        response: *mut WebKitURISchemeResponse,
        status: c_uint,
        reason: *const c_char,
    );
    fn webkit_uri_scheme_response_set_content_type(
        response: *mut WebKitURISchemeResponse,
        content_type: *const c_char,
    );
    fn webkit_uri_scheme_response_set_http_headers(
        response: *mut WebKitURISchemeResponse,
        headers: *mut SoupMessageHeaders,
    );
}

#[link(name = "soup-3.0")]
unsafe extern "C" {
    fn soup_message_headers_new(kind: c_uint) -> *mut SoupMessageHeaders;
    fn soup_message_headers_append(
        headers: *mut SoupMessageHeaders,
        name: *const c_char,
        value: *const c_char,
    );
}

thread_local! {
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    /// The slot's view (a pointer, only ever compared); 0 while there is none.
    static VIEW: Cell<usize> = const { Cell::new(0) };
}

/// The slot's view is `ptr` (0: none). Main thread.
pub fn set_view(ptr: usize) {
    VIEW.with(|v| v.set(ptr));
}

/// Register the scheme on the default web context (once).
pub fn install() {
    if REGISTERED.with(|r| r.replace(true)) {
        return;
    }
    let scheme = CString::new(SCHEME).expect("no NUL");
    // SAFETY: plain calls into WebKitGTK on the main thread; WebKit copies
    // the scheme string; the callback is a static function with no data.
    unsafe {
        let ctx = webkit_web_context_get_default();
        if ctx.is_null() {
            tracing::error!("no default WebKit web context; addons are off");
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
    tracing::info!("copper-addon:// scheme registered");
}

/// A 404 (and why, for the log).
fn not_found(why: &str) -> core_glue::Served {
    tracing::debug!(%why, "copper-addon:// 404");
    core_glue::Served {
        status: 404,
        mime: "text/plain; charset=utf-8",
        body: b"Not found".to_vec(),
        headers: vec![("Cache-Control", "no-cache".to_string())],
    }
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
    tracing::debug!(%uri, "copper-addon:// request");
    match check(&uri, view) {
        Ok(active) => {
            // SAFETY: the request is a GObject; this takes a reference of
            // our own, so it lives until the file is read and it is finished.
            let held: glib::Object =
                unsafe { glib::Object::from_glib_none(request as *mut glib::gobject_ffi::GObject) };
            core_glue::serve(&active, uri, move |served| {
                let ptr = held.as_ptr() as *mut WebKitURISchemeRequest;
                // SAFETY: `held` keeps the request alive; it is finished
                // exactly once, here.
                unsafe { finish(ptr, served) };
            });
        }
        Err(why) => {
            // SAFETY: the request is finished exactly once, here.
            unsafe { finish(request, not_found(&why)) };
        }
    }
}

/// The addon whose file `uri` names, if the slot's view may have it. The
/// core's `serve` then checks the path, the folder and the file itself.
fn check(uri: &str, view: usize) -> Result<core_glue::Active, String> {
    let mine = VIEW.with(Cell::get);
    if view == 0 || view != mine {
        return Err("not the addon slot's view".into());
    }
    let active = core_glue::active().ok_or("no addon in the slot")?;
    if !policy::in_slot(uri, &active.id) {
        return Err(format!("not {}", active.id));
    }
    Ok(active)
}

/// # Safety
/// `request` must be a live request not yet finished.
unsafe fn finish(request: *mut WebKitURISchemeRequest, answer: core_glue::Served) {
    let len = answer.body.len() as i64;
    let bytes = glib::Bytes::from_owned(answer.body);
    let stream: gio::InputStream = gio::MemoryInputStream::from_bytes(&bytes).upcast();
    // WebKit takes this as the bare MIME type: with `; charset=…` on it,
    // `nosniff` refuses the stylesheet or script. The whole value goes in
    // the `Content-Type` header (and the charset with it).
    let ct = CString::new(bare_mime(answer.mime)).unwrap_or_default();
    let reason = CString::new(reason(answer.status)).unwrap_or_default();
    let mut headers: Vec<(CString, CString)> = answer
        .headers
        .iter()
        .filter_map(|(n, v)| Some((CString::new(*n).ok()?, CString::new(v.as_str()).ok()?)))
        .collect();
    if let Ok(v) = CString::new(answer.mime) {
        headers.push((CString::new("Content-Type").expect("no NUL"), v));
    }
    if !answer
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("cache-control"))
    {
        headers.push((
            CString::new("Cache-Control").expect("no NUL"),
            CString::new("no-cache").expect("no NUL"),
        ));
    }
    let stream_ptr: *mut gio::ffi::GInputStream = stream.to_glib_none().0;
    // SAFETY: the response takes its own reference to the stream and full
    // ownership of the headers; we drop our response reference after
    // handing it to the request, which keeps its own.
    unsafe {
        let response = webkit_uri_scheme_response_new(stream_ptr, len);
        webkit_uri_scheme_response_set_status(
            response,
            c_uint::from(answer.status),
            reason.as_ptr(),
        );
        webkit_uri_scheme_response_set_content_type(response, ct.as_ptr());
        let h = soup_message_headers_new(SOUP_HEADERS_RESPONSE);
        if !h.is_null() {
            for (n, v) in &headers {
                soup_message_headers_append(h, n.as_ptr(), v.as_ptr());
            }
            webkit_uri_scheme_response_set_http_headers(response, h);
        }
        webkit_uri_scheme_request_finish_with_response(request, response);
        let obj: glib::Object =
            glib::translate::from_glib_full(response as *mut glib::gobject_ffi::GObject);
        drop(obj);
    }
}

/// `text/css` of `text/css; charset=utf-8`.
fn bare_mime(mime: &str) -> &str {
    mime.split(';').next().unwrap_or(mime).trim()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Internal Server Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_without_parameters() {
        assert_eq!(bare_mime("text/css; charset=utf-8"), "text/css");
        assert_eq!(bare_mime("image/png"), "image/png");
        assert_eq!(reason(404), "Not Found");
    }
}
