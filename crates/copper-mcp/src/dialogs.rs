//! A page's questions and file choosers on a tab an agent works in
//! (Mac `Fork/MCP/AgentDialogs.swift`, `FileChooser.swift`).
//!
//! `alert()`, `confirm()` and `prompt()` on an agent's tab nobody is
//! looking at were declined on the spot, or waited inside the page for
//! ever, and the agent never knew one was asked. Now every question on an
//! agent's tab is written down (what kind, what it said, what it was told)
//! and the next tool answer for that tab carries it under `### Dialogs`.
//! `browser_handle_dialog` (Playwright MCP's name and arguments) answers it:
//!
//! - called after the click, it answers the question still open: an
//!   unanswered question stays open until then — or until the agent's next
//!   other call on that tab, or [`HOLD_FOR`], which give it the answer a
//!   closed dialog gives (OK, false, null);
//! - called before, it arms the answer for the next question.
//!
//! A question is never held while one of Copper's own scripts waits on that
//! page ([`Page::waiting`]): the page is stopped inside the question, so the
//! script would wait on it too. That one gets the closed answer at once, or
//! the armed one. A tab on the user's screen keeps its dialog for the user,
//! unless the agent armed an answer first.
//!
//! File choosers work the same two ways (`browser_file_upload`): armed with
//! paths before the click, the next chooser (within [`ARMED_FOR`]) gets
//! them; opened by a click on a tab an agent is driving, it is held for the
//! agent's next call ([`HELD_FOR`] at most), never shown.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Weak;
use std::time::{Duration, Instant};

use copper_engine::{
    ChooserDecision, Decision, DialogDecision, DialogId, DialogKind, DialogRequest, FileChooserInfo,
};

use crate::Mcp;
use crate::host::{Content, Host, Page, TabId};

/// How long a question waits open for `browser_handle_dialog`.
pub const HOLD_FOR: Duration = Duration::from_secs(30);
/// How long an armed upload waits for its chooser.
pub const ARMED_FOR: Duration = Duration::from_secs(120);
/// How long a held chooser waits for `browser_file_upload`.
pub const HELD_FOR: Duration = Duration::from_secs(60);

/// What pages asked and were told, per tab (part of `Mcp::state`).
#[derive(Default)]
pub(crate) struct Book {
    /// One armed answer per tab, for the next question its page asks.
    armed: HashMap<TabId, (bool, Option<String>)>,
    /// The question each tab's page is stopped in.
    open: HashMap<TabId, Open>,
    /// Lines for the next answer about the tab.
    asked: HashMap<TabId, Vec<String>>,
    /// Files armed for the next chooser, and when.
    uploads: HashMap<TabId, (Vec<PathBuf>, Instant)>,
    /// The chooser held for the agent (its id, and what it takes).
    chooser: HashMap<TabId, (u64, FileChooserInfo)>,
    /// Tabs with an agent call in flight → how many.
    busy: HashMap<TabId, u32>,
    next: u64,
}

struct Open {
    id: DialogId,
    kind: DialogKind,
    message: String,
    default_text: Option<String>,
}

/// The answer a closed dialog gives: `(accept, word)`. Leaving is the
/// closed answer to `beforeunload` (the agent asked to go).
fn closed(kind: DialogKind) -> (bool, &'static str) {
    match kind {
        DialogKind::Alert => (true, "OK"),
        DialogKind::BeforeUnload => (true, "left the page"),
        DialogKind::Confirm | DialogKind::Prompt => (false, "dismissed"),
    }
}

/// Page words in quotes, at most 300 characters (`AgentDialogs.quoted`).
pub(crate) fn quoted(text: &str) -> String {
    let text = if text.chars().count() > 300 {
        format!("{}…", text.chars().take(299).collect::<String>())
    } else {
        text.to_string()
    };
    serde_json::to_string(&text).unwrap_or_else(|_| "\"\"".into())
}

impl Book {
    fn note(&mut self, tab: &TabId, kind: DialogKind, message: &str, outcome: &str) {
        let lines = self.asked.entry(tab.clone()).or_default();
        lines.push(format!("- {} {} → {outcome}", kind.word(), quoted(message)));
        if lines.len() > 20 {
            lines.drain(..lines.len() - 20);
        }
    }

