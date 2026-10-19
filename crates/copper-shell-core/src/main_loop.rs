//! The UI thread's event loop, as the few things the shell asks of it.
//!
//! The shell is single-threaded (`Rc`, engine pages are `!Send`): it lives
//! on the thread that runs the toolkit's main loop and does everything as
//! callbacks and `!Send` futures on that loop. [`MainLoop`] is what it needs
//! from the loop, small enough that each toolkit's implementation is a few
//! lines: GLib's default `MainContext` on Linux, `copper-win-rt`'s message
//! loop on Windows.
//!
//! On top of it this module builds what the shell uses everywhere: tasks it
//! can stop ([`spawn`] / [`Task`]) and a time limit on any future
//! ([`timeout`]).

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

/// The main loop of the UI thread. Every method but [`MainLoop::post`] is
/// called on that thread only.
///
/// Implementations are a marker type with static methods (the loop is a
/// per-thread singleton in every toolkit Copper runs on):
///
/// ```ignore
/// pub enum Glib {}
/// impl MainLoop for Glib {
///     fn spawn_local(future: impl Future<Output = ()> + 'static) {
///         glib::spawn_future_local(future);
///     }
///     fn sleep(duration: Duration) -> impl Future<Output = ()> + 'static {
///         glib::timeout_future(duration)
///     }
///     fn post(f: impl FnOnce() + Send + 'static) {
///         glib::MainContext::default().invoke(f);
///     }
///     fn is_main_thread() -> bool {
///         glib::MainContext::default().is_owner()
///     }
/// }
/// ```
pub trait MainLoop: 'static {
    /// Run `future` to completion on the main thread. It is first polled in
    /// a later turn of the loop, never inside the caller, so a caller may
    /// hold `RefCell` borrows across the call.
    fn spawn_local(future: impl Future<Output = ()> + 'static);

    /// A future that resolves once `duration` has passed, driven by the
    /// loop's timers (polled on the main thread).
    fn sleep(duration: Duration) -> impl Future<Output = ()> + 'static;

    /// Run `f` on the main thread; callable from any thread. `f` may run
    /// before `post` returns when the caller is already on the main thread
    /// (GLib's `invoke`), so `f` should only start work (spawn a task), not
    /// touch state the caller has borrowed.
    fn post(f: impl FnOnce() + Send + 'static);

    /// Whether the caller is on the main thread. Callable from any thread:
    /// code that may run on either side uses it to run in place on the
    /// main thread rather than [`MainLoop::post`] to itself and wait for
    /// an answer that could never come.
    fn is_main_thread() -> bool;

    /// Run `f` on the main thread once the loop has nothing more urgent to
    /// do (after pending input and drawing). The default is a fresh task,
    /// which every loop runs after the current callback returns.
    fn idle(f: impl FnOnce() + 'static) {
        Self::spawn_local(async move { f() });
    }
}

/// Run `work` on a thread of its own and hand its answer to whichever loop
/// awaits it (`None`: it panicked). For CPU work that must not stall the UI
/// thread — cropping and encoding pictures.
pub async fn spawn_blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("copper-work".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .ok()?;
    rx.await.ok()
}

/// A task started with [`spawn`]: it can be stopped from the outside.
///
/// Dropping a `Task` lets the task run on (like a detached thread); only
/// [`Task::abort`] stops it.
pub struct Task {
    state: Rc<TaskState>,
}

#[derive(Default)]
struct TaskState {
    stopped: Cell<bool>,
    waker: RefCell<Option<Waker>>,
}

impl Task {
    /// Stop the task: it is not polled again, and its future (with
    /// everything it holds) is dropped on the loop's next turn. Safe to
    /// call from inside the task itself and more than once.
    pub fn abort(&self) {
        self.state.stopped.set(true);
        let waker = self.state.waker.borrow_mut().take();
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Whether [`Task::abort`] was called.
    pub fn is_aborted(&self) -> bool {
        self.state.stopped.get()
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Task")
            .field("aborted", &self.is_aborted())
            .finish()
    }
}

/// Run `future` on `L`'s main loop (first polled in a later turn) and get a
/// handle that can stop it. A loop such as `copper-win-rt` hands out no
/// join handle of its own; this gives every loop the same one.
pub fn spawn<L: MainLoop>(future: impl Future<Output = ()> + 'static) -> Task {
    let state = Rc::new(TaskState::default());
    L::spawn_local(Stoppable {
        future: Box::pin(future),
        state: state.clone(),
    });
    Task { state }
}

/// The future [`spawn`] hands the loop: `future` until it is stopped.
struct Stoppable {
    future: Pin<Box<dyn Future<Output = ()>>>,
    state: Rc<TaskState>,
}

impl Future for Stoppable {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.state.stopped.get() {
            return Poll::Ready(());
        }
        // Kept so `abort` can get this task polled (and so dropped) even
        // while the future waits on something that will never come.
        self.state.waker.replace(Some(cx.waker().clone()));
        self.future.as_mut().poll(cx)
    }
}

/// `future`'s output, or `None` if `limit` passes first (the future is
/// then dropped).
pub async fn timeout<L: MainLoop, F: Future>(limit: Duration, future: F) -> Option<F::Output> {
    let mut future = std::pin::pin!(future);
    let mut timer = std::pin::pin!(L::sleep(limit));
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(out) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(out));
        }
        if timer.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

#[cfg(test)]
pub(crate) mod testing {
    //! A main loop for unit tests: a queue of tasks run by hand, and timers
    //! that fire when the test advances the clock.

