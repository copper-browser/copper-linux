//! What agents' page tools need of a page (`copper_engine::PageView`'s
//! "agents' page tools" methods): its questions and file choosers taken
//! before anyone sees them, the requests it makes, a PDF of it, and the
//! position it is told.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use copper_engine::{
    ChosenFiles, Decision, DialogHandler, DialogId, DialogKind, DialogRequest, EngineError,
    FileChooserHandler, FileChooserInfo, GeoPosition, LocalFuture, NetLog, PermissionHandler,
    PermissionKind, Result,
};
use glib::prelude::*;
use gtk4::prelude::*;
use webkit6::prelude::*;
use webkit6::{FileChooserRequest, ScriptDialog, ScriptDialogType, WebResource, WebView};

/// A page's agent-facing state (`Shared::agent`).
#[derive(Default)]
pub(crate) struct AgentHooks {
    dialog_handler: RefCell<Option<DialogHandler>>,
    /// Questions taken, waiting for `answer_dialog`.
    dialogs: RefCell<HashMap<DialogId, ScriptDialog>>,
    chooser_handler: RefCell<Option<FileChooserHandler>>,
    /// The file chooser taken, waiting for `answer_file_chooser`.
    chooser: RefCell<Option<FileChooserRequest>>,
    /// Kept from `watch_network` on.
    net: Rc<RefCell<Option<NetLog>>>,
    watching: Cell<bool>,
    permission_handler: RefCell<Option<PermissionHandler>>,
}

impl AgentHooks {
    pub(crate) fn set_dialog_handler(&self, handler: Option<DialogHandler>) {
        *self.dialog_handler.borrow_mut() = handler;
    }

    pub(crate) fn set_chooser_handler(&self, handler: Option<FileChooserHandler>) {
        *self.chooser_handler.borrow_mut() = handler;
    }

    pub(crate) fn set_permission_handler(&self, handler: Option<PermissionHandler>) {
        *self.permission_handler.borrow_mut() = handler;
    }

    /// Whether the handler grants `kind` for the page at `url` now.
    pub(crate) fn grants(&self, kind: &PermissionKind, url: Option<&str>) -> bool {
        let handler = self.permission_handler.borrow_mut().take();
        let granted = handler.as_ref().is_some_and(|h| h(kind, url));
        if handler.is_some() && self.permission_handler.borrow().is_none() {
            *self.permission_handler.borrow_mut() = handler;
        }
        granted
    }

    /// `script-dialog`: offered to the handler first; `false` leaves it to
    /// the caller (WebKit's own dialog, or the backstage's closed answer).
    pub(crate) fn take_dialog(&self, dialog: &ScriptDialog) -> bool {
        let kind = match dialog.dialog_type() {
            ScriptDialogType::Alert => DialogKind::Alert,
            ScriptDialogType::Confirm => DialogKind::Confirm,
            ScriptDialogType::Prompt => DialogKind::Prompt,
            ScriptDialogType::BeforeUnloadConfirm => DialogKind::BeforeUnload,
            _ => return false,
        };
        let request = DialogRequest {
            id: DialogId(crate::next_id()),
            kind,
            message: dialog.message().map(String::from).unwrap_or_default(),
            default_text: (kind == DialogKind::Prompt)
                .then(|| dialog.prompt_get_default_text().map(String::from))
                .flatten(),
        };
        // Taken out while it runs: it may answer (re-entering this page).
        let handler = self.dialog_handler.borrow_mut().take();
        let decision = handler.as_ref().map_or(Decision::Pass, |h| h(&request));
        if handler.is_some() && self.dialog_handler.borrow().is_none() {
            *self.dialog_handler.borrow_mut() = handler;
        }
        match decision {
            Decision::Pass => false,
            Decision::Hold => {
                // Held (a reference kept) until `answer_dialog` closes it.
                self.dialogs.borrow_mut().insert(request.id, dialog.clone());
                true
            }
            Decision::Answer((accept, text)) => {
                // Answered inside the signal: WebKit completes it on return
                // (closing it here as well would finish it twice).
                set_answer(dialog, accept, text.as_deref());
                true
            }
        }
    }

    pub(crate) fn answer_dialog(&self, id: DialogId, accept: bool, text: Option<&str>) -> bool {
        let Some(dialog) = self.dialogs.borrow_mut().remove(&id) else {
            return false;
        };
        answer(&dialog, accept, text);
        true
    }

    /// The page is gone or its process died: every held question closes.
    pub(crate) fn close_all(&self) {
        let open: Vec<ScriptDialog> = self.dialogs.borrow_mut().drain().map(|(_, d)| d).collect();
        for dialog in open {
            dialog.close();
        }
        if let Some(chooser) = self.chooser.borrow_mut().take() {
            chooser.cancel();
        }
    }

