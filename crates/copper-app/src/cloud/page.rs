//! Settings › Cloud (Mac: `CloudSettings.swift`, docs/cloud.md).
//!
//! Three steps, in order — Connect, Account, Sync — or one, with a pairing
//! code from a device already signed in, or a join link (`#sso=`: "Continue
//! with Google", the company sign-in in the system browser). An instance
//! linked any other way that offers single sign-on shows the same button on
//! the Account step. The sign-in itself and what a `copper://cloud/join`
//! link does are copper-shell-core's ([`copper_shell_core::cloud::sso`],
//! [`copper_shell_core::cloud::join`]); this page draws their state. Once sync is on the page shows the
//! status line with Sync now and Pause, the switches (each at once), the tabs
//! open on the other devices, the account (rename this device, sign out),
//! Pair another device, the instance with its fingerprint and Disconnect, and
//! the log.
//!
//! The page is rebuilt when the phase changes (linked, signed in, sync on);
//! within a phase only the live parts (status, devices, log, the pairing
//! card's countdown) are redrawn, so nothing typed is lost.

use super::{Cloud, ago, count, get, run_then, spawn};
use crate::platform::Gtk;
use crate::shell::{OpenTab, Shell, WeakShell};
use copper_cloud::client::MintedPairing;
use copper_cloud::link::{is_bare_pairing_code, manual_link};
use copper_cloud::sync::history::parse_stamp;
use copper_cloud::{Code, Domain, JoinCode, SsoLink, parse_join_code};
use copper_shell_core::cloud::sso::{
    SsoDone, SsoState, SsoTarget, domain_hint, same_instance, sign_in,
};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::{Rc, Weak};
use std::time::Duration;

thread_local! {
    static CURRENT: RefCell<Weak<CloudPage>> = const { RefCell::new(Weak::new()) };
}

/// The Cloud page on screen right now, if any.
pub fn current() -> Option<Rc<CloudPage>> {
    CURRENT.with(|c| c.borrow().upgrade())
}

/// The browser domains, in the page's order (Personal canvas is apart).
const BROWSER: [Domain; 5] = [
    Domain::Spaces,
    Domain::Settings,
    Domain::Bookmarks,
    Domain::Tabs,
    Domain::History,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No cloud this run (cloud.json unreadable).
    Off,
    Unlinked,
    Linked,
    SignedIn,
    Syncing,
}

fn phase() -> Phase {
    match get() {
        None => Phase::Off,
        Some(c) if !c.client.is_linked() => Phase::Unlinked,
        Some(c) if !c.client.is_signed_in() => Phase::Linked,
        Some(c) if !c.sync.is_on() => Phase::SignedIn,
        Some(_) => Phase::Syncing,
    }
}

/// A pairing code on the card.
struct Pairing {
    minted: MintedPairing,
    expires: f64,
    /// `None` while waiting; `Some(true)` used, `Some(false)` revoked.
    outcome: Option<bool>,
}

#[derive(Default)]
struct Widgets {
    // Connect.
    code: Option<adw::EntryRow>,
    said: Option<adw::ActionRow>,
    connect: Option<gtk::Button>,
    connect_error: Option<gtk::Label>,
    trust: Option<gtk::Box>,
    trust_fp: Option<gtk::Label>,
    trust_button: Option<gtk::Button>,
    hand_address: Option<adw::EntryRow>,
    hand_key: Option<adw::EntryRow>,
    hand_fp: Option<adw::EntryRow>,
    // Single sign-on: the row it becomes while the browser is open, and the
    // Account step's "Continue with …".
    sso_wait: Option<adw::ActionRow>,
    sso_spinner: Option<gtk::Spinner>,
    sso_cancel: Option<gtk::Button>,
    sso_area: Option<gtk::Box>,
    sso_button: Option<gtk::Button>,
    sso_error: Option<gtk::Label>,
    password_more: Option<gtk::Button>,
    // A join link for another instance; a word about a link.
    notice: Option<adw::PreferencesGroup>,
    notice_title: Option<gtk::Label>,
    notice_body: Option<gtk::Label>,
    notice_use: Option<gtk::Button>,
    notice_dismiss: Option<gtk::Button>,
    // Account.
    email: Option<adw::EntryRow>,
    password: Option<adw::PasswordEntryRow>,
    name: Option<adw::EntryRow>,
    account_button: Option<gtk::Button>,
    account_error: Option<gtk::Label>,
    // Sync.
    switches: Vec<(Domain, adw::SwitchRow)>,
    turn_on: Option<gtk::Button>,
    sync_error: Option<gtk::Label>,
    status: Option<adw::ActionRow>,
    devices: Option<adw::PreferencesGroup>,
    device_rows: Vec<gtk::Widget>,
    log: Option<gtk::Label>,
    device_name: Option<adw::EntryRow>,
    // Pairing.
    pair_box: Option<gtk::Box>,
    pair_code: Option<gtk::Label>,
    pair_left: Option<gtk::Label>,
    pair_make: Option<gtk::Button>,
    pair_revoke: Option<gtk::Button>,
    pair_copy: Option<gtk::Button>,
    pair_said: Option<gtk::Label>,
}

/// Settings › Cloud.
pub struct CloudPage {
    page: glib::WeakRef<adw::PreferencesPage>,
    shell: WeakShell,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    phase: Cell<Phase>,
    w: RefCell<Widgets>,
    /// Create account rather than Sign in.
    creating: Cell<bool>,
    /// The fingerprint the instance showed when the code had none.
    seen: RefCell<Option<String>>,
    busy: Cell<bool>,
    pairing: RefCell<Option<Pairing>>,
    /// Domains picked before "Turn on sync".
    picked: RefCell<BTreeSet<Domain>>,
    /// What the page says once about a `copper://cloud/join` link ("Already
    /// signed in to …"), until dismissed.
    note: RefCell<Option<String>>,
    /// The sign-in options the Account step was drawn with
    /// ([`SsoState::discovered`]).
    discovered: Cell<u64>,
    /// The linked instance was asked for its sign-in options.
    asked: Cell<bool>,
    /// "Sign in with a password instead" pressed.
    password_open: Cell<bool>,
    /// "Disconnect and use it": the join link waiting is used once Connect
    /// shows.
    use_offer: Cell<bool>,
    /// The single sign-on problem shown now (a password one is left alone).
    sso_problem: RefCell<Option<String>>,
    /// The last sign-in that was said (each new one gets a toast).
    sso_done: RefCell<Option<SsoDone>>,
    /// The pasted code's row says something (hidden while signing in).
    said_shown: Cell<bool>,
}

/// Build the page (the Settings dialog calls this each time it opens).
pub fn build(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_name(Some("cloud"));
    let picked: BTreeSet<Domain> = match get() {
        Some(c) if !c.sync.enabled().is_empty() => c.sync.enabled(),
        _ => Domain::ALL.into_iter().collect(),
    };
    let me = Rc::new(CloudPage {
        page: page.downgrade(),
        shell: shell.downgrade(),
        groups: RefCell::default(),
        phase: Cell::new(Phase::Off),
        w: RefCell::default(),
        creating: Cell::new(false),
        seen: RefCell::default(),
        busy: Cell::new(false),
        pairing: RefCell::default(),
        picked: RefCell::new(picked),
        note: RefCell::default(),
        discovered: Cell::new(0),
        asked: Cell::new(false),
        password_open: Cell::new(false),
        use_offer: Cell::new(false),
        sso_problem: RefCell::default(),
        sso_done: RefCell::new(sign_in().state().signed_in),
        said_shown: Cell::new(false),
    });
    CURRENT.with(|c| *c.borrow_mut() = Rc::downgrade(&me));
    me.rebuild();
    // The page keeps itself alive as long as its widget does.
    let keep = me.clone();
    page.connect_map(move |_| {
        let _ = &keep;
        if let Some(c) = get() {
            let i = c.intelligence.clone();
            spawn(async move { i.settings_opened().await });
        }
    });
    watch(&me);
    page
}

/// Redraw on every change to the cloud's or the engine's status, and once a
/// second for the clocks ("2 minutes ago", the code's countdown).
fn watch(me: &Rc<CloudPage>) {
    let Some(cloud) = get() else { return };
    let weak = Rc::downgrade(me);
    let mut status = cloud.client.watch_status();
    glib::spawn_future_local(async move {
        while status.changed().await.is_ok() {
            let Some(me) = weak.upgrade() else { return };
            me.update();
        }
    });
    let weak = Rc::downgrade(me);
    let mut sync = cloud.sync.watch();
    glib::spawn_future_local(async move {
        while sync.changed().await.is_ok() {
            let Some(me) = weak.upgrade() else { return };
            me.update();
        }
    });
    let weak = Rc::downgrade(me);
    let mut sso = sign_in().watch();
    glib::spawn_future_local(async move {
        while sso.changed().await.is_ok() {
            let Some(me) = weak.upgrade() else { return };
            me.draw_sso();
        }
    });
    let weak = Rc::downgrade(me);
    let mut tick = 0u32;
    glib::timeout_add_local(Duration::from_secs(1), move || {
        let Some(me) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if me.page.upgrade().is_none() {
            return glib::ControlFlow::Break;
        }
        tick += 1;
        me.tick_pairing(tick.is_multiple_of(3));
        if tick.is_multiple_of(15) {
            me.update();
        }
        glib::ControlFlow::Continue
    });
}

