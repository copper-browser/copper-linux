//! The forms lane's own Tokio runtime: the secret store (`oo7` on zbus,
//! Credential Manager) and the `bw` / `op` children are Tokio code, the
//! shell is the UI main loop. [`run`] hands a future to the runtime and awaits its answer on the
//! main loop (a oneshot is executor-neutral).

use std::future::Future;
use std::sync::OnceLock;

static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("copper-creds")
            .enable_all()
            .build()
            .expect("the passwords runtime starts")
    })
}

/// Run `f` on the passwords runtime; `None` only if it panicked.
pub async fn run<T: Send + 'static>(f: impl Future<Output = T> + Send + 'static) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    runtime().spawn(async move {
        let _ = tx.send(f.await);
    });
    rx.await.ok()
}

/// Run blocking work (file reads, SQLite) off the main loop.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    runtime().spawn_blocking(move || {
        let _ = tx.send(f());
    });
    rx.await.ok()
}