    /// `run-file-chooser`: offered to the handler first; `true` when taken.
    pub(crate) fn take_chooser(&self, request: &FileChooserRequest) -> bool {
        let info = FileChooserInfo {
            multiple: request.selects_multiple(),
            // WebKitGTK says nothing of `webkitdirectory`.
            directories: false,
        };
        let handler = self.chooser_handler.borrow_mut().take();
        let decision = handler.as_ref().map_or(Decision::Pass, |h| h(&info));
        if handler.is_some() && self.chooser_handler.borrow().is_none() {
            *self.chooser_handler.borrow_mut() = handler;
        }
        match decision {
            Decision::Pass => false,
            Decision::Hold => {
                if let Some(old) = self.chooser.borrow_mut().replace(request.clone()) {
                    old.cancel();
                }
                true
            }
            Decision::Answer(files) => {
                choose(request, files);
                true
            }
        }
    }

    pub(crate) fn answer_chooser(&self, files: ChosenFiles) -> bool {
        let Some(request) = self.chooser.borrow_mut().take() else {
            return false;
        };
        choose(&request, files);
        true
    }

    /// A new document: a chooser of the old one is moot.
    pub(crate) fn committed(&self) {
        if let Some(chooser) = self.chooser.borrow_mut().take() {
            chooser.cancel();
        }
    }

    pub(crate) fn network(&self) -> Option<NetLog> {
        self.net.borrow().clone()
    }

    /// Listen to `view`'s loads from now on (once).
    pub(crate) fn watch(&self, view: &WebView) {
        if self.watching.replace(true) {
            return;
        }
        *self.net.borrow_mut() = Some(NetLog::default());
        let net = self.net.clone();
        view.connect_resource_load_started(move |v, resource, request| {
            let id = crate::next_id();
            let url = request.uri().map(String::from).unwrap_or_default();
            let method = request.http_method().map(String::from).unwrap_or_default();
            let main = v.main_resource().as_ref() == Some(resource);
            let kind = if main { "document" } else { kind_of("", &url) };
            if let Some(log) = net.borrow_mut().as_mut() {
                log.sent(id, &method, &url, kind, main);
            }
            follow(&net, resource, id, main);
        });
    }
}

/// Hand `files` to the input, or tell it nothing was chosen.
fn choose(request: &FileChooserRequest, files: ChosenFiles) {
    match files {
        Some(files) if !files.is_empty() => {
            let paths: Vec<&str> = files.iter().filter_map(|p| p.to_str()).collect();
            request.select_files(&paths);
        }
        _ => request.cancel(),
    }
}

/// A held question's answer, as a sheet would give it, and the page runs on.
fn answer(dialog: &ScriptDialog, accept: bool, text: Option<&str>) {
    set_answer(dialog, accept, text);
    dialog.close();
}

fn set_answer(dialog: &ScriptDialog, accept: bool, text: Option<&str>) {
    match dialog.dialog_type() {
        ScriptDialogType::Confirm | ScriptDialogType::BeforeUnloadConfirm => {
            dialog.confirm_set_confirmed(accept)
        }
        ScriptDialogType::Prompt if accept => {
            let default = dialog.prompt_get_default_text().map(String::from);
            dialog.prompt_set_text(text.or(default.as_deref()).unwrap_or(""));
        }
        _ => {}
    }
}

/// What a closed dialog answers a page nobody sees: OK, false, null — and
/// leave, for `beforeunload` (the agent asked to go).
pub(crate) fn decline(dialog: &ScriptDialog) {
    if dialog.dialog_type() == ScriptDialogType::BeforeUnloadConfirm {
        dialog.confirm_set_confirmed(true);
    }
}

/// One load's answer and end, into the log.
fn follow(net: &Rc<RefCell<Option<NetLog>>>, resource: &WebResource, id: u64, main: bool) {
    let n = Rc::downgrade(net);
    resource.connect_response_notify(move |r| {
        let (Some(net), Some(response)) = (n.upgrade(), r.response()) else {
            return;
        };
        if let Some(log) = net.borrow_mut().as_mut() {
            let status = response.status_code();
            if status > 0 {
                log.answered(id, u16::try_from(status).unwrap_or(u16::MAX));
            }
            if !main {
                let mime = response.mime_type().map(String::from).unwrap_or_default();
                let url = r.uri().map(String::from).unwrap_or_default();
                log.kind(id, kind_of(&mime, &url));
            }
        }
    });
    let n = Rc::downgrade(net);
    resource.connect_finished(move |_| {
        if let Some(net) = n.upgrade()
            && let Some(log) = net.borrow_mut().as_mut()
        {
            log.finished(id, None);
        }
    });
    let n = Rc::downgrade(net);
    resource.connect_failed(move |_, err| {
        if let Some(net) = n.upgrade()
            && let Some(log) = net.borrow_mut().as_mut()
        {
            log.finished(id, Some(err.message().to_string()));
        }
    });
}

