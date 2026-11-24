//! `browser_handoff`'s model (Mac `Handoff.swift`): the steps agents have
//! handed to the person, one ask per tab, oldest first, and the calls
//! waiting on them. Nothing here moves a tab: the app draws [`bar`] at the
//! bottom of the window, and its Show / Done / Not now call
//! [`crate::shell::Shell::select`] and [`Handoff::answer`]. [`follow`] says
//! when to draw it again.

use super::Agents;
use crate::Platform;
use crate::main_loop::{self, MainLoop};
use crate::shell::Shell;
use copper_core::TabId;
use copper_mcp::HandoffAnswer;
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;
use tokio::sync::oneshot;

/// One step handed to the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub id: u64,
    pub tab: TabId,
    pub agent: String,
    pub message: String,
}

/// What is waiting on the person, and who waits for the answer.
#[derive(Default)]
pub struct Handoff {
    /// Oldest first; one per tab.
    asks: RefCell<Vec<Ask>>,
    /// Calls waiting: (ask id, where the answer goes).
    waiters: RefCell<Vec<(u64, oneshot::Sender<HandoffAnswer>)>>,
    next: Cell<u64>,
    watchers: RefCell<Vec<Rc<dyn Fn()>>>,
}

impl Handoff {
    /// The ask open on `tab` (its message brought up to date), else a new
    /// one. Its id, and whether it is new.
    pub fn ask(&self, tab: &TabId, agent: &str, message: &str) -> (u64, bool) {
        let open = self
            .asks
            .borrow_mut()
            .iter_mut()
            .find(|a| &a.tab == tab)
            .map(|a| {
                a.message = message.to_string();
                a.id
            });
        let out = match open {
            Some(id) => (id, false),
            None => {
                let id = self.next.get() + 1;
                self.next.set(id);
                self.asks.borrow_mut().push(Ask {
                    id,
                    tab: tab.clone(),
                    agent: agent.to_string(),
                    message: message.to_string(),
                });
                (id, true)
            }
        };
        self.changed();
        out
    }

    /// The person's answer to ask `id`, or `None` once `timeout` has passed
    /// (the ask stays up, so the agent can call again).
    pub fn wait<L: MainLoop>(
        &self,
        id: u64,
        timeout: Duration,
    ) -> impl Future<Output = Option<HandoffAnswer>> + 'static {
        let (tx, rx) = oneshot::channel();
        let mut waiters = self.waiters.borrow_mut();
        // Calls that timed out left their senders behind.
        waiters.retain(|(_, w)| !w.is_closed());
        waiters.push((id, tx));
        async move { main_loop::timeout::<L, _>(timeout, rx).await?.ok() }
    }

    /// Done or Not now: the ask goes, and every call waiting on it answers.
    pub fn answer(&self, id: u64, answer: HandoffAnswer) {
        let before = self.asks.borrow().len();
        self.asks.borrow_mut().retain(|a| a.id != id);
        let waiting: Vec<_> = {
            let mut waiters = self.waiters.borrow_mut();
            let (on, rest) = std::mem::take(&mut *waiters)
                .into_iter()
                .partition(|(ask, _)| *ask == id);
            *waiters = rest;
            on
        };
        for (_, w) in waiting {
            let _ = w.send(answer);
        }
        if self.asks.borrow().len() != before {
            self.changed();
        }
    }

    /// Every open ask, oldest first.
    pub fn asks(&self) -> Vec<Ask> {
        self.asks.borrow().clone()
    }

    /// The ask a window shows: the oldest on a tab it holds (`holds`).
    /// Gone tabs ask nothing.
    pub fn first(&self, holds: impl Fn(&TabId) -> bool) -> Option<Ask> {
        self.asks.borrow().iter().find(|a| holds(&a.tab)).cloned()
    }

    /// Call `f` whenever an ask comes, changes or goes.
    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.watchers.borrow_mut().push(Rc::new(f));
    }

    fn changed(&self) {
        let watchers = self.watchers.borrow().clone();
        for f in watchers {
            f();
        }
    }
}

/// What the bar at the bottom of the window shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub ask: Ask,
    /// The tab is not the one on screen: the bar names it and offers Show.
    pub elsewhere: bool,
    /// The tab's title, else its host, else "a tab".
    pub title: String,
}

impl Bar {
    /// The bold part: "<agent> needs you".
    pub fn lead(&self) -> String {
        format!("{} needs you", self.ask.agent)
    }

    /// The rest: " on <tab>: <message>", or ": <message>" when it is on screen.
    pub fn rest(&self) -> String {
        if self.elsewhere {
            format!(" on {}: {}", self.title, self.ask.message)
        } else {
            format!(": {}", self.ask.message)
        }
    }

    /// For screen readers.
    pub fn label(&self) -> String {
        format!("{} needs you: {}", self.ask.agent, self.ask.message)
    }
}

/// The bar this window shows now, if any.
pub fn bar<P: Platform>(shell: &Shell<P>) -> Option<Bar> {
    let handoff = super::get::<P>()?.handoff();
    let browser = shell.browser();
    let ask = handoff.first(|t| browser.tab(t).is_some())?;
    let tab = browser.tab(&ask.tab)?;
    let title = if tab.title.is_empty() {
        url::Url::parse(&tab.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "a tab".into())
    } else {
        tab.title.clone()
    };
    let elsewhere = browser.active_id().as_ref() != Some(&ask.tab);
    Some(Bar {
        ask,
        elsewhere,
        title,
    })
}

