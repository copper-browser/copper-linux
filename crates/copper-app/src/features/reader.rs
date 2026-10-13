//! Reading mode (Mac: `Reader.swift`, `Tab.toggleReader`): Ctrl+Alt+R
//! strips the page back to its article — the piece of the page with the
//! most prose, punished for every link in it — set calmly in one column.
//! Again (or navigating) leaves it, by reloading: restoring the old markup
//! would give a page that looks right and does nothing, every listener it
//! had thrown away with it.

use super::util::run;
use crate::shell::Shell;
use copper_core::TabId;
use copper_engine::{PageView, World};
use copper_pagejs::tab::ReaderOutcome;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    static READING: RefCell<HashSet<TabId>> = RefCell::default();
}

/// The type the Mac's sheet asks for, in the families a Linux desktop has
/// (only the sans-serif parts move; the body keeps its serif), and the
/// article's own `<h1>` dropped when it only repeats the title above it.
const LINUX_TYPE: &str = r#"(function () {
  var s = document.createElement('style');
  s.textContent = '#office-reader h1,#office-reader h2,#office-reader h3,' +
    '#office-reader .office-from,#office-reader figcaption{font-family:' +
    '"Inter","Cantarell","Noto Sans",system-ui,sans-serif !important}' +
    '#office-reader{font-family:"Noto Serif","Source Serif 4","DejaVu Serif",Georgia,serif !important}';
  document.head.appendChild(s);
  // The article's own heading, when it repeats the title just set above it.
  var r = document.getElementById('office-reader');
  var hs = r ? r.querySelectorAll('h1') : [];
  if (hs.length > 1 && hs[1].textContent.trim() === hs[0].textContent.trim()) hs[1].remove();
  return true;
})()"#;

/// Whether `tab` shows its article.
pub fn reading(tab: &TabId) -> bool {
    READING.with(|r| r.borrow().contains(tab))
}

/// The tab left reading mode (navigated, reloaded, closed).
pub fn forget(tab: &TabId) {
    READING.with(|r| r.borrow_mut().remove(tab));
}

/// Ctrl+Alt+R: the article, or back to the page.
pub fn toggle(shell: &Shell) {
    let Some(id) = shell.active_id() else { return };
    let url = shell
        .browser()
        .tab(&id)
        .map(|t| t.url.clone())
        .unwrap_or_default();
    if url.is_empty() || url.starts_with("about:") {
        shell.toast("Nothing to read on this page");
        return;
    }
    if reading(&id) {
        forget(&id);
        shell.reload_active();
        return;
    }
    let Some(page) = shell.ensure_page(&id) else {
        return;
    };
    let weak = shell.downgrade();
    glib::spawn_future_local(async move {
        let outcome = run(&page, &copper_pagejs::tab::reader()).await;
        let Some(shell) = weak.upgrade() else { return };
        match outcome {
            Ok(ReaderOutcome::Read) => {
                READING.with(|r| r.borrow_mut().insert(id.clone()));
                let _ = page.eval(LINUX_TYPE, &World::Page).await;
            }
            Ok(ReaderOutcome::None) => shell.toast("Nothing to read on this page"),
            Err(e) => {
                tracing::warn!(error = %e, "reading mode");
                shell.toast("Nothing to read on this page");
            }
        }
    });
}
