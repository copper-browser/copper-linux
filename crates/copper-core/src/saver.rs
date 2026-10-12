//! The debounced saver: one background thread that writes snapshots.
//!
//! The shell hands it an owned snapshot whenever state changes
//! ([`Saver::schedule`]); the newest snapshot per file wins, and each file is
//! written at most [`DEFAULT_DELAY`] after its first unsaved change, off the
//! GTK main thread. [`Saver::flush`] writes everything pending now and waits
//! (call it on quit — a session handed to a background queue on the way out
//! may never reach the disk). Dropping the saver flushes too.

use crate::session::SessionShape;
use crate::store::{DataFile, FileModel, Store, StoreError};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long after the first unsaved change a file is written.
pub const DEFAULT_DELAY: Duration = Duration::from_millis(500);

type Job = Box<dyn FnOnce(&Store) -> Result<(), StoreError> + Send>;

enum Msg {
    Job(DataFile, Job),
    Flush(Sender<()>),
    Stop,
}

/// Handle to the saver thread. Cheap to share behind an `Arc`; all methods
/// take `&self`.
pub struct Saver {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
    last_error: Arc<Mutex<Option<String>>>,
}

impl Saver {
    /// A saver writing through `store` with the 500 ms delay.
    pub fn new(store: Store) -> Self {
        Self::with_delay(store, DEFAULT_DELAY)
    }

    /// A saver with a custom delay (tests).
    pub fn with_delay(store: Store, delay: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let last_error = Arc::new(Mutex::new(None));
        let errors = last_error.clone();
        let thread = std::thread::Builder::new()
            .name("copper-saver".into())
            .spawn(move || run(store, rx, delay, errors))
            .expect("spawn saver thread");
        Saver {
            tx,
            thread: Some(thread),
            last_error,
        }
    }

    /// Write `value` to its file soon. A later snapshot of the same file
    /// replaces this one if it has not been written yet.
    pub fn schedule<T: FileModel + Send + 'static>(&self, value: T) {
        let job: Job = Box::new(move |store: &Store| store.save(&value));
        let _ = self.tx.send(Msg::Job(T::FILE, job));
    }

    /// Write the session through the session guard ([`Store::save_session`]).
    pub fn schedule_session(&self, shape: SessionShape, restored: bool) {
        let job: Job =
            Box::new(move |store: &Store| store.save_session(&shape, restored).map(|_| ()));
        let _ = self.tx.send(Msg::Job(DataFile::Session, job));
    }

    /// Write an arbitrary job under a file's slot (the newest per slot wins).
    pub fn schedule_with(
        &self,
        file: DataFile,
        job: impl FnOnce(&Store) -> Result<(), StoreError> + Send + 'static,
    ) {
        let _ = self.tx.send(Msg::Job(file, Box::new(job)));
    }

    /// Write everything pending now and wait for it.
    pub fn flush(&self) {
        let (done_tx, done_rx) = mpsc::channel();
        if self.tx.send(Msg::Flush(done_tx)).is_ok() {
            let _ = done_rx.recv();
        }
    }

    /// The last write error, if any (also logged).
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(store: Store, rx: Receiver<Msg>, delay: Duration, errors: Arc<Mutex<Option<String>>>) {
    let mut pending: HashMap<DataFile, (Instant, Job)> = HashMap::new();
    let write = |file: DataFile, job: Job| {
        if let Err(e) = job(&store) {
            tracing::error!(file = ?file, error = %e, "save failed");
            if let Ok(mut slot) = errors.lock() {
                *slot = Some(e.to_string());
            }
        }
    };
    loop {
        let next = pending.values().map(|(due, _)| *due).min();
        let msg = match next {
            Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => Some(Msg::Stop),
            },
            None => Some(rx.recv().unwrap_or(Msg::Stop)),
        };
        match msg {
            Some(Msg::Job(file, job)) => {
                let due = pending
                    .get(&file)
                    .map(|(d, _)| *d)
                    .unwrap_or_else(|| Instant::now() + delay);
                pending.insert(file, (due, job));
            }
            Some(Msg::Flush(done)) => {
                let mut all: Vec<_> = pending.drain().collect();
                all.sort_by_key(|(f, _)| *f);
                for (file, (_, job)) in all {
                    write(file, job);
                }
                let _ = done.send(());
            }
            Some(Msg::Stop) => {
                for (file, (_, job)) in pending.drain() {
                    write(file, job);
                }
                return;
            }
            None => {}
        }
        let now = Instant::now();
        let due: Vec<DataFile> = pending
            .iter()
            .filter(|(_, (d, _))| *d <= now)
            .map(|(f, _)| *f)
            .collect();
        for file in due {
            if let Some((_, job)) = pending.remove(&file) {
                write(file, job);
            }
        }
    }
}