/// Call `draw` whenever the bar may need drawing again: an ask came,
/// changed or went, or the tabs moved (selected, renamed, closed, another
/// space). For the life of the window.
pub fn follow<P: Platform>(
    shell: &Shell<P>,
    agents: &Agents<P>,
    draw: impl Fn(&Shell<P>) + 'static,
) {
    let draw = Rc::new(draw);
    let weak = shell.downgrade();
    let d = draw.clone();
    agents.handoff().on_change(move || {
        if let Some(s) = weak.upgrade() {
            d(&s);
        }
    });
    let mut rx = shell.hub().subscribe();
    let weak = shell.downgrade();
    P::Loop::spawn_local(async move {
        use crate::control::ShellEvent as E;
        use tokio::sync::broadcast::error::RecvError;
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            };
            let Some(s) = weak.upgrade() else { break };
            if matches!(
                ev,
                E::TabClosed(_) | E::TabChanged(_) | E::Selected(_) | E::SpaceSwitched(_)
            ) {
                draw(&s);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::main_loop::testing::TestLoop;

    fn tab(n: u8) -> TabId {
        TabId(format!("0000000{n}-AAAA"))
    }

    /// Wait on `id` in a task; the answer lands in the cell.
    fn waiting(h: &Handoff, id: u64, secs: u64) -> Rc<RefCell<Option<Option<HandoffAnswer>>>> {
        let out: Rc<RefCell<Option<Option<HandoffAnswer>>>> = Rc::default();
        let o = out.clone();
        let wait = h.wait::<TestLoop>(id, Duration::from_secs(secs));
        TestLoop::spawn_local(async move {
            *o.borrow_mut() = Some(wait.await);
        });
        TestLoop::run_until_idle();
        out
    }

    #[test]
    fn one_ask_per_tab_and_a_second_call_updates_its_message() {
        let h = Handoff::default();
        let changes = Rc::new(Cell::new(0));
        let c = changes.clone();
        h.on_change(move || c.set(c.get() + 1));
        let (a, new) = h.ask(&tab(1), "Claude Code", "Solve the check");
        assert!(new);
        let (b, new) = h.ask(&tab(2), "phi", "Pay");
        assert!(new);
        let (again, new) = h.ask(&tab(1), "Claude Code", "Solve the other check");
        assert!(!new);
        assert_eq!(again, a);
        let asks = h.asks();
        assert_eq!(asks.len(), 2);
        assert_eq!(asks[0].message, "Solve the other check", "oldest first");
        assert_eq!(asks[1].id, b);
        assert_eq!(changes.get(), 3);
    }

    #[test]
    fn an_answer_wakes_every_call_waiting_on_that_ask_only() {
        let h = Handoff::default();
        let (a, _) = h.ask(&tab(1), "Claude Code", "Solve the check");
        let (b, _) = h.ask(&tab(2), "phi", "Pay");
        let first = waiting(&h, a, 15);
        let second = waiting(&h, a, 600);
        let other = waiting(&h, b, 15);
        h.answer(a, HandoffAnswer::Done);
        TestLoop::run_until_idle();
        assert_eq!(*first.borrow(), Some(Some(HandoffAnswer::Done)));
        assert_eq!(*second.borrow(), Some(Some(HandoffAnswer::Done)));
        assert_eq!(*other.borrow(), None, "still waiting");
        assert_eq!(h.asks().len(), 1);
        h.answer(b, HandoffAnswer::NotNow);
        TestLoop::run_until_idle();
        assert_eq!(*other.borrow(), Some(Some(HandoffAnswer::NotNow)));
        assert!(h.asks().is_empty());
    }

    #[test]
    fn a_timeout_answers_nothing_and_leaves_the_ask_up() {
        let h = Handoff::default();
        let (a, _) = h.ask(&tab(1), "Claude Code", "Solve the check");
        let out = waiting(&h, a, 3);
        TestLoop::advance(Duration::from_secs(2));
        assert_eq!(*out.borrow(), None);
        TestLoop::advance(Duration::from_secs(1));
        assert_eq!(*out.borrow(), Some(None));
        assert_eq!(h.asks().len(), 1, "the bar is still up");
        // Called again, it waits on the same ask, and Done reaches it.
        let (again, new) = h.ask(&tab(1), "Claude Code", "Solve the check");
        assert_eq!((again, new), (a, false));
        let out = waiting(&h, a, 15);
        h.answer(a, HandoffAnswer::Done);
        TestLoop::run_until_idle();
        assert_eq!(*out.borrow(), Some(Some(HandoffAnswer::Done)));
        assert_eq!(h.waiters.borrow().len(), 0, "timed-out waiters are let go");
    }

    #[test]
    fn gone_tabs_ask_nothing() {
        let h = Handoff::default();
        h.ask(&tab(1), "Claude Code", "Solve the check");
        let (b, _) = h.ask(&tab(2), "phi", "Pay");
        let first = h.first(|t| t != &tab(1)).unwrap();
        assert_eq!(first.id, b);
        assert_eq!(h.first(|_| false), None);
    }

    #[test]
    fn the_bar_names_the_tab_only_when_it_is_elsewhere() {
        let ask = Ask {
            id: 1,
            tab: tab(1),
            agent: "Claude Code".into(),
            message: "Solve the check".into(),
        };
        let mut bar = Bar {
            ask,
            elsewhere: true,
            title: "Example Domain".into(),
        };
        assert_eq!(bar.lead(), "Claude Code needs you");
        assert_eq!(bar.rest(), " on Example Domain: Solve the check");
        assert_eq!(bar.label(), "Claude Code needs you: Solve the check");
        bar.elsewhere = false;
        assert_eq!(bar.rest(), ": Solve the check");
    }
}
