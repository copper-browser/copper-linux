//! "Bring in from Chrome, Chromium or Brave" (Mac `Import.swift`,
//! `Fork/Flow/*`): pick a browser and a profile, choose what comes over —
//! bookmarks, history, open tabs (sleeping, in new spaces) and passwords
//! (into the keyring) — then a progress line and a summary.
//!
//! The other browser's files are copied before they are read and never
//! written to (copper-import).

use super::rt;
use crate::shell::GtkShell;
use crate::shell::Shell;
use crate::spaces::colour::SpaceColour;
use copper_core::OpenOptions;
use copper_import::{Browser, bookmarks, history, passwords, tabs};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// How many history entries come over at most.
const HISTORY_LIMIT: usize = 50_000;

/// What was chosen.
#[derive(Clone, Debug)]
pub struct Choice {
    pub browser: Browser,
    /// `None` = every profile.
    pub profile: Option<String>,
    pub bookmarks: bool,
    pub history: bool,
    pub tabs: bool,
    pub passwords: bool,
}

impl Choice {
    fn dirs(&self) -> Vec<PathBuf> {
        match &self.profile {
            Some(p) => vec![self.browser.root.join(p)],
            None => self.browser.profile_dirs(),
        }
    }
}

/// What came over.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub bookmarks: usize,
    pub history: usize,
    pub spaces: usize,
    pub tabs: usize,
    pub passwords: usize,
    pub never: usize,
    pub problems: Vec<String>,
}

impl Summary {
    pub fn lines(&self, c: &Choice) -> Vec<String> {
        let n =
            |v: usize, one: &str, many: &str| format!("{v} {}", if v == 1 { one } else { many });
        let mut out = Vec::new();
        if c.bookmarks {
            out.push(n(self.bookmarks, "bookmark", "bookmarks"));
        }
        if c.history {
            out.push(n(self.history, "page of history", "pages of history"));
        }
        if c.tabs {
            out.push(format!(
                "{} in {}",
                n(self.tabs, "open tab", "open tabs"),
                n(self.spaces, "new space", "new spaces")
            ));
        }
        if c.passwords {
            out.push(n(self.passwords, "password", "passwords"));
        }
        out
    }
}

/// Every Chromium-family browser with a profile here.
pub fn browsers() -> Vec<Browser> {
    copper_import::config_dir()
        .map(|c| copper_import::detect(&c))
        .unwrap_or_default()
}