// MARK: small widgets

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_xalign(0.0);
    l.set_halign(gtk::Align::Fill);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn error_label() -> gtk::Label {
    let l = label("", &["error", "caption"]);
    l.set_visible(false);
    l.set_margin_top(6);
    l.set_selectable(true);
    l
}

fn show_error(l: &Option<gtk::Label>, text: Option<&str>) {
    if let Some(l) = l {
        l.set_text(text.unwrap_or(""));
        l.set_visible(text.is_some_and(|t| !t.is_empty()));
    }
}

fn button(text: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    for c in classes {
        b.add_css_class(c);
    }
    b.set_valign(gtk::Align::Center);
    b
}

/// A row of buttons at the foot of a group, right-aligned.
fn foot(buttons: &[&gtk::Button]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::End);
    row.set_margin_top(12);
    for b in buttons {
        row.append(*b);
    }
    row
}

fn group(title: &str, description: Option<&str>) -> adw::PreferencesGroup {
    let g = adw::PreferencesGroup::new();
    if !title.is_empty() {
        g.set_title(title);
    }
    if let Some(d) = description {
        g.set_description(Some(d));
    }
    g
}

fn plain_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let r = adw::ActionRow::new();
    r.set_use_markup(false);
    r.set_title(title);
    r.set_subtitle(subtitle);
    r.set_subtitle_lines(0);
    r
}

/// `ab:cd:…` cut to its first `pairs` pairs.
fn short_fp(fp: &str, pairs: usize) -> String {
    let parts: Vec<&str> = fp.split(':').collect();
    if parts.len() <= pairs {
        fp.to_string()
    } else {
        format!("{}…", parts[..pairs].join(":"))
    }
}

/// How the linked instance is trusted, in one line: its pin, a publicly
/// trusted certificate (no pin: the system's roots), or plain HTTP on
/// loopback.
fn certificate_line(cloud: &Cloud, status: &copper_cloud::CloudStatus) -> String {
    match &status.fingerprint {
        Some(fp) => format!("Pinned certificate — SHA-256 {}", short_fp(fp, 4)),
        None if cloud.client.link().is_some_and(|l| l.endpoint.https) => {
            "Publicly trusted certificate".into()
        }
        None => "Plain HTTP on this device (development)".into(),
    }
}

/// `ab:cd:…` from 64 hex characters (or as given).
fn pairs(hex: &str) -> String {
    if hex.contains(':') {
        hex.to_string()
    } else {
        copper_cloud::link::pairs(hex)
    }
}

impl CloudPage {
    fn page(&self) -> Option<adw::PreferencesPage> {
        self.page.upgrade()
    }

    fn shell(&self) -> Option<Shell> {
        self.shell.upgrade()
    }

    /// What phase the page shows.
    pub fn phase(&self) -> Phase {
        self.phase.get()
    }

    fn add(&self, g: adw::PreferencesGroup) {
        if let Some(p) = self.page() {
            p.add(&g);
        }
        self.groups.borrow_mut().push(g);
    }

    /// Redraw: the whole page when the phase changed, else the live parts.
    pub fn update(self: &Rc<Self>) {
        if phase() != self.phase.get() {
            self.rebuild();
        } else {
            self.refresh();
        }
    }

    fn rebuild(self: &Rc<Self>) {
        let Some(page) = self.page() else { return };
        for g in self.groups.borrow_mut().drain(..) {
            page.remove(&g);
        }
        *self.w.borrow_mut() = Widgets::default();
        let p = phase();
        self.phase.set(p);
        let intro = group(
            "",
            Some(
                "Browser sync — spaces, settings, bookmarks, history — and shared canvases, through a Copper Cloud instance you or your team runs.",
            ),
        );
        self.add(intro);
        self.build_notice();
        if p != Phase::Linked {
            self.asked.set(false);
            self.password_open.set(false);
        }
        self.sso_problem.replace(None);
        self.discovered.set(sign_in().state().discovered);
        match p {
            Phase::Off => {
                let g = group(
                    "",
                    Some("Copper Cloud is off this run: cloud.json couldn't be read."),
                );
                self.add(g);
            }
            Phase::Unlinked => self.build_connect(),
            Phase::Linked => {
                self.ask_sso();
                self.build_connected();
                self.build_account_form();
            }
            Phase::SignedIn => {
                self.build_connected();
                self.build_choose();
                self.build_account();
                self.build_pairing();
            }
            Phase::Syncing => {
                self.build_status();
                self.build_switches();
                self.build_devices();
                self.build_account();
                self.build_pairing();
                self.build_instance();
                self.build_log();
            }
        }
        self.refresh();
        self.draw_sso();
    }

    /// The live parts.
    fn refresh(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let status = cloud.sync.status();
        let w = self.w.borrow();
        if let Some(row) = &w.status {
            let line = match &status.state {
                copper_cloud::sync::SyncState::Idle => "Up to date".to_string(),
                other => other.text(),
            };
            row.set_title(&line);
            row.set_subtitle(&match status.last_sync {
                Some(t) => format!("Sync on · Last synced {}", ago(t)),
                None => "Sync on · Not synced yet".into(),
            });
        }
        for (d, row) in &w.switches {
            let on = status.enabled.contains(d);
            if row.is_active() != on && self.phase.get() == Phase::Syncing {
                row.set_active(on);
            }
        }
        if let Some(log) = &w.log {
            let lines: Vec<String> = cloud
                .sync
                .log()
                .iter()
                .rev()
                .take(40)
                .map(|(t, text)| format!("{}  {text}", clock(*t)))
                .collect();
            log.set_text(&if lines.is_empty() {
                "Nothing yet".to_string()
            } else {
                lines.join("\n")
            });
        }
        drop(w);
        self.refresh_devices(&status.other_devices);
        if let Some(entry) = &self.w.borrow().device_name
            && !entry.has_focus()
            && entry.text() != cloud.client.device_name()
        {
            entry.set_text(&cloud.client.device_name());
        }
    }

    // MARK: Connect

