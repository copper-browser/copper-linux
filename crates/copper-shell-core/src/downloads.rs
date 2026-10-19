//! Downloads: every profile saves to the download folder (`~/Downloads`, or
//! Settings' `downloads`) without asking, and every engine download event
//! feeds one [`copper_core::downloads::Downloads`] model — this session's
//! live rows on top of `downloads.json`, the files kept from earlier
//! sessions (Mac: `Fork/Downloads.swift`, `Loot.swift`, `docs/downloads.md`).
//!
//! The Downloads panel (`Ctrl+Shift+Y`, `copper-app`'s `features::panels`)
//! reads the model and hears about every change through
//! [`Downloads::watch`]; raw engine events are still offered to
//! [`Downloads::subscribe`]rs.
//!
//! Quiet by design: "Downloading x" when a file starts; "x is in Downloads"
//! or "x failed" only when neither the panel nor the door is there to say it.

use crate::main_loop::MainLoop;
use crate::shell::Shell;
use crate::shell::host_of;
use crate::shell::pages::profile_key;
use crate::view::ShellView;
use crate::{Platform, ProfileOf};
use copper_core::downloads::{
    DownloadId as RowId, DownloadState as RowState, Downloads as Model, Keep,
};
use copper_engine::{DownloadEvent, DownloadId, DownloadState, Engine, Profile, ProfileId};
use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

type Listener = Rc<dyn Fn(&str, &DownloadEvent)>;
type Watcher = Rc<dyn Fn()>;

/// The engine side of one row: which download, in which profile, from where.
/// Kept while the row can still act (cancel while running, retry when failed).
struct Entry<P: Platform> {
    row: RowId,
    engine: DownloadId,
    profile: ProfileOf<P>,
    url: String,
    from: String,
    path: Option<PathBuf>,
}

/// Every profile's downloads, as one list.
pub struct Downloads<P: Platform> {
    /// Profiles listened to, by [`profile_key`].
    watched: RefCell<HashMap<String, (ProfileId, bool)>>,
    listeners: RefCell<Vec<Listener>>,
    watchers: RefCell<Vec<Watcher>>,
    model: RefCell<Model>,
    loaded: Cell<bool>,
    entries: RefCell<Vec<Entry<P>>>,
    /// The folder chosen this session (Settings › General), if changed.
    dir: RefCell<Option<PathBuf>>,
}

impl<P: Platform> Default for Downloads<P> {
    fn default() -> Self {
        Downloads {
            watched: RefCell::default(),
            listeners: RefCell::default(),
            watchers: RefCell::default(),
            model: RefCell::default(),
            loaded: Cell::default(),
            entries: RefCell::default(),
            dir: RefCell::default(),
        }
    }
}

impl<P: Platform> Downloads<P> {
    /// Hear every raw download event from now on: `(profile key, event)`.
    pub fn subscribe(&self, f: impl Fn(&str, &DownloadEvent) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    /// Hear that the list changed (a row came, moved on, or went).
    pub fn watch(&self, f: impl Fn() + 'static) {
        self.watchers.borrow_mut().push(Rc::new(f));
    }

    /// The list: this session's rows ([`Model::live`]) and the files kept
    /// from before ([`Model::kept`]). Never hold it across a call back in.
    pub fn model(&self, shell: &Shell<P>) -> Ref<'_, Model> {
        self.ensure_loaded(shell);
        self.model.borrow()
    }

    /// Read `downloads.json` once.
    fn ensure_loaded(&self, shell: &Shell<P>) {
        if self.loaded.replace(true) {
            return;
        }
        match shell.store().load::<Vec<Keep>>() {
            Ok(l) => {
                let live = std::mem::take(&mut *self.model.borrow_mut());
                let mut model = Model::new(l.value);
                // Rows that arrived before the file was read (none in practice).
                for d in live.live().iter().rev() {
                    let id = model.start(&d.name, &d.source_url, d.total);
                    if let Some(p) = &d.path {
                        model.set_destination(id, p, None);
                    }
                }
                *self.model.borrow_mut() = model;
            }
            Err(e) => tracing::warn!(error = %e, "downloads.json"),
        }
    }

    /// Where new downloads go.
    pub fn folder(&self, shell: &Shell<P>) -> PathBuf {
        self.dir
            .borrow()
            .clone()
            .unwrap_or_else(|| shell.engine().paths().downloads_dir.clone())
    }

    /// Save to `dir` from now on, in every profile.
    pub fn set_folder(&self, shell: &Shell<P>, dir: &Path) {
        self.dir.replace(Some(dir.to_path_buf()));
        let profiles: Vec<(ProfileId, bool)> = self.watched.borrow().values().cloned().collect();
        for (id, ephemeral) in profiles {
            if let Ok(p) = shell.engine().profile(&id, ephemeral) {
                p.set_download_dir(dir.to_path_buf());
            }
        }
    }

