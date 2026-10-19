//! `copper.oauth.authorize`: a small sign-in window (Mirrors the Mac's
//! `Fork/Addons/AddonBridge.swift`, `oauthAuthorize`).
//!
//! The window uses the person's normal browsing profile (the current
//! space's), so the single sign-on session they already have applies. It is
//! not modal: the browser stays usable. Every navigation and redirect is
//! seen by WebKit's `decide-policy`; the first whose address starts with the
//! addon's `redirectPrefix` is cancelled (nothing ever loads it, and nothing
//! listens on the loopback port), the window closes, and the bridge call
//! resolves with that address. Closing the window is `cancelled`; running
//! past the deadline is `timeout`.

use super::policy::{self, Verdict};
use crate::shell::Shell;
use copper_engine::{Engine, PageEvent, PageOptions, PageView};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// How a sign-in ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The redirect, with its code.
    Redirect(String),
    /// The person closed the window.
    Cancelled,
    Timeout,
    /// The window could not be made.
    Failed(String),
}

/// For the log: never the redirect itself (it carries the code).
fn outcome_name(o: &Outcome) -> &'static str {
    match o {
        Outcome::Redirect(_) => "redirect",
        Outcome::Cancelled => "cancelled",
        Outcome::Timeout => "timeout",
        Outcome::Failed(_) => "failed",
    }
}

/// Who hears how a sign-in ended.
type Done = Box<dyn FnOnce(Outcome)>;

/// One open sign-in.
struct SignIn {
    window: adw::Window,
    done: RefCell<Option<Done>>,
    finished: Cell<bool>,
}

impl SignIn {
    /// Answer the addon once; close the window unless it is already closing.
    fn finish(&self, outcome: Outcome, close: bool) {
        if self.finished.replace(true) {
            return;
        }
        tracing::info!(outcome = outcome_name(&outcome), "addon sign-in finished");
        if let Some(done) = self.done.borrow_mut().take() {
            done(outcome);
        }
        if close {
            self.window.close();
        }
    }
}

/// Open the sign-in window for addon `name` at `url`; `done` gets the
/// outcome, once.
pub fn authorize(
    shell: &Shell,
    name: &str,
    url: &str,
    redirect_prefix: &str,
    timeout: Duration,
    done: impl FnOnce(Outcome) + 'static,
) {
    let space_profile = shell.browser().current_space().profile.clone();
    let profile = match shell.engine().profile(
        &crate::shell::pages::profile_id(space_profile.as_deref()),
        false,
    ) {
        Ok(p) => p,
        Err(e) => {
            done(Outcome::Failed(e.to_string()));
            return;
        }
    };
    let page = shell.engine().new_page(&profile, PageOptions::default());
    let web = page.widget().clone();
    web.set_hexpand(true);
    web.set_vexpand(true);

    let window = adw::Window::builder()
        .title(format!("Sign in — {name}"))
        .default_width(520)
        .default_height(680)
        .modal(false)
        .transient_for(shell.window())
        .destroy_with_parent(true)
        .build();
    let header = adw::HeaderBar::new();
    let title = adw::WindowTitle::new(&format!("Sign in — {name}"), "");
    header.set_title_widget(Some(&title));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&web));
    window.set_content(Some(&toolbar));

    let signin = Rc::new(SignIn {
        window: window.clone(),
        done: RefCell::new(Some(Box::new(done))),
        finished: Cell::new(false),
    });

    // Every navigation, redirects included.
    let prefix = redirect_prefix.to_string();
    let weak = Rc::downgrade(&signin);
    policy::on_decide(&web, move |nav| {
        if policy::redirect_matches(&nav.uri, &prefix) {
            if let Some(s) = weak.upgrade() {
                let uri = nav.uri.clone();
                // Not from inside WebKit's signal: finish on the next turn.
                glib::idle_add_local_once(move || s.finish(Outcome::Redirect(uri), true));
            }
            return Verdict::Refuse;
        }
        if nav.new_window {
            // A provider's popup stays in this window.
            return Verdict::Refuse;
        }
        Verdict::Default
    });

    // The host shows where the person is signing in.
    let mut events = page.events();
    let t = title.clone();
    glib::spawn_future_local(async move {
        while let Some(ev) = events.next().await {
            if let PageEvent::Url(Some(u)) = ev {
                let host = url::Url::parse(&u)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_default();
                t.set_subtitle(&host);
            }
        }
    });

    let weak = Rc::downgrade(&signin);
    window.connect_close_request(move |_| {
        if let Some(s) = weak.upgrade() {
            s.finish(Outcome::Cancelled, false);
        }
        glib::Propagation::Proceed
    });
    let weak = Rc::downgrade(&signin);
    glib::timeout_add_local_once(timeout, move || {
        if let Some(s) = weak.upgrade() {
            s.finish(Outcome::Timeout, true);
        }
    });
    // The sign-in lives as long as its window.
    let keep = RefCell::new(Some((signin.clone(), page.clone())));
    window.connect_destroy(move |_| {
        keep.borrow_mut().take();
    });

    page.load(url);
    window.present();
    tracing::info!(%name, "addon sign-in window open");
}

/// The open sign-in windows (the self-test closes them).
pub fn windows() -> Vec<gtk::Window> {
    gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
        .filter(|w| w.title().is_some_and(|t| t.starts_with("Sign in — ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_are_logged_without_the_code() {
        let o = Outcome::Redirect("http://127.0.0.1:53682/cb?code=secret".into());
        assert_eq!(outcome_name(&o), "redirect");
        assert_eq!(outcome_name(&Outcome::Cancelled), "cancelled");
        assert_eq!(outcome_name(&Outcome::Timeout), "timeout");
        assert_eq!(outcome_name(&Outcome::Failed("x".into())), "failed");
    }
}