/// Run an import step by step, telling `progress` what is being read.
pub async fn run(shell: &Shell, choice: &Choice, progress: impl Fn(&str, f64)) -> Summary {
    let mut sum = Summary::default();
    let steps = [
        choice.bookmarks,
        choice.history,
        choice.tabs,
        choice.passwords,
    ]
    .iter()
    .filter(|b| **b)
    .count()
    .max(1) as f64;
    let mut done = 0.0;
    let name = choice.browser.name.clone();

    if choice.bookmarks {
        progress(&format!("Reading {name}'s bookmarks…"), done / steps);
        let dirs = choice.dirs();
        let read = rt::blocking(move || bookmarks::read(&dirs))
            .await
            .unwrap_or_default();
        if !read.complete() {
            sum.problems.push(format!(
                "Some of {name}'s bookmark files couldn't be read ({})",
                read.failed_files.len()
            ));
        }
        sum.bookmarks = read.count();
        if !read.nodes.is_empty() {
            let roots = {
                let mut b = shell.bookmarks_mut();
                let mut roots = b.roots().to_vec();
                bookmarks::merge(read.nodes, &mut roots);
                b.replace(roots.clone());
                roots
            };
            shell.saver().schedule(roots);
        }
        done += 1.0;
    }

    if choice.history {
        progress(&format!("Reading {name}'s history…"), done / steps);
        let dirs = choice.dirs();
        let places = rt::blocking(move || history::read(&dirs, HISTORY_LIMIT))
            .await
            .unwrap_or_default();
        sum.history = places.len();
        if !places.is_empty() {
            let mut h = shell.history_mut();
            for p in &places {
                p.take_into(&mut h);
            }
        }
        shell.history_changed();
        done += 1.0;
    }

    if choice.tabs {
        progress(&format!("Reading {name}'s open tabs…"), done / steps);
        let (root, profile, source) = (
            choice.browser.root.clone(),
            choice.profile.clone(),
            name.clone(),
        );
        let read = rt::blocking(move || match profile {
            Some(p) => tabs::read_profile(&root.join(p), &source),
            None => tabs::read(&root, &source),
        })
        .await
        .unwrap_or_else(|| Err(copper_import::ImportError::Unreadable("the session".into())));
        match read {
            Ok(spaces) => {
                let (s, t) = open_spaces(shell, spaces);
                sum.spaces = s;
                sum.tabs = t;
            }
            Err(e) => sum.problems.push(e.to_string()),
        }
        done += 1.0;
    }

    if choice.passwords {
        progress(&format!("Reading {name}'s passwords…"), done / steps);
        let browser = choice.browser.clone();
        let dirs = choice.dirs();
        let found = rt::run(async move {
            let keys = passwords::Keys::for_browser(&browser).await?;
            passwords::read(&dirs, &keys)
        })
        .await;
        match found {
            Some(Ok(found)) => {
                if found.undecryptable > 0 {
                    sum.problems.push(format!(
                        "{} {} couldn't be decrypted",
                        found.undecryptable,
                        if found.undecryptable == 1 {
                            "password"
                        } else {
                            "passwords"
                        }
                    ));
                }
                let logins: Vec<(String, String, String, Option<f64>)> = found
                    .logins
                    .into_iter()
                    .map(|l| (l.host, l.user, l.password, l.used))
                    .collect();
                let wanted = logins.len();
                progress("Saving passwords to the keyring…", (done + 0.5) / steps);
                match super::vault() {
                    Some(v) => {
                        let saved = rt::run(async move { v.save_imported(logins).await })
                            .await
                            .unwrap_or_else(|| Err("no answer".into()));
                        match saved {
                            Ok(n) => {
                                sum.passwords = n;
                                if n < wanted {
                                    sum.problems.push(format!(
                                        "The keyring took {n} of {wanted} passwords"
                                    ));
                                }
                            }
                            Err(e) => sum.problems.push(format!("Passwords: {e}")),
                        }
                    }
                    None => sum.problems.push("Passwords aren't available".into()),
                }
                for host in &found.never {
                    super::set_never(shell, host, true);
                }
                sum.never = found.never.len();
            }
            Some(Err(e)) => sum.problems.push(e.to_string()),
            None => sum.problems.push("Reading passwords stopped".into()),
        }
    }
    progress("Done", 1.0);
    sum
}

/// Each imported window becomes a new space of sleeping tabs.
fn open_spaces(shell: &Shell, spaces: Vec<tabs::ImportedSpace>) -> (usize, usize) {
    let mut count = (0, 0);
    shell.with_browser(|b| {
        for imp in spaces {
            let real: Vec<&tabs::OpenTab> = imp
                .tabs
                .iter()
                .filter(|t| t.url.starts_with("http://") || t.url.starts_with("https://"))
                .collect();
            if real.is_empty() {
                continue;
            }
            let colour = SpaceColour::unused(b.spaces());
            let id = b.add_space(&imp.space.name);
            let profile = imp.space.profile.clone();
            b.update_space(&id, |s| {
                colour.apply(s);
                s.profile = profile;
            });
            for t in real {
                let tab = b.open_tab(
                    &t.url,
                    OpenOptions {
                        title: t.title.clone(),
                        space: Some(id.clone()),
                        at_end: true,
                        ..Default::default()
                    },
                );
                b.set_sleeping(&tab, true);
                count.1 += 1;
            }
            count.0 += 1;
        }
    });
    count
}

