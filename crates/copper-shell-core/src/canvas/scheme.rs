//! The `copper:` URI scheme, through the engine
//! ([`copper_engine::Engine::register_scheme`]).
//!
//! The Mac catches `copper://canvas/<id>` on its way into WebKit and loads
//! the bundled page from a file in its place. WebKitGTK (and WebView2) have
//! real custom schemes, so here the address is served as it is: the page's
//! own URL is `copper://canvas/<id>`, which the field, the session and the
//! agents see.
//!
//! Registered once, before the first page. The engine marks the scheme
//! secure (the board needs `crypto.randomUUID`).
//!
//! Hosts register a handler ([`register_host`]); a request is answered only
//! for a page that is a tab's page (never a popup, the backstage or a page
//! another view opened), and the handler is told which tab asks.

use crate::Platform;
use crate::shell::{Shell, current};
use copper_core::TabId;
use copper_engine::{Engine, PageId, PageView, SchemeBody, SchemeRequest, SchemeResponse};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// What a host handler answers: the body and its content type, or a refusal.
pub type Answer = Result<(Vec<u8>, &'static str), String>;

/// A host's handler: the shell, the tab asking, the full URI.
pub type Handler<P> = Rc<dyn Fn(&Shell<P>, &TabId, &str) -> Answer>;

thread_local! {
    /// Host name → its `Handler<P>` (one platform per process).
    static HOSTS: RefCell<HashMap<String, Rc<dyn Any>>> = RefCell::new(HashMap::new());
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
}

/// Serve `copper://<host>/…` with `handler`. Main thread.
pub fn register_host<P: Platform>(host: &str, handler: Handler<P>) {
    HOSTS.with(|h| {
        h.borrow_mut()
            .insert(host.to_ascii_lowercase(), Rc::new(handler) as Rc<dyn Any>)
    });
}

/// Register the scheme with the shell's engine (once).
pub fn install<P: Platform>(shell: &Shell<P>) {
    if REGISTERED.with(|r| r.replace(true)) {
        return;
    }
    match shell
        .engine()
        .register_scheme("copper", Box::new(on_request::<P>))
    {
        Ok(()) => tracing::info!("copper:// scheme registered"),
        Err(e) => tracing::error!(error = %e, "copper:// pages are off"),
    }
}

fn on_request<P: Platform>(request: &SchemeRequest) -> SchemeResponse {
    answer::<P>(&request.uri, request.page).map(|(data, mime)| SchemeBody {
        data,
        mime: mime.to_owned(),
    })
}

fn answer<P: Platform>(uri: &str, page: Option<PageId>) -> Answer {
    let shell = current::<P>().ok_or("no window")?;
    let host = url::Url::parse(uri)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    let handler = HOSTS
        .with(|h| h.borrow().get(&host).cloned())
        .and_then(|h| h.downcast_ref::<Handler<P>>().cloned())
        .ok_or_else(|| format!("nothing at copper://{host}"))?;
    let tab = page
        .and_then(|p| tab_of_page(&shell, p))
        .ok_or("copper:// pages open only in a tab")?;
    handler(&shell, &tab, uri)
}

/// The tab whose live page is `page`.
pub fn tab_of_page<P: Platform>(shell: &Shell<P>, page: PageId) -> Option<TabId> {
    shell
        .pages_map()
        .borrow()
        .iter()
        .find(|(_, slot)| slot.page.page_id() == page)
        .map(|(id, _)| id.clone())
}