    /// Start listening to `profile`'s downloads (idempotent).
    pub fn attach(&self, shell: &Shell<P>, profile: &ProfileOf<P>) {
        self.ensure_loaded(shell);
        let key = profile_key(profile);
        if self.watched.borrow().contains_key(&key) {
            return;
        }
        self.watched
            .borrow_mut()
            .insert(key.clone(), (profile.id().clone(), profile.is_ephemeral()));
        if let Some(dir) = self.dir.borrow().clone() {
            profile.set_download_dir(dir);
        }
        let mut stream = profile.downloads();
        let weak = shell.downgrade();
        P::Loop::spawn_local(async move {
            while let Some(ev) = stream.next().await {
                let Some(shell) = weak.upgrade() else { break };
                shell.downloads().dispatch(&shell, &key, &ev);
            }
            if let Some(shell) = weak.upgrade() {
                shell.downloads().watched.borrow_mut().remove(&key);
            }
        });
    }

    fn dispatch(&self, shell: &Shell<P>, key: &str, ev: &DownloadEvent) {
        let listeners: Vec<Listener> = self.listeners.borrow().clone();
        for l in &listeners {
            l(key, ev);
        }
        self.apply(shell, key, ev);
        self.changed();
    }

    fn row_of(&self, engine: DownloadId) -> Option<RowId> {
        self.entries
            .borrow()
            .iter()
            .find(|e| e.engine == engine)
            .map(|e| e.row)
    }

    fn apply(&self, shell: &Shell<P>, key: &str, ev: &DownloadEvent) {
        match &ev.state {
            DownloadState::Started { url } => {
                if self.row_of(ev.id).is_some() {
                    return;
                }
                let url = url.clone().unwrap_or_default();
                let Some(profile) = self.profile_for(shell, key) else {
                    return;
                };
                let row = self
                    .model
                    .borrow_mut()
                    .start(&name_from_url(&url), &url, None);
                self.entries.borrow_mut().push(Entry {
                    row,
                    engine: ev.id,
                    profile,
                    from: host_of(&url),
                    url,
                    path: None,
                });
            }
            DownloadState::Destination { path, total, .. } => {
                let Some(row) = self.row_of(ev.id) else {
                    return;
                };
                let name = file_name(path);
                {
                    let mut m = self.model.borrow_mut();
                    m.set_destination(row, &path.to_string_lossy(), Some(&name));
                    if total.is_some() {
                        m.progress(row, 0, *total);
                    }
                }
                if let Some(e) = self.entries.borrow_mut().iter_mut().find(|e| e.row == row) {
                    e.path = Some(path.clone());
                }
                // Always: it is how you learn something started.
                shell.toast(&format!("Downloading {name}"));
            }
            DownloadState::Progress {
                received, total, ..
            } => {
                if let Some(row) = self.row_of(ev.id) {
                    self.model.borrow_mut().progress(row, *received, *total);
                }
            }
            DownloadState::Finished { path, bytes } => {
                let Some(row) = self.row_of(ev.id) else {
                    return;
                };
                let entry = {
                    let mut entries = self.entries.borrow_mut();
                    let at = entries.iter().position(|e| e.row == row);
                    at.map(|i| entries.remove(i))
                };
                let from = entry.as_ref().map(|e| e.from.clone()).unwrap_or_default();
                let name = file_name(path);
                {
                    let mut m = self.model.borrow_mut();
                    if !path.as_os_str().is_empty() {
                        m.set_destination(row, &path.to_string_lossy(), Some(&name));
                    }
                    let total = m.get(row).and_then(|d| d.total);
                    m.progress(row, *bytes, total.or(Some(*bytes)));
                    m.finish(row, &from);
                }
                self.save(shell);
                if !shell.view().showing_downloads() {
                    shell.toast(&format!(
                        "{name} is in {}",
                        folder_name(&self.folder(shell))
                    ));
                }
            }
            DownloadState::Failed { error, cancelled } => {
                let Some(row) = self.row_of(ev.id) else {
                    return;
                };
                let path = self
                    .entries
                    .borrow()
                    .iter()
                    .find(|e| e.row == row)
                    .and_then(|e| e.path.clone());
                if let Some(p) = path {
                    remove_partial(&p);
                }
                if *cancelled {
                    self.model.borrow_mut().cancel(row);
                    self.entries.borrow_mut().retain(|e| e.row != row);
                } else {
                    tracing::warn!(%error, "download failed");
                    self.model.borrow_mut().fail(row, &sentence(error));
                    if !shell.view().showing_downloads() {
                        let name = self.model.borrow().get(row).map(|d| d.name.clone());
                        shell.toast(&format!(
                            "{} failed",
                            name.unwrap_or_else(|| "A download".into())
                        ));
                    }
                }
            }
        }
    }

    /// The profile a key names, alive while one of its pages is.
    fn profile_for(&self, shell: &Shell<P>, key: &str) -> Option<ProfileOf<P>> {
        let (id, ephemeral) = self.watched.borrow().get(key).cloned()?;
        shell.engine().profile(&id, ephemeral).ok()
    }

    fn changed(&self) {
        let watchers: Vec<Watcher> = self.watchers.borrow().clone();
        for w in &watchers {
            w();
        }
    }

    fn save(&self, shell: &Shell<P>) {
        let kept = self.model.borrow().kept().to_vec();
        shell.saver().schedule(kept);
    }