/// The sheet.
pub fn present(shell: &Shell) {
    let dialog = adw::Dialog::new();
    dialog.set_title("Bring In");
    dialog.set_content_width(460);
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    view.add_top_bar(&header);
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_vhomogeneous(false);
    view.set_content(Some(&stack));
    dialog.set_child(Some(&view));

    let found = browsers();
    let choose = adw::PreferencesPage::new();
    let intro = adw::PreferencesGroup::new();
    intro.set_title("Bring in from another browser");
    intro.set_description(Some(
        "Copper reads a copy of the other browser's files and changes nothing there. Passwords go into your keyring.",
    ));
    choose.add(&intro);

    if found.is_empty() {
        let r = adw::ActionRow::new();
        r.set_title("No Chrome, Chromium or Brave profile found");
        r.set_subtitle("Copper looks in ~/.config/google-chrome, ~/.config/chromium and ~/.config/BraveSoftware/Brave-Browser");
        r.set_subtitle_lines(0);
        intro.add(&r);
        stack.add_named(&choose, Some("choose"));
        dialog.present(Some(shell.window()));
        return;
    }

    let names: Vec<&str> = found.iter().map(|b| b.name.as_str()).collect();
    let which = adw::ComboRow::new();
    which.set_title("Browser");
    which.set_model(Some(&gtk::StringList::new(&names)));
    intro.add(&which);
    let profile = adw::ComboRow::new();
    profile.set_title("Profile");
    intro.add(&profile);
    let profiles: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
    let fill_profiles = {
        let found = found.clone();
        let profile = profile.clone();
        let profiles = profiles.clone();
        move |i: u32| {
            let Some(b) = found.get(i as usize) else {
                return;
            };
            let mut keys = vec![None];
            let mut labels = vec!["All profiles".to_string()];
            for p in b.profiles() {
                let label = copper_import::profile_name(&b.root.join(&p))
                    .map(|n| format!("{n} ({p})"))
                    .unwrap_or_else(|| p.clone());
                labels.push(label);
                keys.push(Some(p));
            }
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            profile.set_model(Some(&gtk::StringList::new(&refs)));
            profile.set_selected(0);
            *profiles.borrow_mut() = keys;
        }
    };
    fill_profiles(0);
    which.connect_selected_notify(move |r| fill_profiles(r.selected()));

    let what = adw::PreferencesGroup::new();
    what.set_title("What to bring");
    let switch = |title: &str, sub: &str| {
        let s = adw::SwitchRow::new();
        s.set_title(title);
        s.set_subtitle(sub);
        s.set_active(true);
        what.add(&s);
        s
    };
    let sw_bookmarks = switch(
        "Bookmarks",
        "Merged with yours; the bar's contents at the top",
    );
    let sw_history = switch(
        "History",
        "Pages you've been to, for the field's suggestions",
    );
    let sw_tabs = switch(
        "Open tabs",
        "Each window becomes a new space of sleeping tabs",
    );
    let keyring_ok = super::vault().is_some_and(|v| v.snapshot().keyring_ready());
    let sw_passwords = switch(
        "Passwords",
        if keyring_ok {
            "Into your keyring"
        } else {
            "Needs a keyring (Secret Service) — none is available"
        },
    );
    if !keyring_ok {
        sw_passwords.set_active(false);
        sw_passwords.set_sensitive(false);
    }
    choose.add(&what);
    let go_group = adw::PreferencesGroup::new();
    let go = gtk::Button::with_label("Bring In");
    go.add_css_class("suggested-action");
    go.add_css_class("pill");
    go.set_halign(gtk::Align::Center);
    go_group.add(&go);
    choose.add(&go_group);
    stack.add_named(&choose, Some("choose"));

    // Working.
    let working = gtk::Box::new(gtk::Orientation::Vertical, 14);
    working.set_margin_top(48);
    working.set_margin_bottom(48);
    working.set_margin_start(32);
    working.set_margin_end(32);
    let doing = gtk::Label::new(Some("Starting…"));
    doing.set_wrap(true);
    let bar = gtk::ProgressBar::new();
    working.append(&doing);
    working.append(&bar);
    stack.add_named(&working, Some("working"));

    // Done.
    let summary = adw::StatusPage::new();
    summary.set_icon_name(Some("emblem-ok-symbolic"));
    summary.add_css_class("compact");
    let close = gtk::Button::with_label("Done");
    close.add_css_class("pill");
    close.add_css_class("suggested-action");
    close.set_halign(gtk::Align::Center);
    // Mac Flow's "Open the Chrome guide": offered when tabs or bookmarks
    // came over from Chrome.
    let guide = gtk::Button::with_label("Open the Chrome guide");
    guide.add_css_class("flat");
    guide.set_halign(gtk::Align::Center);
    guide.set_visible(false);
    let guide_failed = gtk::Label::new(Some("Couldn't open the Chrome guide."));
    guide_failed.add_css_class("dim-label");
    guide_failed.set_visible(false);
    let done_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    done_box.append(&close);
    done_box.append(&guide);
    done_box.append(&guide_failed);
    summary.set_child(Some(&done_box));
    stack.add_named(&summary, Some("done"));
    {
        let (dialog, weak) = (dialog.clone(), shell.downgrade());
        let failed = guide_failed.clone();
        guide.connect_clicked(move |b| {
            let Some(shell) = weak.upgrade() else { return };
            b.set_label("Opening the guide…");
            b.set_sensitive(false);
            failed.set_visible(false);
            let (b, dialog, failed) = (b.clone(), dialog.clone(), failed.clone());
            glib::spawn_future_local(async move {
                let landed = crate::canvas::land_chrome_canvas(&shell).await;
                tracing::info!(?landed, "bring in: chrome guide");
                if landed.is_some() {
                    dialog.close();
                } else {
                    b.set_label("Open the Chrome guide");
                    b.set_sensitive(true);
                    failed.set_visible(true);
                }
            });
        });
    }
    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| {
            dialog.close();
        });
    }

    let weak = shell.downgrade();
    let stack2 = stack.clone();
    go.connect_clicked(move |_| {
        let Some(shell) = weak.upgrade() else { return };
        let Some(browser) = found.get(which.selected() as usize).cloned() else {
            return;
        };
        let profile = profiles
            .borrow()
            .get(profile.selected() as usize)
            .cloned()
            .flatten();
        let choice = Choice {
            browser,
            profile,
            bookmarks: sw_bookmarks.is_active(),
            history: sw_history.is_active(),
            tabs: sw_tabs.is_active(),
            passwords: sw_passwords.is_active() && sw_passwords.is_sensitive(),
        };
        stack2.set_visible_child_name("working");
        let (doing, bar, summary, stack, guide) = (
            doing.clone(),
            bar.clone(),
            summary.clone(),
            stack2.clone(),
            guide.clone(),
        );
        glib::spawn_future_local(async move {
            let d2 = doing.clone();
            let b2 = bar.clone();
            let sum = run(&shell, &choice, move |text, fraction| {
                d2.set_text(text);
                b2.set_fraction(fraction);
            })
            .await;
            let lines = sum.lines(&choice);
            summary.set_title(&format!("Brought in from {}", choice.browser.name));
            let mut desc = lines.join("\n");
            if !sum.problems.is_empty() {
                summary.set_icon_name(Some("dialog-warning-symbolic"));
                desc.push_str("\n\n");
                desc.push_str(&sum.problems.join("\n"));
            }
            summary.set_description(Some(&glib::markup_escape_text(&desc)));
            guide.set_visible(
                choice.browser.name == "Chrome" && (sum.tabs > 0 || sum.bookmarks > 0),
            );
            stack.set_visible_child_name("done");
            tracing::info!(?sum, "bring in");
            LAST.with(|l| *l.borrow_mut() = Some(sum));
        });
    });
    stack.set_visible_child_name("choose");
    dialog.present(Some(shell.window()));
    CURRENT.with(|c| *c.borrow_mut() = Some(dialog.downgrade()));
}

thread_local! {
    /// The last import's summary (self-test).
    pub static LAST: RefCell<Option<Summary>> = const { RefCell::new(None) };
    static CURRENT: RefCell<Option<glib::WeakRef<adw::Dialog>>> = const { RefCell::new(None) };
}

/// The open sheet (self-test).
pub fn current() -> Option<adw::Dialog> {
    CURRENT.with(|c| c.borrow().as_ref().and_then(|w| w.upgrade()))
}