    use super::MainLoop;
    use std::cell::{Cell, RefCell};
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::Duration;

    type Boxed = Pin<Box<dyn Future<Output = ()>>>;

    struct Slot {
        future: RefCell<Option<Boxed>>,
        woken: Arc<Flag>,
    }

    #[derive(Default)]
    struct Flag(AtomicBool);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    thread_local! {
        static TASKS: RefCell<Vec<Rc<Slot>>> = const { RefCell::new(Vec::new()) };
        static NOW: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    }

    /// The test loop (thread-local state: one per test thread).
    pub enum TestLoop {}

    impl TestLoop {
        /// Poll every woken task until none is left to run.
        pub fn run_until_idle() {
            loop {
                let ready: Vec<Rc<Slot>> = TASKS.with(|t| {
                    t.borrow()
                        .iter()
                        .filter(|s| s.woken.0.swap(false, Ordering::SeqCst))
                        .cloned()
                        .collect()
                });
                if ready.is_empty() {
                    break;
                }
                for slot in ready {
                    let waker = Waker::from(slot.woken.clone());
                    let mut cx = Context::from_waker(&waker);
                    let done = match slot.future.borrow_mut().as_mut() {
                        Some(f) => f.as_mut().poll(&mut cx).is_ready(),
                        None => true,
                    };
                    if done {
                        slot.future.replace(None);
                    }
                }
                TASKS.with(|t| t.borrow_mut().retain(|s| s.future.borrow().is_some()));
            }
        }

        /// Move the clock on by `by`, waking every task (sleepers check the
        /// clock), then run.
        pub fn advance(by: Duration) {
            NOW.with(|n| n.set(n.get() + by));
            TASKS.with(|t| {
                for s in t.borrow().iter() {
                    s.woken.0.store(true, Ordering::SeqCst);
                }
            });
            Self::run_until_idle();
        }

        /// Tasks not finished yet.
        pub fn pending() -> usize {
            TASKS.with(|t| t.borrow().len())
        }
    }

    impl MainLoop for TestLoop {
        fn spawn_local(future: impl Future<Output = ()> + 'static) {
            let woken = Arc::new(Flag::default());
            woken.0.store(true, Ordering::SeqCst);
            let slot = Rc::new(Slot {
                future: RefCell::new(Some(Box::pin(future))),
                woken,
            });
            TASKS.with(|t| t.borrow_mut().push(slot));
        }

        fn sleep(duration: Duration) -> impl Future<Output = ()> + 'static {
            let until = NOW.with(Cell::get) + duration;
            std::future::poll_fn(move |_| {
                if NOW.with(Cell::get) >= until {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
        }

        fn post(_f: impl FnOnce() + Send + 'static) {
            unreachable!("unit tests stay on one thread");
        }

        fn is_main_thread() -> bool {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TestLoop;
    use super::*;

    #[test]
    fn tasks_start_later_and_finish() {
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        let task = spawn::<TestLoop>(async move { h.set(h.get() + 1) });
        assert_eq!(hits.get(), 0, "never inside the caller");
        TestLoop::run_until_idle();
        assert_eq!(hits.get(), 1);
        assert!(!task.is_aborted());
        assert_eq!(TestLoop::pending(), 0);
    }

    #[test]
    fn aborted_tasks_stop_and_drop_their_future() {
        let hits = Rc::new(Cell::new(0));
        let h = hits.clone();
        let task = spawn::<TestLoop>(async move {
            loop {
                TestLoop::sleep(Duration::from_secs(1)).await;
                h.set(h.get() + 1);
            }
        });
        // First poll at t = 0: the first sleep runs until t = 1 s.
        TestLoop::run_until_idle();
        TestLoop::advance(Duration::from_secs(1));
        assert_eq!(hits.get(), 1);
        task.abort();
        TestLoop::advance(Duration::from_secs(5));
        assert_eq!(hits.get(), 1, "not polled again");
        assert_eq!(TestLoop::pending(), 0, "and dropped");
        assert_eq!(Rc::strong_count(&hits), 1);
    }

    #[test]
    fn a_task_can_abort_itself() {
        let cell: Rc<RefCell<Option<Task>>> = Rc::default();
        let hits = Rc::new(Cell::new(0));
        let (c, h) = (cell.clone(), hits.clone());
        let task = spawn::<TestLoop>(async move {
            loop {
                h.set(h.get() + 1);
                if let Some(t) = c.borrow().as_ref() {
                    t.abort();
                }
                TestLoop::sleep(Duration::from_secs(1)).await;
            }
        });
        cell.replace(Some(task));
        TestLoop::advance(Duration::from_secs(3));
        assert_eq!(hits.get(), 1);
        assert_eq!(TestLoop::pending(), 0);
    }

    #[test]
    fn timeouts() {
        let out: Rc<RefCell<Vec<Option<u8>>>> = Rc::default();
        let o = out.clone();
        TestLoop::spawn_local(async move {
            let fast = timeout::<TestLoop, _>(Duration::from_secs(5), async { 1 }).await;
            o.borrow_mut().push(fast);
            let slow = timeout::<TestLoop, _>(
                Duration::from_secs(1),
                TestLoop::sleep(Duration::from_secs(10)),
            )
            .await;
            o.borrow_mut().push(slow.map(|()| 2));
        });
        TestLoop::run_until_idle();
        assert_eq!(*out.borrow(), vec![Some(1)]);
        TestLoop::advance(Duration::from_secs(1));
        assert_eq!(*out.borrow(), vec![Some(1), None]);
    }
}