/// A request's kind (the Mac's `NetLog.kind` names), from its response's
/// MIME type, else its address: WebKitGTK reports no resource type. Data
/// calls (JSON, text, no extension) read `fetch`.
pub(crate) fn kind_of(mime: &str, url: &str) -> &'static str {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !mime.is_empty() {
        if mime == "text/html" || mime == "application/xhtml+xml" {
            return "document";
        }
        if mime == "text/css" {
            return "stylesheet";
        }
        if mime.contains("javascript") || mime.contains("ecmascript") {
            return "script";
        }
        if mime.starts_with("image/") {
            return "image";
        }
        if mime.starts_with("font/") || mime.contains("font-woff") {
            return "font";
        }
        if mime.starts_with("audio/") || mime.starts_with("video/") {
            return "media";
        }
        if mime.contains("manifest") {
            return "manifest";
        }
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, e)| e)
        .unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "avif" | "bmp" => "image",
        "css" => "stylesheet",
        "js" | "mjs" => "script",
        "woff" | "woff2" | "ttf" | "otf" => "font",
        "mp4" | "webm" | "mp3" | "ogg" | "wav" | "m4a" => "media",
        _ => "fetch",
    }
}

/// Print `view` to a PDF at `path`, letter-sized pages, with no dialog.
pub(crate) fn print_pdf(view: &WebView, path: &Path) -> LocalFuture<Result<()>> {
    let uri = gio::File::for_path(path).uri();
    let op = webkit6::PrintOperation::new(view);
    let settings = gtk4::PrintSettings::new();
    settings.set_printer("Print to File");
    settings.set(gtk4::PRINT_SETTINGS_OUTPUT_URI, Some(uri.as_str()));
    settings.set(gtk4::PRINT_SETTINGS_OUTPUT_FILE_FORMAT, Some("pdf"));
    op.set_print_settings(&settings);
    let setup = gtk4::PageSetup::new();
    setup.set_paper_size(&gtk4::PaperSize::new(Some(gtk4::PAPER_NAME_LETTER)));
    op.set_page_setup(&setup);
    let (tx, rx) = futures_channel::oneshot::channel::<Result<()>>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let t = tx.clone();
    op.connect_failed(move |_, err| {
        if let Some(tx) = t.borrow_mut().take() {
            let _ = tx.send(Err(EngineError::Other(format!(
                "printing failed: {}",
                err.message()
            ))));
        }
    });
    op.connect_finished(move |_| {
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(Ok(()));
        }
    });
    op.print();
    Box::pin(async move {
        // The operation lives until it reports.
        let _op = op;
        rx.await
            .unwrap_or_else(|_| Err(EngineError::Other("printing never finished".into())))
    })
}

thread_local! {
    /// The position pages are told, and the contexts listening for it.
    static GEO: RefCell<(Option<GeoPosition>, Vec<glib::WeakRef<webkit6::GeolocationManager>>)> =
        const { RefCell::new((None, Vec::new())) };
}

/// From now on pages granted geolocation are told `position` (one for the
/// whole engine, as on the Mac); `None` gives WebKit its own provider back.
pub(crate) fn set_geolocation(view: &WebView, position: Option<GeoPosition>) {
    let manager = view.context().and_then(|c| c.geolocation_manager());
    GEO.with(|g| {
        let mut g = g.borrow_mut();
        g.0 = position;
        g.1.retain(|w| w.upgrade().is_some());
        if let Some(manager) = manager
            && !g.1.iter().any(|w| w.upgrade().as_ref() == Some(&manager))
        {
            manager.connect_start(|m| {
                let Some(p) = GEO.with(|g| g.borrow().0) else {
                    return false;
                };
                // Told once the manager has taken the start.
                let m = m.clone();
                glib::idle_add_local_once(move || tell(&m, p));
                true
            });
            g.1.push(manager.downgrade());
        }
    });
    if let Some(p) = position {
        let managers: Vec<_> =
            GEO.with(|g| g.borrow().1.iter().filter_map(|w| w.upgrade()).collect());
        for m in managers {
            tell(&m, p);
        }
    }
}

fn tell(manager: &webkit6::GeolocationManager, p: GeoPosition) {
    let mut fix = webkit6::GeolocationPosition::new(p.latitude, p.longitude, p.accuracy);
    manager.update_position(&mut fix);
}

#[cfg(test)]
mod tests {
    use super::kind_of;

    #[test]
    fn kinds_from_type_then_address() {
        assert_eq!(kind_of("image/png", "https://a.test/x"), "image");
        assert_eq!(
            kind_of("text/css; charset=utf-8", "https://a.test/x"),
            "stylesheet"
        );
        assert_eq!(kind_of("application/javascript", ""), "script");
        assert_eq!(kind_of("text/html", "https://a.test/frame"), "document");
        assert_eq!(kind_of("application/json", "https://a.test/api"), "fetch");
        assert_eq!(kind_of("", "https://a.test/logo.PNG?v=2"), "image");
        assert_eq!(kind_of("", "https://a.test/app.js"), "script");
        assert_eq!(kind_of("", "https://a.test/api/items"), "fetch");
        assert_eq!(kind_of("font/woff2", ""), "font");
    }
}