    /// The chooser held for `tab`, if any: whether it takes several files,
    /// and folders.
    pub(crate) fn held_chooser(&self, tab: &TabId) -> Option<FileChooserInfo> {
        self.chooser.get(tab).map(|(_, info)| *info)
    }
}

impl<H: Host> Mcp<H> {
    /// Hook the tab's page up for agents: its questions and file choosers
    /// come here first, and its requests are kept (`AgentTabs.prepare` →
    /// `NetLog.watch`). Idempotent; called whenever a tab is readied.
    pub(crate) fn hook(&self, id: &TabId) {
        let Some(page) = self.host.page(id) else {
            return;
        };
        let (me, tab) = (self.me.clone(), id.clone());
        page.set_dialog_handler(Some(Box::new(move |request| {
            asked(&me, &tab, |m| m.dialog_asked(&tab, request))
        })));
        let (me, tab) = (self.me.clone(), id.clone());
        page.set_file_chooser_handler(Some(Box::new(move |info| {
            asked(&me, &tab, |m| m.chooser_asked(&tab, info))
        })));
        page.watch_network();
    }

    /// The page asks (`AgentDialogs.take`). Only a tab agents hold, or
    /// one with an answer armed, is the agent's; any other asks its user.
    fn dialog_asked(&self, tab: &TabId, request: &DialogRequest) -> DialogDecision {
        let held = self.state.borrow().held.contains_key(tab);
        let unattended = held && self.host.tab(tab).is_some_and(|t| !t.selected);
        let armed = self.state.borrow_mut().dialogs.armed.remove(tab);
        let (kind, message) = (request.kind, request.message.as_str());
        if let Some((accept, text)) = armed {
            let text = (kind == DialogKind::Prompt && accept).then(|| {
                text.or_else(|| request.default_text.clone())
                    .unwrap_or_default()
            });
            let outcome = match (&text, accept) {
                (Some(t), true) => format!("accepted with {}", quoted(t)),
                (None, true) => "accepted".into(),
                _ => "dismissed".into(),
            };
            self.state
                .borrow_mut()
                .dialogs
                .note(tab, kind, message, &outcome);
            return Decision::Answer((accept, text));
        }
        if !held {
            return Decision::Pass;
        }
        if !unattended {
            self.state.borrow_mut().dialogs.note(
                tab,
                kind,
                message,
                "shown to the user, who answers it",
            );
            return Decision::Pass;
        }
        let (accept, word) = closed(kind);
        // "Leave this page?" never waits: the agent asked to go, as on the
        // Mac, where WebKit never asks it at all. Arm a dismiss to stay.
        if kind == DialogKind::BeforeUnload {
            self.state
                .borrow_mut()
                .dialogs
                .note(tab, kind, message, word);
            return Decision::Answer((accept, None));
        }
        if self.host.page(tab).is_some_and(|p| p.waiting()) {
            let outcome = format!(
                "{word} — a script of Copper's was waiting on the page; arm browser_handle_dialog before the action to answer it"
            );
            self.state
                .borrow_mut()
                .dialogs
                .note(tab, kind, message, &outcome);
            return Decision::Answer((accept, None));
        }
        // A page asks one at a time; never leave one hanging.
        self.close_dialog(tab, "a new question came");
        {
            let mut state = self.state.borrow_mut();
            state.dialogs.open.insert(
                tab.clone(),
                Open {
                    id: request.id,
                    kind,
                    message: message.to_string(),
                    default_text: request.default_text.clone(),
                },
            );
            state.dialogs.note(
                tab,
                kind,
                message,
                "open — answer it with browser_handle_dialog (accept, promptText); your next other call on this tab dismisses it",
            );
        }
        let (me, tab, id) = (self.me.clone(), tab.clone(), request.id);
        self.host.spawn(Box::pin(async move {
            let Some(m) = me.upgrade() else { return };
            m.host.sleep(HOLD_FOR).await;
            let same = m
                .state
                .borrow()
                .dialogs
                .open
                .get(&tab)
                .is_some_and(|o| o.id == id);
            if same {
                m.close_dialog(&tab, &format!("unanswered for {} s", HOLD_FOR.as_secs()));
            }
        }));
        Decision::Hold
    }

