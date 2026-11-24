//! What an agent's page tools need from the engine beyond script and input
//! (Mac `Fork/MCP/AgentDialogs.swift`, `FileChooser.swift`, `NetLog.swift`,
//! `Permissions.swift`): a page's questions and file choosers taken before
//! anyone sees them, the requests it made, and the position it is told.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

/// One of a page's `alert()`, `confirm()`, `prompt()` or "leave this
/// page?" questions, while it waits for its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DialogId(pub u64);

/// What kind of question a page asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DialogKind {
    Alert,
    Confirm,
    Prompt,
    /// `beforeunload`: accept leaves the page, dismiss stays.
    BeforeUnload,
}

impl DialogKind {
    /// The word agents read (`alert`, `confirm`, `prompt`, `beforeunload`).
    pub fn word(self) -> &'static str {
        match self {
            DialogKind::Alert => "alert",
            DialogKind::Confirm => "confirm",
            DialogKind::Prompt => "prompt",
            DialogKind::BeforeUnload => "beforeunload",
        }
    }
}

/// A page's question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogRequest {
    pub id: DialogId,
    pub kind: DialogKind,
    pub message: String,
    /// A prompt's default answer.
    pub default_text: Option<String>,
}

/// What a handler decides, the moment a page asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision<T> {
    /// Left to the engine: shown to the user, or — on a page held
    /// backstage — answered at once as a closed dialog / cancelled chooser.
    Pass,
    /// Taken and kept waiting (the page stopped inside it) until it is
    /// answered ([`crate::PageView::answer_dialog`] /
    /// [`crate::PageView::answer_file_chooser`]).
    Hold,
    /// Taken and answered now.
    Answer(T),
}

/// A question's answer: OK (`true`) or Cancel, and a prompt's text.
pub type DialogDecision = Decision<(bool, Option<String>)>;

/// Decides what happens to a page's question. The page owns its handler:
/// capture the page weakly.
pub type DialogHandler = Box<dyn Fn(&DialogRequest) -> DialogDecision>;

/// A page's file chooser (`<input type=file>` clicked), as an agent sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileChooserInfo {
    /// The input takes several files.
    pub multiple: bool,
    /// The input takes folders (`webkitdirectory`), where the engine says.
    pub directories: bool,
}

/// Files for a file chooser; `None` cancels it.
pub type ChosenFiles = Option<Vec<PathBuf>>;

/// What happens to a page's file chooser (nothing is shown when taken).
pub type ChooserDecision = Decision<ChosenFiles>;

/// Decides what happens to a page's file chooser.
pub type FileChooserHandler = Box<dyn Fn(&FileChooserInfo) -> ChooserDecision>;

/// Decides a page's permission request before anyone is asked: `true`
/// grants it at once (a site the user or an agent allowed); `false` leaves it
/// to the engine and the shell (`PageEvent::PermissionRequest`, or refused
/// on a page held backstage). Arguments: the kind and the page's address.
pub type PermissionHandler = Box<dyn Fn(&crate::PermissionKind, Option<&str>) -> bool>;

/// A position pages are given (`browser_permissions` grant with latitude
/// and longitude): degrees, and metres of accuracy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeoPosition {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f64,
}

/// One request a page made ([`NetLog`]).
#[derive(Clone, Debug, PartialEq)]
pub struct NetEntry {
    /// The engine's id for the load.
    pub id: u64,
    pub method: String,
    pub url: String,
    /// `document`, `image`, `fetch`, `xhr`, `script`, `stylesheet`, `font`,
    /// `media`, … (the Mac's `NetLog.kind` names); `other` when unknown.
    pub kind: String,
    pub status: Option<u16>,
    pub error: Option<String>,
    /// Order of asking, in this log.
    pub seq: u64,
    /// The engine said the load ended.
    pub done: bool,
    /// When it was asked, and when its answer began (a status) or it ended.
    pub at: Instant,
    pub answered_at: Option<Instant>,
}

impl NetEntry {
    pub fn new(method: &str, url: &str, kind: &str) -> NetEntry {
        NetEntry {
            id: 0,
            method: method.to_string(),
            url: url.to_string(),
            kind: kind.to_string(),
            status: None,
            error: None,
            seq: 0,
            done: false,
            at: Instant::now(),
            answered_at: None,
        }
    }
}