    /// Start downloading `url` in the selected tab's profile (or the
    /// default one).
    pub fn start(&self, shell: &Shell<P>, url: &str) -> bool {
        use copper_engine::PageView;
        let started = match shell.active_page() {
            Some(page) => page.download(url),
            None => match shell
                .engine()
                .profile(&crate::shell::pages::profile_id(None), false)
            {
                Ok(p) => {
                    self.attach(shell, &p);
                    p.download(url)
                }
                Err(e) => Err(e),
            },
        };
        if let Err(e) = &started {
            tracing::warn!(error = %e, "download");
        }
        started.is_ok()
    }

    /// × on a running row: stop it (its partial file goes with it).
    pub fn cancel(&self, row: RowId) {
        let target = self
            .entries
            .borrow()
            .iter()
            .find(|e| e.row == row)
            .map(|e| (e.profile.clone(), e.engine));
        if let Some((profile, engine)) = target
            && !profile.cancel_download(engine)
        {
            // Already over on the engine side: just drop the row.
            self.model.borrow_mut().cancel(row);
            self.entries.borrow_mut().retain(|e| e.row != row);
            self.changed();
        }
    }

    /// Retry on a failed row: start over from the original address; the
    /// failed row goes as the new one arrives.
    pub fn retry(&self, shell: &Shell<P>, row: RowId) -> bool {
        let failed = matches!(
            self.model.borrow().get(row).map(|d| &d.state),
            Some(RowState::Failed { .. })
        );
        if !failed {
            return false;
        }
        let Some((profile, url)) = self
            .entries
            .borrow()
            .iter()
            .find(|e| e.row == row)
            .map(|e| (e.profile.clone(), e.url.clone()))
        else {
            return false;
        };
        match profile.download(&url) {
            Ok(_) => {
                self.model.borrow_mut().cancel(row);
                self.entries.borrow_mut().retain(|e| e.row != row);
                self.changed();
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "retry");
                shell.toast("Couldn’t start it again");
                false
            }
        }
    }

    /// × on a finished or failed row: off the list (never the file).
    pub fn remove(&self, row: RowId) {
        self.model.borrow_mut().cancel(row);
        self.entries.borrow_mut().retain(|e| e.row != row);
        self.changed();
    }

    /// × on a file kept from before: off the list (never the file).
    pub fn forget(&self, shell: &Shell<P>, path: &str) {
        self.model.borrow_mut().forget(path);
        self.save(shell);
        self.changed();
    }

    /// **Clear list**: finished and failed rows and the kept files go from
    /// the list; running downloads stay; no file is touched.
    pub fn clear(&self, shell: &Shell<P>) {
        {
            let mut m = self.model.borrow_mut();
            m.clear_finished();
            m.forget_all();
        }
        let live: Vec<RowId> = self.model.borrow().live().iter().map(|d| d.id).collect();
        self.entries.borrow_mut().retain(|e| live.contains(&e.row));
        self.save(shell);
        self.changed();
    }

    /// The panel was opened: nothing is unseen any more.
    pub fn seen(&self) {
        if self.model.borrow().live().iter().any(|d| d.unseen) {
            self.model.borrow_mut().mark_seen();
        }
    }
}

/// The last path segment of an address, or `download`.
fn name_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.path_segments()
                .and_then(|mut s| s.next_back().map(str::to_string))
        })
        .filter(|s| !s.is_empty())
        .map(|s| {
            url::form_urlencoded::parse(format!("x={s}").as_bytes())
                .next()
                .map(|(_, v)| v.to_string())
                .unwrap_or(s)
        })
        .unwrap_or_else(|| "download".into())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".into())
}

fn folder_name(dir: &Path) -> String {
    file_name(dir)
}

/// The partial file a failed download leaves (WebKit's intermediate file,
/// or the destination itself).
fn remove_partial(path: &Path) {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".wkdownload");
    for p in [PathBuf::from(tmp), path.to_path_buf()] {
        if p.is_file() {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// WebKit's error text as one sentence.
fn sentence(error: &str) -> String {
    let e = error.trim().trim_end_matches('.');
    if e.is_empty() {
        return "It stopped".into();
    }
    let mut c = e.chars();
    let first = c
        .next()
        .map(|f| f.to_uppercase().collect::<String>())
        .unwrap_or_default();
    format!("{first}{}", c.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_addresses() {
        assert_eq!(name_from_url("http://127.0.0.1:8766/big?mb=20"), "big");
        assert_eq!(name_from_url("https://x.test/a%20b.pdf"), "a b.pdf");
        assert_eq!(name_from_url("https://x.test/"), "download");
        assert_eq!(name_from_url("nonsense"), "download");
    }

    #[test]
    fn partial_files_go() {
        let dir = std::env::temp_dir().join(format!("copper-dl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.bin");
        std::fs::write(&p, b"x").unwrap();
        std::fs::write(dir.join("a.bin.wkdownload"), b"x").unwrap();
        remove_partial(&p);
        assert!(!p.exists());
        assert!(!dir.join("a.bin.wkdownload").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