    /// `browser_handle_dialog`: answer the question open on the tab, or arm
    /// the answer for the next one; says which (`AgentDialogs.handle`).
    pub(crate) fn handle_dialog(&self, tab: &TabId, accept: bool, text: Option<&str>) -> String {
        let open = self.state.borrow_mut().dialogs.open.remove(tab);
        let Some(question) = open else {
            self.state
                .borrow_mut()
                .dialogs
                .armed
                .insert(tab.clone(), (accept, text.map(str::to_string)));
            let how = if accept {
                format!(
                    "accepted{}",
                    text.map(|t| format!(" with {}", quoted(t)))
                        .unwrap_or_default()
                )
            } else {
                "dismissed".into()
            };
            return format!(
                "No dialog is open on this tab. The next alert, confirm or prompt will be {how} the moment it comes — now do what makes the page ask."
            );
        };
        let answer = (question.kind == DialogKind::Prompt && accept).then(|| {
            text.map(str::to_string)
                .or(question.default_text.clone())
                .unwrap_or_default()
        });
        if let Some(page) = self.host.page(tab) {
            page.answer_dialog(question.id, accept, answer.as_deref());
        }
        let how = match (&answer, accept) {
            (Some(t), true) => format!("Accepted with {}", quoted(t)),
            (None, true) => "Accepted".into(),
            _ => "Dismissed".into(),
        };
        format!(
            "{how}: the {} {}.",
            question.kind.word(),
            quoted(&question.message)
        )
    }

    /// The agent moved on (a call on this tab that isn't
    /// `browser_handle_dialog`): an open question gets the closed answer.
    pub(crate) fn move_on(&self, tab: Option<&TabId>) {
        if let Some(tab) = tab {
            self.close_dialog(tab, "you moved on without browser_handle_dialog");
        }
    }

    fn close_dialog(&self, tab: &TabId, why: &str) {
        let Some(question) = self.state.borrow_mut().dialogs.open.remove(tab) else {
            return;
        };
        let (accept, word) = closed(question.kind);
        self.state.borrow_mut().dialogs.note(
            tab,
            question.kind,
            &question.message,
            &format!("{word} ({why})"),
        );
        if let Some(page) = self.host.page(tab) {
            page.answer_dialog(question.id, accept, None);
        }
    }

    /// What the tab's pages asked since the last answer, once, as a section
    /// of a tool answer.
    pub(crate) fn dialog_report(&self, tab: Option<&TabId>) -> Vec<Content> {
        let Some(tab) = tab else { return Vec::new() };
        match self.state.borrow_mut().dialogs.asked.remove(tab) {
            Some(lines) if !lines.is_empty() => {
                vec![Content::Text(format!("### Dialogs\n{}", lines.join("\n")))]
            }
            _ => Vec::new(),
        }
    }

    // -- file choosers ----------------------------------------------------------

    /// The page opens a file chooser (`FileChooser.answer`).
    fn chooser_asked(&self, tab: &TabId, info: &FileChooserInfo) -> ChooserDecision {
        let armed = self.state.borrow_mut().dialogs.uploads.remove(tab);
        if let Some((files, at)) = armed
            && at.elapsed() < ARMED_FOR
        {
            let files = if info.multiple {
                files
            } else {
                files.into_iter().take(1).collect()
            };
            return Decision::Answer(Some(files));
        }
        let unattended = self.state.borrow().held.contains_key(tab)
            && self.host.tab(tab).is_some_and(|t| !t.selected);
        let busy = self
            .state
            .borrow()
            .dialogs
            .busy
            .get(tab)
            .is_some_and(|n| *n > 0);
        if !unattended && !busy {
            return Decision::Pass;
        }
        // One chooser at a time per page: the engine cancels the old one.
        let id = {
            let mut state = self.state.borrow_mut();
            state.dialogs.next += 1;
            let id = state.dialogs.next;
            state.dialogs.chooser.insert(tab.clone(), (id, *info));
            id
        };
        let (me, tab) = (self.me.clone(), tab.clone());
        self.host.spawn(Box::pin(async move {
            let Some(m) = me.upgrade() else { return };
            m.host.sleep(HELD_FOR).await;
            let same = m
                .state
                .borrow()
                .dialogs
                .chooser
                .get(&tab)
                .is_some_and(|(i, _)| *i == id);
            if same {
                m.answer_chooser(&tab, None);
            }
        }));
        Decision::Hold
    }

