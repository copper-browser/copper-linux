//! Downloads, for now: every profile saves to the download folder
//! (`~/Downloads`, or Settings' `downloads`) without asking, and the window
//! says one line when a file lands.
//!
//! The boundary for the downloads lane (T-013): every [`DownloadEvent`] of
//! every profile passes through [`Downloads`]; a panel subscribes with
//! [`Downloads::subscribe`] and gets `(profile key, event)` for each,
//! without touching the shell.

use crate::shell::Shell;
use crate::shell::pages::profile_key;
use copper_engine::{DownloadEvent, DownloadState, Profile};
use copper_engine_webkitgtk::WebKitProfile;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

type Listener = Rc<dyn Fn(&str, &DownloadEvent)>;

/// Watches each profile's downloads once.
#[derive(Default)]
pub struct Downloads {
    watched: RefCell<HashSet<String>>,
    listeners: RefCell<Vec<Listener>>,
}

impl Downloads {
    /// Hear every download event from now on: `(profile key, event)`.
    pub fn subscribe(&self, f: impl Fn(&str, &DownloadEvent) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    /// Start listening to `profile`'s downloads (idempotent).
    pub fn attach(&self, shell: &Shell, profile: &WebKitProfile) {
        let key = profile_key(profile);
        if !self.watched.borrow_mut().insert(key.clone()) {
            return;
        }
        let mut stream = profile.downloads();
        let weak = shell.downgrade();
        glib::spawn_future_local(async move {
            while let Some(ev) = stream.next().await {
                let Some(shell) = weak.upgrade() else { break };
                shell.downloads().dispatch(&shell, &key, &ev);
            }
        });
    }

    fn dispatch(&self, shell: &Shell, key: &str, ev: &DownloadEvent) {
        let listeners: Vec<Listener> = self.listeners.borrow().clone();
        for l in &listeners {
            l(key, ev);
        }
        if !listeners.is_empty() {
            return;
        }
        // No panel yet: one line when it lands or fails.
        match &ev.state {
            DownloadState::Finished { path, .. } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "The file".into());
                shell.toast(&format!("{name} is in Downloads"));
            }
            DownloadState::Failed { cancelled: false, error } => {
                tracing::warn!(%error, "download failed");
                shell.toast("A download failed");
            }
            _ => {}
        }
    }
}