    fn build_connect(self: &Rc<Self>) {
        let g = group(
            "Connect",
            Some(
                "Paste the link code or join link from your Copper Cloud administrator — or a pairing code from a device that is already signed in.",
            ),
        );
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let code = adw::EntryRow::new();
        code.set_title("Link code, join link or pairing code");
        list.append(&code);
        let said = plain_row("", "");
        said.set_visible(false);
        list.append(&said);
        let (wait, spinner, cancel) = self.waiting_row();
        list.append(&wait);
        g.add(&list);

        let error = error_label();
        g.add(&error);

        // The fingerprint the instance showed, for a code without one.
        let trust = gtk::Box::new(gtk::Orientation::Vertical, 8);
        trust.add_css_class("card");
        trust.set_margin_top(12);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
        inner.set_margin_top(14);
        inner.set_margin_bottom(14);
        inner.set_margin_start(14);
        inner.set_margin_end(14);
        inner.append(&label(
            "This device doesn't trust the instance's certificate, and the code or link has no fingerprint to pin it by. Its certificate's fingerprint is:",
            &[],
        ));
        let fp = label("", &["monospace"]);
        fp.set_selectable(true);
        inner.append(&fp);
        inner.append(&label(
            "Compare it with `copper-cloud doctor` on the server. Trusting it pins this instance to that certificate from now on.",
            &["dim-label", "caption"],
        ));
        let trust_button = button("Trust and connect", &["suggested-action"]);
        trust_button.set_halign(gtk::Align::End);
        inner.append(&trust_button);
        trust.append(&inner);
        trust.set_visible(false);
        g.add(&trust);

        let connect = button("Connect", &["suggested-action"]);
        connect.set_sensitive(false);
        g.add(&foot(&[&connect]));

        let hint = label(
            "Running your own server? `copper-cloud link-code` prints one.",
            &["dim-label", "caption"],
        );
        hint.set_margin_top(6);
        g.add(&hint);
        self.add(g);

        // Enter it by hand: the address, the key and the fingerprint.
        let hand = group("", None);
        let expander = adw::ExpanderRow::new();
        expander.set_title("Enter it by hand");
        let address = adw::EntryRow::new();
        address.set_title("Address (host:port)");
        let key = adw::EntryRow::new();
        key.set_title("Key — the k= part of the link code");
        let hfp = adw::EntryRow::new();
        hfp.set_title("Certificate fingerprint (SHA-256, optional)");
        expander.add_row(&address);
        expander.add_row(&key);
        expander.add_row(&hfp);
        let hand_connect = button("Connect", &[]);
        let row = adw::ActionRow::new();
        row.set_title("Leave the fingerprint empty for a public certificate");
        row.add_css_class("dim-label");
        row.add_suffix(&hand_connect);
        expander.add_row(&row);
        let hand_list = gtk::ListBox::new();
        hand_list.add_css_class("boxed-list");
        hand_list.set_selection_mode(gtk::SelectionMode::None);
        hand_list.append(&expander);
        hand.add(&hand_list);
        self.add(hand);

        let weak = Rc::downgrade(self);
        code.connect_changed(move |_| {
            if let Some(me) = weak.upgrade() {
                me.read_code();
            }
        });
        let weak = Rc::downgrade(self);
        code.connect_entry_activated(move |_| {
            if let Some(me) = weak.upgrade()
                && me
                    .w
                    .borrow()
                    .connect
                    .as_ref()
                    .is_some_and(|b| b.is_sensitive())
            {
                me.connect_code(None);
            }
        });
        let weak = Rc::downgrade(self);
        connect.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.connect_code(None);
            }
        });
        let weak = Rc::downgrade(self);
        trust_button.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                let seen = me.seen.borrow().clone();
                me.connect_code(seen);
            }
        });
        let weak = Rc::downgrade(self);
        hand_connect.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.connect_by_hand();
            }
        });

        let mut w = self.w.borrow_mut();
        w.code = Some(code);
        w.said = Some(said);
        w.connect = Some(connect);
        w.connect_error = Some(error);
        w.trust = Some(trust);
        w.trust_fp = Some(fp);
        w.trust_button = Some(trust_button);
        w.hand_address = Some(address);
        w.hand_key = Some(key);
        w.hand_fp = Some(hfp);
        w.sso_wait = Some(wait);
        w.sso_spinner = Some(spinner);
        w.sso_cancel = Some(cancel);
    }

    /// Say what was pasted: which kind of code, the host, the pin.
    fn read_code(&self) {
        let w = self.w.borrow();
        let (Some(code), Some(said), Some(connect)) = (&w.code, &w.said, &w.connect) else {
            return;
        };
        if let Some(t) = &w.trust {
            t.set_visible(false);
        }
        show_error(&w.connect_error, None);
        self.seen.replace(None);
        self.sso_problem.replace(None);
        sign_in().clear_problem();
        let text = code.text().to_string();
        let linked = get().and_then(|c| c.client.link());
        let parsed = parse_join_code(&text, linked.as_ref());
        let describe = |https: bool, fp: Option<&str>| -> String {
            match (https, fp) {
                (false, _) => "Plain HTTP — this device only, for development".into(),
                (true, Some(fp)) => format!("Pinned to this certificate (SHA-256): {}", pairs(fp)),
                (true, None) => {
                    "No fingerprint in it — a publicly trusted certificate is checked as usual; any other is shown to you to compare first".into()
                }
            }
        };
        self.said_shown.set(parsed.is_some());
        match parsed {
            Some(JoinCode::Link(link)) => {
                said.set_title("Link code — connects to the instance; you sign in next");
                said.set_subtitle(&format!(
                    "{}\n{}",
                    link.host(),
                    describe(link.endpoint.https, link.fingerprint.as_deref())
                ));
                said.set_visible(true);
                connect.set_label("Connect");
                connect.set_sensitive(!self.busy.get());
            }
            Some(JoinCode::Pairing(p)) => {
                said.set_title("Pairing code — links this device and signs you in");
                said.set_subtitle(&format!(
                    "{}\n{}",
                    p.host(),
                    describe(p.endpoint.https, p.fingerprint.as_deref())
                ));
                said.set_visible(true);
                connect.set_label("Pair this device");
                connect.set_sensitive(!self.busy.get());
            }
            Some(JoinCode::Sso(link)) => {
                let name = sign_in().provider_name(&link.endpoint, &link.provider);
                said.set_title(&format!(
                    "Join link — links this device and signs you in with {name} in your browser"
                ));
                said.set_subtitle(&format!(
                    "{}\n{}",
                    link.host(),
                    describe(link.endpoint.https, link.fingerprint.as_deref())
                ));
                said.set_visible(true);
                connect.set_label(&format!("Continue with {name}"));
                connect.set_sensitive(!self.busy.get());
                // The instance's own name for it, and the domain hint.
                let endpoint = link.endpoint.clone();
                let pin = link.fingerprint.clone();
                spawn(async move {
                    sign_in().discover(&endpoint, pin.as_deref(), false).await;
                });
            }
            None => {
                said.set_visible(false);
                connect.set_label("Connect");
                connect.set_sensitive(false);
                let t = text.trim();
                if t.is_empty() {
                    return;
                }
                let raw: String = t.chars().filter(|c| !c.is_whitespace()).collect();
                show_error(
                    &w.connect_error,
                    Some(if is_bare_pairing_code(&raw) {
                        "That's the pairing code without its address — copy the whole code, from copper-cloud:// to the end"
                    } else {
                        "That isn't a link code, a join link or a pairing code — all start copper-cloud://, then #k=, #sso= or #p="
                    }),
                );
            }
        }
    }

    /// The join link in the Connect field, if that is what it holds.
    fn field_sso(&self) -> Option<SsoLink> {
        let text = self.w.borrow().code.as_ref()?.text().to_string();
        match parse_join_code(&text, None)? {
            JoinCode::Sso(link) => Some(link),
            _ => None,
        }
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        let w = self.w.borrow();
        for b in [
            &w.connect,
            &w.trust_button,
            &w.account_button,
            &w.sso_button,
            &w.turn_on,
            &w.pair_make,
        ]
        .into_iter()
        .flatten()
        {
            b.set_sensitive(!busy);
        }
    }

    /// Connect (a link code), pair (a pairing code) or sign in through the
    /// browser (a join link). `trusting`: the fingerprint a person compared
    /// and chose to trust.
    pub fn connect_code(self: &Rc<Self>, trusting: Option<String>) {
        let Some(cloud) = get() else { return };
        let Some(text) = self.w.borrow().code.as_ref().map(|c| c.text().to_string()) else {
            return;
        };
        if let Some(link) = self.field_sso() {
            self.start_sso(SsoTarget::Join(link), trusting);
            return;
        }
        self.set_busy(true);
        show_error(&self.w.borrow().connect_error, None);
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        run_then(
            async move { client.connect_code(&text, trusting.as_deref()).await },
            move |result| {
                let Some(me) = weak.upgrade() else { return };
                me.set_busy(false);
                match result {
                    Ok(code) => {
                        let cloud = get().expect("cloud");
                        cloud.start();
                        if matches!(code, Code::Pairing(_)) {
                            me.after_pairing(cloud);
                        }
                        me.update();
                    }
                    Err(f) if f.code == "untrusted" && f.seen.is_some() => {
                        let seen = f.seen.clone().unwrap_or_default();
                        me.seen.replace(Some(seen.clone()));
                        let w = me.w.borrow();
                        if let Some(l) = &w.trust_fp {
                            l.set_text(&pairs(&seen));
                        }
                        if let Some(b) = &w.trust_button {
                            let pairing = w
                                .connect
                                .as_ref()
                                .is_some_and(|c| c.label().is_some_and(|l| l.starts_with("Pair")));
                            b.set_label(if pairing {
                                "Trust and pair"
                            } else {
                                "Trust and connect"
                            });
                        }
                        if let Some(t) = &w.trust {
                            t.set_visible(true);
                        }
                    }
                    // A join link: the browser, not this request.
                    Err(f) if f.code == "sso_link" => match me.field_sso() {
                        Some(link) => me.start_sso(SsoTarget::Join(link), None),
                        None => show_error(&me.w.borrow().connect_error, Some(&f.message)),
                    },
                    Err(f) => show_error(&me.w.borrow().connect_error, Some(&f.message)),
                }
            },
        );
    }

    /// Paired: sync turns on for everything (the switches from before, if
    /// this device was signed in once) — as after single sign-on.
    fn after_pairing(&self, cloud: &'static Cloud) {
        spawn(copper_shell_core::cloud::sync_after_sign_in(cloud));
    }

    fn connect_by_hand(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let (address, key, fp) = {
            let w = self.w.borrow();
            let t = |e: &Option<adw::EntryRow>| e.as_ref().map(|e| e.text().trim().to_string());
            (t(&w.hand_address), t(&w.hand_key), t(&w.hand_fp))
        };
        let (Some(address), Some(key)) = (address, key) else {
            return;
        };
        let fp = fp.filter(|f| !f.is_empty());
        let Some(link) = manual_link(&address, &key, fp.as_deref()) else {
            show_error(
                &self.w.borrow().connect_error,
                Some(
                    "Check the address, the key and the fingerprint (64 hex characters, or leave it empty for a public certificate)",
                ),
            );
            return;
        };
        self.set_busy(true);
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        run_then(
            async move { client.connect(link, None).await },
            move |result| {
                let Some(me) = weak.upgrade() else { return };
                me.set_busy(false);
                match result {
                    Ok(_) => {
                        if let Some(c) = get() {
                            c.start();
                        }
                        me.update();
                    }
                    Err(f) => show_error(&me.w.borrow().connect_error, Some(&f.message)),
                }
            },
        );
    }

    /// Connected: one line — the host, the start of the pin — that opens to
    /// the whole fingerprint and Disconnect.
    fn build_connected(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let status = cloud.client.status();
        let g = group("Connect", None);
        let row = adw::ExpanderRow::new();
        row.set_title(&format!(
            "Connected to {}",
            status.host.clone().unwrap_or_default()
        ));
        row.set_subtitle(&certificate_line(cloud, &status));
        self.instance_rows(&row, &status);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&row);
        g.add(&list);
        self.add(g);
    }

    /// The fingerprint, the version and Disconnect, inside an expander.
    fn instance_rows(self: &Rc<Self>, row: &adw::ExpanderRow, status: &copper_cloud::CloudStatus) {
        if let Some(fp) = &status.fingerprint {
            let r = plain_row("Pinned to this certificate (SHA-256)", fp);
            r.add_css_class("property");
            r.set_subtitle_selectable(true);
            row.add_row(&r);
        }
        if let Some(v) = &status.server_version {
            row.add_row(&plain_row("Version", &format!("copper-cloud {v}")));
        }
        let off = adw::ActionRow::new();
        off.set_title("Disconnect");
        off.set_subtitle(
            "Signs out and forgets this instance, to connect to another. Nothing is deleted.",
        );
        let b = button("Disconnect", &["destructive-action"]);
        off.add_suffix(&b);
        row.add_row(&off);
        let weak = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.ask_disconnect();
            }
        });
    }

    fn ask_disconnect(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let host = cloud
            .client
            .status()
            .host
            .unwrap_or_else(|| "the cloud".into());
        let dialog = adw::AlertDialog::new(
            Some(&format!("Disconnect from {host}?")),
            Some("Copper signs out, stops syncing and forgets the instance. Nothing is deleted."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("disconnect", "Disconnect");
        dialog.set_response_appearance("disconnect", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, r| {
            let Some(me) = weak.upgrade() else { return };
            if r == "disconnect" {
                me.disconnect();
            } else {
                me.use_offer.set(false);
            }
        });
        if let Some(p) = self.page() {
            dialog.present(Some(&p));
        }
    }

    /// Sign out (clearing this device's open tabs first), forget the instance.
    pub fn disconnect(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let sync = cloud.sync.clone();
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        run_then(
            async move {
                sync.signing_out().await;
                client.disconnect();
            },
            move |_| {
                if let Some(me) = weak.upgrade() {
                    me.update();
                }
            },
        );
    }

    // MARK: Single sign-on

    /// The row a sign-in becomes while the browser is open: a spinner,
    /// "Finish signing in in your browser…", Cancel. Hidden until then.
    fn waiting_row(self: &Rc<Self>) -> (adw::ActionRow, gtk::Spinner, gtk::Button) {
        let row = plain_row("Finish signing in in your browser…", "");
        let spinner = gtk::Spinner::new();
        spinner.set_valign(gtk::Align::Center);
        row.add_prefix(&spinner);
        let cancel = button("Cancel", &[]);
        row.add_suffix(&cancel);
        row.set_visible(false);
        cancel.connect_clicked(|_| sign_in().cancel());
        (row, spinner, cancel)
    }

    /// Sign in through `target`'s single sign-on, off the main thread; the
    /// page follows it through [`SsoState`] ([`Self::draw_sso`]).
    pub fn start_sso(self: &Rc<Self>, target: SsoTarget, trusting: Option<String>) {
        let Some(cloud) = get() else { return };
        {
            let w = self.w.borrow();
            show_error(&w.connect_error, None);
            show_error(&w.sso_error, None);
            if let Some(t) = &w.trust {
                t.set_visible(false);
            }
        }
        self.sso_problem.replace(None);
        self.seen.replace(None);
        // The outcome comes back through the state; nothing waits on it here.
        drop(sign_in().start(cloud, target, trusting, super::opener()));
    }

    /// Ask the linked instance which sign-ins it offers (once a page), for
    /// the Account step's "Continue with …".
    fn ask_sso(self: &Rc<Self>) {
        if self.asked.replace(true) {
            return;
        }
        let Some(link) = get().and_then(|c| c.client.link()) else {
            return;
        };
        spawn(async move {
            sign_in()
                .discover(&link.endpoint, link.fingerprint.as_deref(), true)
                .await;
        });
    }

    /// Draw single sign-on's state: the waiting row, the problem, the
    /// certificate to compare, a join link's code, the notice.
    fn draw_sso(self: &Rc<Self>) {
        let st = sign_in().state();
        match self.phase.get() {
            Phase::Unlinked => {
                self.apply_offer(&st);
                self.draw_connect_sso(&sign_in().state());
            }
            Phase::Linked => {
                if st.discovered != self.discovered.get() {
                    // The instance said what it offers: redraw the step.
                    self.rebuild_keeping_email();
                    return;
                }
                self.draw_account_sso(&st);
            }
            _ => {}
        }
        self.draw_notice(&st);
        let fresh = *self.sso_done.borrow() != st.signed_in;
        if fresh {
            if let (Some(done), Some(shell)) = (&st.signed_in, self.shell()) {
                shell.toast(&done.text());
            }
            self.sso_done.replace(st.signed_in.clone());
        }
    }

    fn rebuild_keeping_email(self: &Rc<Self>) {
        let keep = self.w.borrow().email.as_ref().map(|e| e.text());
        self.rebuild();
        if let (Some(text), Some(e)) = (keep, self.w.borrow().email.as_ref()) {
            e.set_text(&text);
        }
    }

    /// The waiting row for `running` (a sign-in to `host` with `name`).
    fn draw_waiting(&self, st: &SsoState, running: bool) {
        let w = self.w.borrow();
        if let Some(row) = &w.sso_wait {
            row.set_visible(running);
            if let Some(text) = st.phase.waiting_text() {
                row.set_title(text);
            }
            if let Some(t) = &st.target {
                let name = sign_in().provider_name(t.endpoint(), t.provider());
                row.set_subtitle(&format!("Signing in with {name} to {}", t.host()));
            }
        }
        if let Some(s) = &w.sso_spinner {
            s.set_spinning(running);
        }
        if let Some(c) = &w.sso_cancel {
            c.set_sensitive(running && st.phase.cancellable());
        }
    }

    /// A single sign-on problem in `line` — only when it changed, so a
    /// problem the page showed for something else stays.
    fn show_sso_problem(&self, line: &Option<gtk::Label>, problem: Option<&str>) {
        let mut last = self.sso_problem.borrow_mut();
        if last.as_deref() == problem {
            return;
        }
        match problem {
            Some(p) => show_error(line, Some(p)),
            None => {
                if line
                    .as_ref()
                    .is_some_and(|l| Some(l.text().as_str()) == last.as_deref())
                {
                    show_error(line, None);
                }
            }
        }
        *last = problem.map(str::to_string);
    }

    /// Connect: a join link in the field (or one a `copper://cloud/join`
    /// link started) becomes the waiting row while the browser is open.
    fn draw_connect_sso(&self, st: &SsoState) {
        let field = self.field_sso();
        // Not linked, any sign-in under way is a join link's.
        let running = st.phase.busy();
        self.draw_waiting(st, running);
        let w = self.w.borrow();
        if let Some(said) = &w.said {
            said.set_visible(!running && self.said_shown.get());
        }
        if let Some(c) = &w.connect {
            c.set_visible(!running);
            if let Some(link) = &field {
                c.set_label(&format!(
                    "Continue with {}",
                    sign_in().provider_name(&link.endpoint, &link.provider)
                ));
            }
        }
        if let Some(c) = &w.code {
            c.set_sensitive(!running);
        }
        let endpoint = field
            .as_ref()
            .map(|l| l.endpoint.clone())
            .or_else(|| st.target.as_ref().map(|t| t.endpoint().clone()));
        let Some(endpoint) = endpoint else { return };
        let problem = st.problem_for(&endpoint).map(str::to_string);
        drop(w);
        self.show_sso_problem(&self.w.borrow().connect_error, problem.as_deref());
        let w = self.w.borrow();
        if let Some(seen) = st.seen_for(&endpoint) {
            self.seen.replace(Some(seen.to_string()));
            if let Some(l) = &w.trust_fp {
                l.set_text(&pairs(seen));
            }
            if let Some(b) = &w.trust_button {
                b.set_label("Trust and continue");
            }
            if let Some(t) = &w.trust {
                t.set_visible(true);
            }
        }
    }

    /// Account: "Continue with …" becomes the waiting row while the browser
    /// is open; its problem shows under it.
    fn draw_account_sso(&self, st: &SsoState) {
        let Some(link) = get().and_then(|c| c.client.link()) else {
            return;
        };
        let running = st.running_for(&link.endpoint);
        self.draw_waiting(st, running);
        {
            let w = self.w.borrow();
            if let Some(a) = &w.sso_area {
                a.set_visible(!running);
            }
            if let Some(list) = w.sso_wait.as_ref().and_then(|r| r.parent()) {
                list.set_visible(running);
            }
            if let Some(b) = &w.account_button {
                b.set_sensitive(!running && !self.busy.get());
            }
        }
        let problem = st.problem_for(&link.endpoint).map(str::to_string);
        let line = self.w.borrow().sso_error.clone();
        self.show_sso_problem(&line, problem.as_deref());
    }

    /// Connect: the code a `copper://cloud/join` link brought goes into the
    /// field (once); "Disconnect and use it" starts its sign-in.
    fn apply_offer(self: &Rc<Self>, st: &SsoState) {
        let Some(offer) = st.offer.clone() else {
            return;
        };
        if self.w.borrow().code.is_none() {
            return;
        }
        self.type_code(&offer.text);
        let start = self.use_offer.replace(false);
        sign_in().dismiss_offer();
        if start
            && let JoinCode::Sso(link) = &offer.code
            && !sign_in().state().phase.busy()
        {
            self.start_sso(SsoTarget::Join(link.clone()), None);
        }
    }

    /// A notice at the top: a join link for another instance than this
    /// Copper's ("This link is for HOST" — Dismiss, or Disconnect and use
    /// it), or a word about a link ("Already signed in to …").
    fn build_notice(self: &Rc<Self>) {
        let g = group("", None);
        let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        card.add_css_class("card");
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
        inner.set_margin_top(14);
        inner.set_margin_bottom(14);
        inner.set_margin_start(14);
        inner.set_margin_end(14);
        let title = label("", &["heading"]);
        let body = label("", &["dim-label"]);
        inner.append(&title);
        inner.append(&body);
        let dismiss = button("Dismiss", &[]);
        let use_it = button("Disconnect and use it…", &["destructive-action"]);
        let row = foot(&[&use_it, &dismiss]);
        row.set_margin_top(6);
        inner.append(&row);
        card.append(&inner);
        g.add(&card);
        g.set_visible(false);
        self.add(g.clone());
        let weak = Rc::downgrade(self);
        dismiss.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.note.replace(None);
                sign_in().dismiss_offer();
                me.draw_notice(&sign_in().state());
            }
        });
        let weak = Rc::downgrade(self);
        use_it.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.use_offer.set(true);
                me.ask_disconnect();
            }
        });
        let mut w = self.w.borrow_mut();
        w.notice = Some(g);
        w.notice_title = Some(title);
        w.notice_body = Some(body);
        w.notice_use = Some(use_it);
        w.notice_dismiss = Some(dismiss);
    }

    fn draw_notice(&self, st: &SsoState) {
        let linked = get().and_then(|c| c.client.link());
        let other = st.offer.as_ref().and_then(|o| {
            let l = linked.as_ref()?;
            (!same_instance(&l.endpoint, o.endpoint())).then(|| (o.host(), l.host()))
        });
        let note = self.note.borrow().clone();
        let w = self.w.borrow();
        let (Some(g), Some(title), Some(body), Some(use_it)) =
            (&w.notice, &w.notice_title, &w.notice_body, &w.notice_use)
        else {
            return;
        };
        match (other, note) {
            (Some((host, mine)), _) => {
                title.set_text(&format!("This link is for {host}"));
                body.set_text(&format!(
                    "Copper is connected to {mine}, and nothing changes unless you choose to. To use the link, disconnect from {mine} first — nothing is deleted."
                ));
                body.set_visible(true);
                use_it.set_visible(true);
                g.set_visible(true);
            }
            (None, Some(text)) => {
                title.set_text(&text);
                body.set_visible(false);
                use_it.set_visible(false);
                g.set_visible(true);
            }
            (None, None) => g.set_visible(false),
        }
    }

    /// Say something once at the top of the page (a join link's "Already
    /// signed in to …"), until dismissed.
    pub fn say(&self, text: &str) {
        self.note.replace(Some(text.to_string()));
        self.draw_notice(&sign_in().state());
    }

    /// Put the cursor in the Connect field (when there is one).
    pub fn focus_code(&self) {
        if let Some(c) = &self.w.borrow().code {
            c.grab_focus();
        }
    }

    // MARK: Single sign-on, for QA

    /// The waiting row's words, while it shows.
    pub fn sso_waiting(&self) -> Option<String> {
        let w = self.w.borrow();
        let row = w.sso_wait.as_ref().filter(|r| r.is_visible())?;
        Some(format!(
            "{} — {}",
            row.title(),
            row.subtitle().map(|s| s.to_string()).unwrap_or_default()
        ))
    }

    /// What the step's main button says: Connect's (a join link's "Continue
    /// with …"), or the Account step's "Continue with …".
    pub fn sso_button_label(&self) -> Option<String> {
        let w = self.w.borrow();
        w.sso_button
            .as_ref()
            .or(w.connect.as_ref())
            .filter(|b| b.is_visible())
            .and_then(|b| b.label())
            .map(|l| l.to_string())
    }

    /// Press the Account step's "Continue with …" (or Connect's).
    pub fn press_sso(&self) {
        let b = {
            let w = self.w.borrow();
            w.sso_button.clone().or(w.connect.clone())
        };
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// Press the waiting row's Cancel.
    pub fn press_cancel_sso(&self) {
        let b = self.w.borrow().sso_cancel.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// Whether the password form waits behind "Sign in with a password
    /// instead".
    pub fn password_folded(&self) -> bool {
        self.phase.get() == Phase::Linked && self.w.borrow().email.is_none()
    }

    /// The notice at the top, while it shows.
    pub fn notice_text(&self) -> Option<String> {
        let w = self.w.borrow();
        w.notice.as_ref().filter(|g| g.is_visible())?;
        w.notice_title.as_ref().map(|l| l.text().to_string())
    }

    /// Press the notice's Dismiss.
    pub fn press_dismiss(&self) {
        let b = self.w.borrow().notice_dismiss.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// Press "Sign in with a password instead".
    pub fn press_password_instead(&self) {
        let b = self.w.borrow().password_more.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// What is in the Connect field.
    pub fn code_field(&self) -> Option<String> {
        self.w.borrow().code.as_ref().map(|c| c.text().to_string())
    }

    // MARK: Account

    fn build_account_form(self: &Rc<Self>) {
        let creating = self.creating.get();
        // What the instance offers besides a password, once it has said.
        let info = get()
            .and_then(|c| c.client.link())
            .and_then(|l| sign_in().known(&l.endpoint));
        let provider = info.as_ref().and_then(|i| i.offered().first().cloned());
        let password_signin = info.as_ref().is_none_or(|i| i.password_signin);
        if let Some(p) = &provider {
            self.build_sso_button(p);
            // Only admins sign in with a password there: it waits behind a
            // button.
            if !password_signin && !self.password_open.get() && !creating {
                let g = group("", None);
                let more = button("Sign in with a password instead", &["flat"]);
                more.set_halign(gtk::Align::Center);
                g.add(&more);
                self.add(g);
                let weak = Rc::downgrade(self);
                more.connect_clicked(move |_| {
                    if let Some(me) = weak.upgrade() {
                        me.password_open.set(true);
                        me.rebuild();
                        let w = me.w.borrow();
                        if let Some(e) = &w.email {
                            e.grab_focus();
                        }
                    }
                });
                self.w.borrow_mut().password_more = Some(more);
                return;
            }
        }
        let g = group(
            if provider.is_some() {
                "Email and password"
            } else {
                "Account"
            },
            Some(if creating {
                "The first account on an instance is its admin."
            } else if provider.is_some() {
                "Or sign in with email and password."
            } else {
                "Same email and password as on your other devices."
            }),
        );
        let switch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        switch.add_css_class("linked");
        let sign_in = gtk::ToggleButton::with_label("Sign in");
        let create = gtk::ToggleButton::with_label("Create account");
        create.set_group(Some(&sign_in));
        sign_in.set_active(!creating);
        create.set_active(creating);
        switch.append(&sign_in);
        switch.append(&create);
        switch.set_valign(gtk::Align::Center);
        g.set_header_suffix(Some(&switch));

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let email = adw::EntryRow::new();
        email.set_title("Email");
        email.set_input_purpose(gtk::InputPurpose::Email);
        list.append(&email);
        let password = adw::PasswordEntryRow::new();
        password.set_title(if creating {
            "Password — at least 10 characters"
        } else {
            "Password"
        });
        list.append(&password);
        let name = adw::EntryRow::new();
        name.set_title("Name — shown beside your cursor on shared canvases");
        name.set_visible(creating);
        list.append(&name);
        g.add(&list);
        let error = error_label();
        g.add(&error);
        let go = button(
            if creating {
                "Create account"
            } else {
                "Sign in"
            },
            &["suggested-action"],
        );
        g.add(&foot(&[&go]));
        self.add(g);

        let weak = Rc::downgrade(self);
        create.connect_toggled(move |b| {
            if let Some(me) = weak.upgrade()
                && me.creating.get() != b.is_active()
            {
                me.creating.set(b.is_active());
                let keep = me.w.borrow().email.as_ref().map(|e| e.text());
                me.rebuild();
                if let (Some(text), Some(e)) = (keep, me.w.borrow().email.as_ref()) {
                    e.set_text(&text);
                }
            }
        });
        let weak = Rc::downgrade(self);
        go.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.submit_account();
            }
        });
        let weak = Rc::downgrade(self);
        password.connect_entry_activated(move |_| {
            if let Some(me) = weak.upgrade() {
                me.submit_account();
            }
        });
        let mut w = self.w.borrow_mut();
        w.email = Some(email);
        w.password = Some(password);
        w.name = Some(name);
        w.account_button = Some(go);
        w.account_error = Some(error);
    }

    /// The Account step's "Continue with …" (the first provider the linked
    /// instance offers), the domain hint under it, and the row it becomes
    /// while the browser is open.
    fn build_sso_button(self: &Rc<Self>, provider: &copper_cloud::SsoProvider) {
        let g = group(
            "Account",
            Some("Sign in with your company account, in your browser."),
        );
        let area = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let go = button(&provider.button(), &["suggested-action", "pill"]);
        go.set_halign(gtk::Align::Center);
        area.append(&go);
        if let Some(hint) = domain_hint(&provider.domains) {
            let l = label(&hint, &["dim-label", "caption"]);
            l.set_halign(gtk::Align::Center);
            l.set_xalign(0.5);
            area.append(&l);
        }
        g.add(&area);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let (wait, spinner, cancel) = self.waiting_row();
        list.append(&wait);
        // Only there while the browser is open (an empty list draws a line).
        list.set_visible(false);
        g.add(&list);
        let error = error_label();
        g.add(&error);
        self.add(g);
        let id = provider.id.clone();
        let weak = Rc::downgrade(self);
        go.connect_clicked(move |_| {
            let (Some(me), Some(link)) = (weak.upgrade(), get().and_then(|c| c.client.link()))
            else {
                return;
            };
            me.start_sso(
                SsoTarget::Linked {
                    endpoint: link.endpoint,
                    provider: id.clone(),
                },
                None,
            );
        });
        let mut w = self.w.borrow_mut();
        w.sso_area = Some(area);
        w.sso_button = Some(go);
        w.sso_wait = Some(wait);
        w.sso_spinner = Some(spinner);
        w.sso_cancel = Some(cancel);
        w.sso_error = Some(error);
    }

    /// Choose Create account (`true`) or Sign in.
    pub fn set_creating(self: &Rc<Self>, creating: bool) {
        if self.creating.get() != creating {
            self.creating.set(creating);
            self.rebuild();
        }
    }

    /// Fill the account form (the bench).
    pub fn fill_account(&self, email: &str, password: &str, name: &str) {
        let w = self.w.borrow();
        if let Some(e) = &w.email {
            e.set_text(email);
        }
        if let Some(p) = &w.password {
            p.set_text(password);
        }
        if let Some(n) = &w.name {
            n.set_text(name);
        }
    }

    /// Press Sign in / Create account.
    pub fn press_account(&self) {
        let b = self.w.borrow().account_button.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    fn submit_account(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        if self.busy.get() {
            return;
        }
        let (email, password, name) = {
            let w = self.w.borrow();
            (
                w.email
                    .as_ref()
                    .map(|e| e.text().to_string())
                    .unwrap_or_default(),
                w.password
                    .as_ref()
                    .map(|e| e.text().to_string())
                    .unwrap_or_default(),
                w.name
                    .as_ref()
                    .map(|e| e.text().to_string())
                    .unwrap_or_default(),
            )
        };
        let creating = self.creating.get();
        self.set_busy(true);
        show_error(&self.w.borrow().account_error, None);
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        run_then(
            async move {
                if creating {
                    client.sign_up(&email, &password, &name).await
                } else {
                    client.sign_in(&email, &password).await
                }
            },
            move |result| {
                let Some(me) = weak.upgrade() else { return };
                me.set_busy(false);
                match result {
                    Ok(_) => me.update(),
                    Err(f) => show_error(&me.w.borrow().account_error, Some(&f.message)),
                }
            },
        );
    }

    /// Signed in: who, this device's name, Sign out.
    fn build_account(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let Some(account) = cloud.client.account() else {
            return;
        };
        let g = group("Account", None);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let who = plain_row(&account.display_name, &account.email);
        let out = button("Sign out", &[]);
        who.add_suffix(&out);
        list.append(&who);
        let device = adw::EntryRow::new();
        device.set_title("This device — the name your other devices see it by");
        device.set_text(&cloud.client.device_name());
        device.set_show_apply_button(true);
        list.append(&device);
        g.add(&list);
        self.add(g);
        let weak = Rc::downgrade(self);
        device.connect_apply(move |e| {
            let name = e.text().to_string();
            if let (Some(me), Some(cloud)) = (weak.upgrade(), get()) {
                let client = cloud.client.clone();
                let weak = Rc::downgrade(&me);
                run_then(async move { client.rename_device(&name).await }, move |r| {
                    if let Some(me) = weak.upgrade() {
                        if let (Err(f), Some(s)) = (r, me.shell()) {
                            s.toast(&f.message);
                        }
                        me.update();
                    }
                });
            }
        });
        let weak = Rc::downgrade(self);
        out.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.sign_out();
            }
        });
        self.w.borrow_mut().device_name = Some(device);
    }

    /// Sign out: this device's open tabs leave the server first.
    pub fn sign_out(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let sync = cloud.sync.clone();
        let weak = Rc::downgrade(self);
        run_then(async move { sync.sign_out().await }, move |_| {
            if let Some(me) = weak.upgrade() {
                me.update();
            }
        });
    }

    // MARK: Sync

    fn switch_row(d: Domain) -> adw::SwitchRow {
        let r = adw::SwitchRow::new();
        r.set_use_markup(false);
        r.set_title(d.title());
        r.set_subtitle(d.detail());
        r
    }

    /// Signed in, sync off: choose, then Turn on sync.
    fn build_choose(self: &Rc<Self>) {
        let g = group(
            "Sync",
            Some("Pick at least one. Nothing leaves this device until you turn sync on."),
        );
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let mut switches = Vec::new();
        let picked = self.picked.borrow().clone();
        for d in BROWSER {
            let r = Self::switch_row(d);
            r.set_active(picked.contains(&d));
            list.append(&r);
            switches.push((d, r));
        }
        g.add(&list);
        let canvas = group("", None);
        let clist = gtk::ListBox::new();
        clist.add_css_class("boxed-list");
        clist.set_selection_mode(gtk::SelectionMode::None);
        let r = Self::switch_row(Domain::Canvas);
        r.set_active(picked.contains(&Domain::Canvas));
        clist.append(&r);
        switches.push((Domain::Canvas, r));
        let error = error_label();
        let turn_on = button("Turn on sync", &["suggested-action"]);
        canvas.set_title("Canvas");
        canvas.add(&clist);
        canvas.add(&error);
        canvas.add(&foot(&[&turn_on]));
        self.add(g);
        self.add(canvas);
        for (d, row) in &switches {
            let weak = Rc::downgrade(self);
            let d = *d;
            row.connect_active_notify(move |r| {
                if let Some(me) = weak.upgrade() {
                    if r.is_active() {
                        me.picked.borrow_mut().insert(d);
                    } else {
                        me.picked.borrow_mut().remove(&d);
                    }
                    let any = !me.picked.borrow().is_empty();
                    if let Some(b) = &me.w.borrow().turn_on {
                        b.set_sensitive(any && !me.busy.get());
                    }
                }
            });
        }
        turn_on.set_sensitive(!picked.is_empty());
        let weak = Rc::downgrade(self);
        turn_on.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.turn_on();
            }
        });
        let mut w = self.w.borrow_mut();
        w.switches = switches;
        w.turn_on = Some(turn_on);
        w.sync_error = Some(error);
    }

    /// Pick exactly these domains (the bench).
    pub fn pick(&self, domains: &[Domain]) {
        let w = self.w.borrow();
        for (d, row) in &w.switches {
            row.set_active(domains.contains(d));
        }
    }

    /// "Turn on sync" with what is picked.
    pub fn turn_on(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let domains: Vec<Domain> = self.picked.borrow().iter().copied().collect();
        if domains.is_empty() {
            show_error(
                &self.w.borrow().sync_error,
                Some(
                    "Pick at least one. Browser data stays on this device until you turn sync on.",
                ),
            );
            return;
        }
        self.set_busy(true);
        let sync = cloud.sync.clone();
        let weak = Rc::downgrade(self);
        run_then(async move { sync.turn_on(&domains).await }, move |_| {
            if let Some(me) = weak.upgrade() {
                me.set_busy(false);
                me.update();
            }
        });
    }

    /// Press Turn on sync (the bench).
    pub fn press_turn_on(&self) {
        let b = self.w.borrow().turn_on.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// Sync on: the status line with Sync now and Pause.
    fn build_status(self: &Rc<Self>) {
        let g = group("Sync", None);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let row = plain_row("Up to date", "");
        let now = button("Sync now", &[]);
        let pause = button("Pause…", &["flat"]);
        row.add_suffix(&now);
        row.add_suffix(&pause);
        list.append(&row);
        g.add(&list);
        self.add(g);
        now.connect_clicked(|_| super::sync_now());
        let weak = Rc::downgrade(self);
        pause.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.ask_pause();
            }
        });
        self.w.borrow_mut().status = Some(row);
    }

    fn ask_pause(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some("Pause sync?"),
            Some("Pauses browser sync and the Personal canvas. Shared canvases stay live."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("off", "Turn off");
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, r| {
            if r == "off"
                && let Some(me) = weak.upgrade()
            {
                me.pause();
            }
        });
        if let Some(p) = self.page() {
            dialog.present(Some(&p));
        }
    }

    /// Sync off; the switches are remembered.
    pub fn pause(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let sync = cloud.sync.clone();
        let weak = Rc::downgrade(self);
        run_then(async move { sync.turn_off().await }, move |_| {
            if let Some(me) = weak.upgrade() {
                me.update();
            }
        });
    }

    /// Sync on: each switch takes effect at once.
    fn build_switches(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let enabled = cloud.sync.enabled();
        let g = group("Browser sync", None);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let mut switches = Vec::new();
        for d in BROWSER {
            let r = Self::switch_row(d);
            r.set_active(enabled.contains(&d));
            list.append(&r);
            switches.push((d, r));
        }
        g.add(&list);
        self.add(g);
        let canvas = group("Canvas", None);
        let clist = gtk::ListBox::new();
        clist.add_css_class("boxed-list");
        clist.set_selection_mode(gtk::SelectionMode::None);
        let r = Self::switch_row(Domain::Canvas);
        r.set_active(enabled.contains(&Domain::Canvas));
        clist.append(&r);
        switches.push((Domain::Canvas, r));
        canvas.add(&clist);
        self.add(canvas);
        for (d, row) in &switches {
            let d = *d;
            row.connect_active_notify(move |r| {
                let Some(cloud) = get() else { return };
                if cloud.sync.enabled().contains(&d) == r.is_active() {
                    return;
                }
                let sync = cloud.sync.clone();
                let on = r.is_active();
                spawn(async move { sync.set(d, on).await });
                if d == Domain::Canvas {
                    crate::canvas::cloud_changed::<Gtk>();
                }
            });
        }
        self.w.borrow_mut().switches = switches;
    }

    /// "On your other devices".
    fn build_devices(self: &Rc<Self>) {
        let g = group("On your other devices", None);
        self.w.borrow_mut().devices = Some(g.clone());
        self.add(g);
        if let Some(c) = get() {
            let sync = c.sync.clone();
            spawn(async move { sync.refresh_devices().await });
        }
    }

    fn refresh_devices(self: &Rc<Self>, devices: &[copper_cloud::sync::DeviceTabs]) {
        let Some(g) = self.w.borrow().devices.clone() else {
            return;
        };
        for old in self.w.borrow_mut().device_rows.drain(..) {
            g.remove(&old);
        }
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        if devices.is_empty() {
            let r = plain_row(
                "Nothing yet",
                "Tabs open on your other devices show here when they sync Open tabs.",
            );
            r.add_css_class("dim-label");
            list.append(&r);
        }
        for d in devices {
            let row = adw::ExpanderRow::new();
            row.set_use_markup(false);
            row.set_title(&d.name);
            let mut sub = count(d.tabs.len(), "tab", "tabs");
            if let Some(t) = d.updated {
                sub.push_str(&format!(" · Updated {}", ago(t)));
            }
            row.set_subtitle(&sub);
            for t in d.tabs.iter().take(100) {
                let r = plain_row(
                    if t.title.is_empty() { &t.url } else { &t.title },
                    &crate::shell::host_of(&t.url),
                );
                r.set_subtitle_lines(1);
                r.set_title_lines(1);
                r.set_activatable(true);
                let url = t.url.clone();
                let shell = self.shell.clone();
                r.connect_activated(move |_| {
                    if let Some(s) = shell.upgrade() {
                        s.open(
                            &url,
                            OpenTab {
                                foreground: true,
                                ..Default::default()
                            },
                        );
                    }
                });
                row.add_row(&r);
            }
            list.append(&row);
        }
        g.add(&list);
        self.w.borrow_mut().device_rows.push(list.upcast());
    }

    /// The instance in one line, with Details (the fingerprint) and Disconnect.
    fn build_instance(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let status = cloud.client.status();
        let g = group("Instance", None);
        let row = adw::ExpanderRow::new();
        row.set_title(&status.host.clone().unwrap_or_default());
        row.set_subtitle(&certificate_line(cloud, &status));
        self.instance_rows(&row, &status);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&row);
        g.add(&list);
        self.add(g);
    }

    fn build_log(self: &Rc<Self>) {
        let g = group("", None);
        let row = adw::ExpanderRow::new();
        row.set_title("Log");
        let text = label("", &["monospace", "caption"]);
        text.set_selectable(true);
        text.set_margin_top(8);
        text.set_margin_bottom(8);
        text.set_margin_start(12);
        text.set_margin_end(12);
        let holder = gtk::ListBoxRow::new();
        holder.set_activatable(false);
        holder.set_child(Some(&text));
        row.add_row(&holder);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&row);
        g.add(&list);
        self.add(g);
        self.w.borrow_mut().log = Some(text);
    }

    // MARK: Pair another device

    fn build_pairing(self: &Rc<Self>) {
        let g = group(
            "Pair another device",
            Some("Signs another device in as you with one paste. Works once, for 10 minutes."),
        );
        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("card");
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 10);
        inner.set_margin_top(14);
        inner.set_margin_bottom(14);
        inner.set_margin_start(14);
        inner.set_margin_end(14);
        let said = label("Paste this into Settings › Cloud on the other device.", &[]);
        inner.append(&said);
        let code = label("", &["monospace"]);
        code.set_selectable(true);
        code.set_wrap_mode(gtk::pango::WrapMode::Char);
        inner.append(&code);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let left = label("", &["dim-label", "caption"]);
        left.set_hexpand(true);
        let copy = button("Copy", &[]);
        let revoke = button("Revoke", &["flat", "error"]);
        row.append(&left);
        row.append(&revoke);
        row.append(&copy);
        inner.append(&row);
        card.append(&inner);
        card.set_visible(false);
        g.add(&card);
        let make = button("Make a code", &[]);
        g.add(&foot(&[&make]));
        self.add(g);

        let weak = Rc::downgrade(self);
        make.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.make_code();
            }
        });
        let weak = Rc::downgrade(self);
        copy.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                let link = me.pairing.borrow().as_ref().map(|p| p.minted.link.clone());
                if let (Some(link), Some(p)) = (link, me.page()) {
                    p.clipboard().set_text(&link);
                    if let Some(s) = me.shell() {
                        s.toast("Pairing code copied");
                    }
                }
            }
        });
        let weak = Rc::downgrade(self);
        revoke.connect_clicked(move |_| {
            if let Some(me) = weak.upgrade() {
                me.ask_revoke();
            }
        });
        let mut w = self.w.borrow_mut();
        w.pair_box = Some(card);
        w.pair_code = Some(code);
        w.pair_left = Some(left);
        w.pair_make = Some(make);
        w.pair_revoke = Some(revoke);
        w.pair_copy = Some(copy);
        w.pair_said = Some(said);
        drop(w);
        self.draw_pairing();
    }

    /// Make a code (a new one revokes the one shown).
    pub fn make_code(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        if !cloud.client.is_signed_in() {
            return;
        }
        let old = self
            .pairing
            .borrow()
            .as_ref()
            .filter(|p| p.outcome.is_none())
            .map(|p| p.minted.id.clone());
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        self.set_busy(true);
        run_then(
            async move {
                if let Some(id) = old {
                    let _ = client.revoke_pairing_code(&id).await;
                }
                client.mint_pairing_code(None).await
            },
            move |r| {
                let Some(me) = weak.upgrade() else { return };
                me.set_busy(false);
                match r {
                    Ok(minted) => {
                        let expires = parse_stamp(&minted.expires_at)
                            .unwrap_or_else(|| copper_core::date::now_unix() + 600.0);
                        me.pairing.replace(Some(Pairing {
                            minted,
                            expires,
                            outcome: None,
                        }));
                        me.draw_pairing();
                    }
                    Err(f) => {
                        if let Some(s) = me.shell() {
                            s.toast(&f.message);
                        }
                    }
                }
            },
        );
    }

    /// The code on the card now (the bench).
    pub fn code_text(&self) -> Option<String> {
        self.pairing
            .borrow()
            .as_ref()
            .map(|p| p.minted.link.clone())
    }

    /// What the pairing card says (the bench).
    pub fn pairing_said(&self) -> String {
        self.w
            .borrow()
            .pair_said
            .as_ref()
            .filter(|l| l.is_visible() && l.is_mapped())
            .map(|l| l.text().to_string())
            .unwrap_or_default()
    }

    // MARK: for the bench

    /// Scroll the page to `fraction` (0 top … 1 bottom).
    pub fn scroll_to(&self, fraction: f64) {
        let Some(page) = self.page() else { return };
        let mut stack: Vec<gtk::Widget> = vec![page.upcast()];
        while let Some(w) = stack.pop() {
            if let Some(sw) = w.downcast_ref::<gtk::ScrolledWindow>() {
                let adj = sw.vadjustment();
                let max = (adj.upper() - adj.page_size()).max(0.0);
                adj.set_value(max * fraction.clamp(0.0, 1.0));
                return;
            }
            let mut child = w.first_child();
            while let Some(c) = child {
                child = c.next_sibling();
                stack.push(c);
            }
        }
    }

    /// Type into the Connect field.
    pub fn type_code(&self, text: &str) {
        let code = self.w.borrow().code.clone();
        if let Some(c) = code {
            c.set_text(text);
        }
    }

    /// What the page says about the pasted code.
    pub fn said_text(&self) -> Option<String> {
        let w = self.w.borrow();
        let row = w.said.as_ref().filter(|r| r.is_visible())?;
        Some(format!(
            "{} — {}",
            row.title(),
            row.subtitle().map(|s| s.to_string()).unwrap_or_default()
        ))
    }

    /// Press Connect / Pair this device.
    pub fn press_connect(&self) {
        let b = self.w.borrow().connect.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// The fingerprint offered for trust, while the trust box shows.
    pub fn trust_shown(&self) -> Option<String> {
        let w = self.w.borrow();
        w.trust.as_ref().filter(|t| t.is_visible())?;
        w.trust_fp.as_ref().map(|l| l.text().to_string())
    }

    /// Press Trust and connect / Trust and pair.
    pub fn press_trust(&self) {
        let b = self.w.borrow().trust_button.clone();
        if let Some(b) = b {
            b.emit_clicked();
        }
    }

    /// The problem the page shows, if any.
    pub fn error_text(&self) -> Option<String> {
        let w = self.w.borrow();
        [
            &w.connect_error,
            &w.sso_error,
            &w.account_error,
            &w.sync_error,
        ]
        .into_iter()
        .flatten()
        .find(|l| l.is_visible() && !l.text().is_empty())
        .map(|l| l.text().to_string())
    }

    fn ask_revoke(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some("Revoke this pairing code?"),
            Some("Nobody will be able to use it. A device already paired with it stays signed in."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("revoke", "Revoke");
        dialog.set_response_appearance("revoke", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, r| {
            if r == "revoke"
                && let Some(me) = weak.upgrade()
            {
                me.revoke();
            }
        });
        if let Some(p) = self.page() {
            dialog.present(Some(&p));
        }
    }

    /// Revoke the code on the card.
    pub fn revoke(self: &Rc<Self>) {
        let Some(cloud) = get() else { return };
        let Some(id) = self.pairing.borrow().as_ref().map(|p| p.minted.id.clone()) else {
            return;
        };
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        run_then(
            async move { client.revoke_pairing_code(&id).await },
            move |r| {
                let Some(me) = weak.upgrade() else { return };
                if let Some(p) = me.pairing.borrow_mut().as_mut() {
                    p.outcome = Some(false);
                }
                if let (Err(f), Some(s)) = (r, me.shell()) {
                    s.toast(&f.message);
                }
                me.draw_pairing();
            },
        );
    }

    fn draw_pairing(&self) {
        let w = self.w.borrow();
        let (Some(card), Some(code), Some(left), Some(make), Some(revoke), Some(copy), Some(said)) = (
            &w.pair_box,
            &w.pair_code,
            &w.pair_left,
            &w.pair_make,
            &w.pair_revoke,
            &w.pair_copy,
            &w.pair_said,
        ) else {
            return;
        };
        let p = self.pairing.borrow();
        let Some(p) = p.as_ref() else {
            card.set_visible(false);
            make.set_label("Make a code");
            return;
        };
        card.set_visible(true);
        let now = copper_core::date::now_unix();
        let remaining = p.expires - now;
        match p.outcome {
            Some(true) => {
                said.set_text("Used — the other device is signed in");
                code.set_visible(false);
                left.set_text("Make another code for the next device.");
                revoke.set_visible(false);
                copy.set_visible(false);
                make.set_label("Make a code");
            }
            Some(false) => {
                said.set_text("Nobody can use it now.");
                code.set_visible(false);
                left.set_text("Make a new one when the other device is ready.");
                revoke.set_visible(false);
                copy.set_visible(false);
                make.set_label("Make a code");
            }
            None if remaining <= 0.0 => {
                said.set_text("This code has expired.");
                code.set_visible(false);
                left.set_text("Make a new one when the other device is ready.");
                revoke.set_visible(false);
                copy.set_visible(false);
                make.set_label("Make a code");
            }
            None => {
                said.set_text("Paste this into Settings › Cloud on the other device.");
                code.set_visible(true);
                code.set_text(&split_code(&p.minted.link));
                let secs = remaining.ceil() as i64;
                left.set_text(&format!(
                    "Expires in {}:{:02} · works once",
                    secs / 60,
                    secs % 60
                ));
                revoke.set_visible(true);
                copy.set_visible(true);
                make.set_label("Make a new code");
            }
        }
    }

    /// Once a second: the countdown; every third second, whether the code
    /// was used (it leaves the list of unused codes before it expires).
    fn tick_pairing(self: &Rc<Self>, check: bool) {
        let waiting = self
            .pairing
            .borrow()
            .as_ref()
            .filter(|p| p.outcome.is_none())
            .map(|p| (p.minted.id.clone(), p.expires));
        let Some((id, expires)) = waiting else { return };
        self.draw_pairing();
        if !check || copper_core::date::now_unix() >= expires {
            return;
        }
        let Some(cloud) = get() else { return };
        let client = cloud.client.clone();
        let weak = Rc::downgrade(self);
        run_then(async move { client.pairing_codes().await }, move |r| {
            let Some(me) = weak.upgrade() else { return };
            let Ok(list) = r else { return };
            let open = list
                .iter()
                .any(|c| c.id.eq_ignore_ascii_case(&id) && c.used_at.is_none());
            if !open
                && let Some(p) = me.pairing.borrow_mut().as_mut()
                && p.minted.id == id
                && p.outcome.is_none()
            {
                p.outcome = Some(true);
            }
            me.draw_pairing();
        });
    }
}

/// The code on three lines: the address, the `cp_…` part, the fingerprint.
fn split_code(link: &str) -> String {
    let Some((head, frag)) = link.split_once('#') else {
        return link.to_string();
    };
    let parts: Vec<&str> = frag.split('&').collect();
    format!("{head}#\n{}", parts.join("\n&"))
}

/// `HH:MM:SS`, local wall clock is not known here: UTC.
fn clock(unix: f64) -> String {
    let secs = unix as i64;
    let day = secs.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}Z",
        day / 3600,
        (day % 3600) / 60,
        day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_on_lines() {
        assert_eq!(
            split_code("copper-cloud://h:8443/#p=cp_x&fp=ab"),
            "copper-cloud://h:8443/#\np=cp_x\n&fp=ab"
        );
        assert_eq!(short_fp("aa:bb:cc:dd:ee", 4), "aa:bb:cc:dd…");
        assert_eq!(clock(3661.0), "01:01:01Z");
    }
}