    /// Answer the chooser held for `tab`; `false` if none is.
    pub(crate) fn answer_chooser(&self, tab: &TabId, files: Option<Vec<PathBuf>>) -> bool {
        if self
            .state
            .borrow_mut()
            .dialogs
            .chooser
            .remove(tab)
            .is_none()
        {
            return false;
        }
        if let Some(page) = self.host.page(tab) {
            page.answer_file_chooser(files);
        }
        true
    }

    /// Arm `files` for the tab's next chooser; `None` disarms. Says whether
    /// some were armed before.
    pub(crate) fn arm_upload(&self, tab: &TabId, files: Option<Vec<PathBuf>>) -> bool {
        let mut state = self.state.borrow_mut();
        match files {
            Some(files) => state
                .dialogs
                .uploads
                .insert(tab.clone(), (files, Instant::now()))
                .is_some(),
            None => state.dialogs.uploads.remove(tab).is_some(),
        }
    }

    /// Around every agent call (`FileChooser.began` / `ended`).
    pub(crate) fn began(&self, tab: Option<&TabId>) {
        if let Some(tab) = tab {
            *self
                .state
                .borrow_mut()
                .dialogs
                .busy
                .entry(tab.clone())
                .or_default() += 1;
        }
    }

    pub(crate) fn ended(&self, tab: Option<&TabId>) {
        let Some(tab) = tab else { return };
        let mut state = self.state.borrow_mut();
        if let Some(n) = state.dialogs.busy.get_mut(tab) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                state.dialogs.busy.remove(tab);
            }
        }
    }

    /// Told on the answer of whichever call left a chooser open.
    pub(crate) fn chooser_note(&self, tab: Option<&TabId>, name: &str) -> Vec<Content> {
        match tab {
            Some(tab)
                if name != "browser_file_upload"
                    && self.state.borrow().dialogs.chooser.contains_key(tab) =>
            {
                vec![Content::text(
                    "### File chooser\nThe page opened a file chooser — call browser_file_upload with paths (absolute) to choose files, or with none to cancel.",
                )]
            }
            _ => Vec::new(),
        }
    }
}

/// A page hook's way back to the server; a server that is gone passes.
fn asked<H: Host, T>(
    me: &Weak<Mcp<H>>,
    _tab: &TabId,
    f: impl FnOnce(&Mcp<H>) -> Decision<T>,
) -> Decision<T> {
    match me.upgrade() {
        Some(m) => f(&m),
        None => Decision::Pass,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_answers_and_quotes() {
        assert_eq!(closed(DialogKind::Alert), (true, "OK"));
        assert_eq!(closed(DialogKind::Confirm), (false, "dismissed"));
        assert_eq!(closed(DialogKind::Prompt), (false, "dismissed"));
        assert_eq!(quoted("Delete \"x\"?"), r#""Delete \"x\"?""#);
        let long = "a".repeat(400);
        assert_eq!(quoted(&long).chars().count(), 302);
        assert!(quoted(&long).ends_with("…\""));
    }

    #[test]
    fn a_tab_keeps_its_last_twenty_lines() {
        let mut book = Book::default();
        let tab = TabId("t".into());
        for i in 0..25 {
            book.note(&tab, DialogKind::Alert, &format!("m{i}"), "OK");
        }
        let lines = &book.asked[&tab];
        assert_eq!(lines.len(), 20);
        assert_eq!(lines[0], "- alert \"m5\" → OK");
    }
}