/// The requests one page made since its main frame last loaded a document
/// (Mac `NetLog`): its last [`NetLog::CAP`], newest last. Engines feed it
/// from their own load reports, never from a script in the page, so a page
/// can't see, spoof or break it. It lives with its page, so a closed tab's
/// log never stands in for a new one's.
#[derive(Clone, Debug, Default)]
pub struct NetLog {
    entries: VecDeque<NetEntry>,
    sequence: u64,
    document_at: u64,
}

impl NetLog {
    /// Requests kept per page.
    pub const CAP: usize = 300;

    /// A request went out. `new_document`: the main frame's own document
    /// (a new page), which starts the log afresh.
    pub fn sent(&mut self, id: u64, method: &str, url: &str, kind: &str, new_document: bool) {
        self.sequence += 1;
        if new_document {
            self.entries.clear();
            self.document_at = self.sequence;
        }
        self.entries.push_back(NetEntry {
            id,
            seq: self.sequence,
            ..NetEntry::new(if method.is_empty() { "GET" } else { method }, url, kind)
        });
        while self.entries.len() > Self::CAP {
            self.entries.pop_front();
        }
    }

    /// What the engine learned of a request's kind after it went out.
    pub fn kind(&mut self, id: u64, kind: &str) {
        if let Some(e) = self.entry(id) {
            e.kind = kind.to_string();
        }
    }

    /// Its answer began, with `status`.
    pub fn answered(&mut self, id: u64, status: u16) {
        if let Some(e) = self.entry(id) {
            e.status = Some(status);
            e.answered_at.get_or_insert_with(Instant::now);
        }
    }

    /// It ended: `error` when it failed. A load that ended with no HTTP
    /// status (`file:`, `data:`) came back, so it reads 200, as on the Mac.
    pub fn finished(&mut self, id: u64, error: Option<String>) {
        if let Some(e) = self.entry(id) {
            e.done = true;
            e.answered_at.get_or_insert_with(Instant::now);
            match error {
                Some(why) => e.error = Some(why),
                None if e.status.is_none() => e.status = Some(200),
                None => {}
            }
        }
    }

    fn entry(&mut self, id: u64) -> Option<&mut NetEntry> {
        self.entries.iter_mut().rev().find(|e| e.id == id)
    }

    /// Every request kept, oldest first.
    pub fn entries(&self) -> Vec<NetEntry> {
        self.entries.iter().cloned().collect()
    }

    /// The last request's `seq` (requests after it are the next action's).
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The `seq` of the main frame's current document.
    pub fn document_at(&self) -> u64 {
        self.document_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_document_starts_the_log_and_answers_land_on_their_request() {
        let mut log = NetLog::default();
        log.sent(1, "GET", "https://a.test/", "document", true);
        log.sent(2, "POST", "https://a.test/api", "fetch", false);
        log.sent(3, "", "https://a.test/x.png", "other", false);
        log.kind(3, "image");
        log.answered(1, 200);
        log.finished(1, None);
        log.finished(2, Some("offline".into()));
        log.finished(3, None);
        let e = log.entries();
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].status, e[0].done), (Some(200), true));
        assert_eq!(e[1].error.as_deref(), Some("offline"));
        assert_eq!(e[1].status, None);
        assert_eq!((e[2].method.as_str(), e[2].kind.as_str()), ("GET", "image"));
        assert_eq!(e[2].status, Some(200), "a load with no status came back");
        assert_eq!(log.document_at(), 1);

        log.sent(9, "GET", "https://b.test/", "document", true);
        assert_eq!(log.entries().len(), 1);
        assert_eq!(log.document_at(), 4);
        log.finished(2, None);
        assert_eq!(
            log.entries().len(),
            1,
            "a load of the old page is not listed"
        );
    }

    #[test]
    fn the_log_keeps_its_last_cap() {
        let mut log = NetLog::default();
        for i in 0..(NetLog::CAP as u64 + 20) {
            log.sent(i, "GET", &format!("https://a.test/{i}"), "fetch", false);
        }
        let e = log.entries();
        assert_eq!(e.len(), NetLog::CAP);
        assert_eq!(e[0].id, 20);
        assert_eq!(log.sequence(), NetLog::CAP as u64 + 20);
    }

    #[test]
    fn dialog_words() {
        assert_eq!(DialogKind::Prompt.word(), "prompt");
        assert_eq!(DialogKind::BeforeUnload.word(), "beforeunload");
    }
}
