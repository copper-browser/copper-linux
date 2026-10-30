//! Settings › Passwords: saving and filling, the keyring's saved sign-ins
//! (search, add, copy, show after a confirmation, delete), sites never asked
//! about, Bitwarden, 1Password and BotPass, agent access, bringing passwords
//! in from Chrome, and a passwords CSV in and out (Mac `Passwords.swift`,
//! `Fork/CredentialsSettings.swift`, `Fork/Credentials/PasswordCSV.swift`).

use super::vault::{Keyring, Snapshot};
use super::{keys, rt, source_title};
use crate::shell::{GtkShell, Shell, WeakShell};
use copper_core::settings::keys as core_keys;
use copper_creds::Login;
use copper_creds::bitwarden::BwState;
use copper_creds::credential::{Credential, Source};
use copper_creds::onepassword::Mode;
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Open Settings on this page.
pub fn present(shell: &Shell) {
    let d = crate::window::settings::dialog(shell);
    d.set_visible_page_name("passwords");
}

struct Ui {
    shell: WeakShell,
    page: glib::WeakRef<adw::PreferencesPage>,
    keyring: adw::PreferencesGroup,
    saved: adw::PreferencesGroup,
    /// "Add a password", first in Saved passwords while a keyring is there.
    add: adw::ExpanderRow,
    never: adw::PreferencesGroup,
    bw: adw::PreferencesGroup,
    op: adw::PreferencesGroup,
    bp: adw::PreferencesGroup,
    agents: adw::PreferencesGroup,
    /// Rows added per group, so a rebuild takes exactly those out.
    rows: RefCell<HashMap<&'static str, Vec<gtk::Widget>>>,
    logins: RefCell<Vec<Login>>,
    search: RefCell<String>,
    /// Passwords shown after a confirmation, until hidden or the page goes.
    shown: RefCell<HashMap<(String, String), String>>,
    snap: RefCell<Snapshot>,
    busy: std::cell::Cell<bool>,
}

fn put(ui: &Ui, key: &'static str, group: &adw::PreferencesGroup, rows: Vec<gtk::Widget>) {
    let mut all = ui.rows.borrow_mut();
    for old in all.remove(key).unwrap_or_default() {
        group.remove(&old);
    }
    for r in &rows {
        group.add(r);
    }
    all.insert(key, rows);
}

fn say(ui: &Ui, text: &str) {
    if let Some(p) = ui.page.upgrade()
        && let Some(d) = p
            .ancestor(adw::PreferencesDialog::static_type())
            .and_downcast::<adw::PreferencesDialog>()
    {
        d.add_toast(adw::Toast::new(text));
        return;
    }
    if let Some(s) = ui.shell.upgrade() {
        s.toast(text);
    }
}

fn row(title: &str, subtitle: &str) -> adw::ActionRow {
    let r = adw::ActionRow::new();
    r.set_title(&glib::markup_escape_text(title));
    if !subtitle.is_empty() {
        r.set_subtitle(&glib::markup_escape_text(subtitle));
    }
    r
}

fn pill(label: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.set_valign(gtk::Align::Center);
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_valign(gtk::Align::Center);
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    b.update_property(&[gtk::accessible::Property::Label(tip)]);
    b
}

/// Run `work` on the passwords runtime with the page marked busy, then
/// toast its error (if any) and rebuild.
fn act<F, Fut>(ui: &Rc<Ui>, work: F)
where
    F: FnOnce(std::sync::Arc<super::vault::Vault>) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    act_saying(ui, None, work);
}

/// [`act`], toasting `done` when the work went through.
fn act_saying<F, Fut>(ui: &Rc<Ui>, done: Option<String>, work: F)
where
    F: FnOnce(std::sync::Arc<super::vault::Vault>) -> Fut + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let Some(v) = super::vault() else { return };
    if ui.busy.replace(true) {
        return;
    }
    let weak = Rc::downgrade(ui);
    let fut = work(v.clone());
    glib::spawn_future_local(async move {
        let r = rt::run(async move {
            let r = fut.await;
            v.refresh().await;
            r
        })
        .await
        .unwrap_or_else(|| Err("Copper couldn't finish that".into()));
        let Some(ui) = weak.upgrade() else { return };
        ui.busy.set(false);
        match (r, done) {
            (Err(e), _) => say(&ui, &first_line(&e)),
            (Ok(()), Some(d)) => say(&ui, &d),
            (Ok(()), None) => {}
        }
        refresh(&ui);
    });
}

fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(s)
        .trim()
        .to_string()
}

pub fn page(shell: &Shell) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::new();
    page.set_title("Passwords");
    page.set_icon_name(Some("dialog-password-symbolic"));

    // — saving and filling —
    let behaviour = adw::PreferencesGroup::new();
    let save = adw::SwitchRow::new();
    save.set_title("Offer to save passwords");
    save.set_subtitle("Asked once, after a sign-in works");
    save.set_active(shell.settings().saves_passwords());
    let weak = shell.downgrade();
    save.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            let on = r.is_active();
            s.update_settings(|st| st.set(core_keys::PASSWORDS_SAVE, on));
        }
    });
    behaviour.add(&save);
    let fill = adw::SwitchRow::new();
    fill.set_title("Offer saved accounts in sign-in fields");
    fill.set_subtitle("A list under the field; nothing is filled until you pick one");
    fill.set_active(shell.settings().fills_passwords());
    let weak = shell.downgrade();
    fill.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            let on = r.is_active();
            s.update_settings(|st| st.set(core_keys::PASSWORDS_FILL, on));
        }
    });
    behaviour.add(&fill);
    // Mac `Settings.swift` passwords: "Fill two-step codes", "Fill
    // addresses and cards".
    behaviour.add(&switch(
        shell,
        "Fill two-step codes",
        "Right after you pick an account whose code Copper keeps, the code page fills itself",
        shell.settings().fills_codes(),
        core_keys::PASSWORDS_CODES,
    ));
    behaviour.add(&switch(
        shell,
        "Fill addresses and cards",
        "Click into a checkout or address form and your Bitwarden and 1Password identities and cards hang from it",
        shell.settings().fills_everything(),
        core_keys::AUTOFILL_EVERYTHING,
    ));
    let target = adw::ComboRow::new();
    target.set_title("Save new passwords to");
    target.set_subtitle("Bitwarden or 1Password while unlocked, else the keyring");
    target.set_model(Some(&gtk::StringList::new(&[
        "Keyring",
        "Bitwarden",
        "1Password",
    ])));
    target.set_selected(match super::chosen_backend(shell) {
        Source::Keychain | Source::BotPass => 0,
        Source::Bitwarden => 1,
        Source::OnePassword => 2,
    });
    let weak = shell.downgrade();
    target.connect_selected_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            super::set_backend(
                &s,
                match r.selected() {
                    1 => Source::Bitwarden,
                    2 => Source::OnePassword,
                    _ => Source::Keychain,
                },
            );
        }
    });
    behaviour.add(&target);
    let bring = row(
        "Bring in from Chrome, Chromium or Brave",
        "Bookmarks, history, open tabs and passwords",
    );
    let go = pill("Bring In…", false);
    let weak = shell.downgrade();
    go.connect_clicked(move |_| {
        if let Some(s) = weak.upgrade() {
            super::import::present(&s);
        }
    });
    bring.add_suffix(&go);
    bring.set_activatable_widget(Some(&go));
    behaviour.add(&bring);
    page.add(&behaviour);

    let keyring = adw::PreferencesGroup::new();
    keyring.set_title("Keyring");
    page.add(&keyring);

    let saved = adw::PreferencesGroup::new();
    saved.set_title("Saved passwords");
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search"));
    search.set_widget_name("copper-passwords-search");
    search.set_valign(gtk::Align::Center);
    search.update_property(&[gtk::accessible::Property::Label("Search saved passwords")]);
    saved.set_header_suffix(Some(&search));
    let add = adw::ExpanderRow::new();
    add.set_title("Add a password");
    add.set_subtitle("Type one in by hand: the site, the name and the password");
    saved.add(&add);
    page.add(&saved);

    let never = adw::PreferencesGroup::new();
    never.set_title("Never offered on");
    page.add(&never);

    let bw = adw::PreferencesGroup::new();
    bw.set_title("Bitwarden");
    page.add(&bw);
    let op = adw::PreferencesGroup::new();
    op.set_title("1Password");
    page.add(&op);
    let bp = adw::PreferencesGroup::new();
    bp.set_title("BotPass");
    bp.set_description(Some(
        "Copper as one of your BotPass bots: its accounts are offered on the sites BotPass grants that bot, and fetched only when filled. Agents can sign in with them; BotPass's grants decide.",
    ));
    page.add(&bp);
    let agents = adw::PreferencesGroup::new();
    agents.set_title("Agent access");
    agents.set_description(Some(
        "Agents can sign in with an account only when you share it here. The password is filled inside Copper and never handed to the agent.",
    ));
    page.add(&agents);

    // Mac `PasswordCSVLines`: a CSV in, a CSV out.
    let csv = adw::PreferencesGroup::new();
    csv.set_title("Import and export");
    let from = row(
        "From a CSV file",
        "A passwords export from Chrome, Safari, Apple Passwords or a password manager",
    );
    let choose = pill("Choose File…", false);
    from.add_suffix(&choose);
    from.set_activatable_widget(Some(&choose));
    csv.add(&from);
    let to = row(
        "Export to a CSV file",
        "Every password in the keyring, after a confirmation. Anyone with the file can read them — delete it when you're done.",
    );
    to.set_subtitle_lines(0);
    let export = pill("Export…", false);
    to.add_suffix(&export);
    to.set_activatable_widget(Some(&export));
    csv.add(&to);
    page.add(&csv);

    let pw = glib::WeakRef::new();
    pw.set(Some(&page));
    let ui = Rc::new(Ui {
        shell: shell.downgrade(),
        page: pw,
        keyring,
        saved,
        add,
        never,
        bw,
        op,
        bp,
        agents,
        rows: RefCell::default(),
        logins: RefCell::default(),
        search: RefCell::default(),
        shown: RefCell::default(),
        snap: RefCell::new(super::vault().map(|v| v.snapshot()).unwrap_or_default()),
        busy: std::cell::Cell::new(false),
    });
    {
        let weak = Rc::downgrade(&ui);
        search.connect_search_changed(move |e| {
            if let Some(ui) = weak.upgrade() {
                *ui.search.borrow_mut() = e.text().to_string();
                build_saved(&ui);
            }
        });
    }
    build_add(&ui);
    {
        let weak = Rc::downgrade(&ui);
        choose.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                import_csv(&ui);
            }
        });
        let weak = Rc::downgrade(&ui);
        export.connect_clicked(move |b| {
            if let Some(ui) = weak.upgrade() {
                export_csv(&ui, b.upcast_ref());
            }
        });
    }
    // The page keeps its state alive; the state never keeps the page.
    let keep = ui.clone();
    page.connect_destroy(move |_| keep.shown.borrow_mut().clear());
    build_all(&ui);
    refresh(&ui);
    page
}

/// Re-read the keyring and the vaults, then rebuild.
fn refresh(ui: &Rc<Ui>) {
    let Some(v) = super::vault() else { return };
    let weak = Rc::downgrade(ui);
    glib::spawn_future_local(async move {
        if !v.keyring_open() {
            v.note_probe(v.probe(true).await);
        }
        let vv = v.clone();
        let logins = rt::run(async move {
            // Is BotPass there, and locked? (No token, no secret.)
            vv.bp.refresh().await;
            vv.refresh().await;
            vv.logins().await
        })
        .await
        .unwrap_or_else(|| Err("no answer".into()));
        let Some(ui) = weak.upgrade() else { return };
        *ui.logins.borrow_mut() = logins.unwrap_or_default();
        *ui.snap.borrow_mut() = v.snapshot();
        if let Some(s) = ui.shell.upgrade() {
            super::remember_op(&s, &ui.snap.borrow().op_remembered);
        }
        build_all(&ui);
        build_agents(&ui);
    });
}

fn build_all(ui: &Rc<Ui>) {
    build_keyring(ui);
    build_saved(ui);
    build_never(ui);
    build_bw(ui);
    build_op(ui);
    build_bp(ui);
}

fn build_keyring(ui: &Rc<Ui>) {
    let k = ui.snap.borrow().keyring.clone();
    let (title, sub, icon) = match &k {
        Keyring::Ready => (
            "Saved in your login keyring".to_string(),
            "Through the Secret Service (GNOME Keyring, KWallet or KeePassXC), as Copper items".to_string(),
            "emblem-ok-symbolic",
        ),
        Keyring::Unknown => ("Looking for the keyring…".to_string(), String::new(), "content-loading-symbolic"),
        Keyring::Closed { has_default: Some(false), .. } => (
            "No keyring yet".to_string(),
            "Copper keeps passwords in the Secret Service, which has no default keyring in this session. Set one up now, or Copper asks the first time you save a password.".to_string(),
            "dialog-password-symbolic",
        ),
        Keyring::Closed { locked: false, .. } => (
            "Saved in your login keyring".to_string(),
            "Through the Secret Service (GNOME Keyring, KWallet or KeePassXC), opened when Copper first needs it".to_string(),
            "emblem-ok-symbolic",
        ),
        Keyring::Locked | Keyring::Closed { locked: true, .. } => (
            "The keyring is locked".to_string(),
            "Unlock it from your desktop (it usually opens with your login), then try again".to_string(),
            "system-lock-screen-symbolic",
        ),
        Keyring::Unavailable(_) => (
            "No keyring is available".to_string(),
            "Copper keeps passwords in the Secret Service, and none is running in this session — so nothing is saved or offered from it. Start GNOME Keyring, KWallet or KeePassXC (with Secret Service on), then try again. Bitwarden and 1Password still work.".to_string(),
            "dialog-warning-symbolic",
        ),
    };
    let r = row(&title, &sub);
    r.set_subtitle_lines(0);
    if let Keyring::Unavailable(why) = &k {
        r.set_tooltip_text(Some(&first_line(why)));
    }
    r.add_prefix(&gtk::Image::from_icon_name(icon));
    // Opening the keyring may ask for a password (to create or unlock it):
    // only ever on this click, never by just showing the page.
    let open = match &k {
        Keyring::Closed {
            has_default: Some(false),
            ..
        } => Some("Set Up…"),
        Keyring::Locked | Keyring::Closed { locked: true, .. } => Some("Unlock…"),
        _ => None,
    };
    if let Some(label) = open {
        let b = pill(label, false);
        let weak = Rc::downgrade(ui);
        b.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                act(&ui, |v| async move {
                    v.store().await.map_err(|e| e.to_string())?;
                    v.logins().await.map(|_| ())
                });
            }
        });
        r.add_suffix(&b);
    } else if !matches!(k, Keyring::Ready | Keyring::Closed { .. }) {
        let again = pill("Try Again", false);
        let weak = Rc::downgrade(ui);
        again.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                refresh(&ui);
            }
        });
        r.add_suffix(&again);
    }
    put(ui, "keyring", &ui.keyring, vec![r.upcast()]);
}

/// A switch row for a setting.
fn switch(
    shell: &Shell,
    title: &str,
    subtitle: &str,
    on: bool,
    key: &'static str,
) -> adw::SwitchRow {
    let r = adw::SwitchRow::new();
    r.set_title(title);
    r.set_subtitle(subtitle);
    r.set_active(on);
    let weak = shell.downgrade();
    r.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            let on = r.is_active();
            s.update_settings(|st| st.set(key, on));
        }
    });
    r
}

/// The add form (Mac `Passwords.swift` `AddForm`): site, username,
/// password, Save "Kept for <site>".
fn build_add(ui: &Rc<Ui>) {
    let site = entry("Site", "");
    site.set_input_purpose(gtk::InputPurpose::Url);
    let user = entry("Username", "");
    let password = password_entry("Password");
    let go = row("", "");
    let save = pill("Save", true);
    go.add_suffix(&save);
    let keep = {
        let weak = Rc::downgrade(ui);
        let (site, user, password) = (site.clone(), user.clone(), password.clone());
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let host = copper_creds::site::host_of(&site.text());
            let pw = password.text().to_string();
            if host.is_empty() || pw.is_empty() {
                say(&ui, "A site and a password, both");
                return;
            }
            let who = user.text().trim().to_string();
            for e in [&site, &user] {
                e.set_text("");
            }
            password.set_text("");
            ui.add.set_expanded(false);
            let said = format!("Kept for {host}");
            act_saying(&ui, Some(said), move |v| async move {
                v.save(Source::Keychain, &host, &who, &pw).await
            });
        }
    };
    let keep = Rc::new(keep);
    let k = keep.clone();
    save.connect_clicked(move |_| k());
    password.connect_entry_activated(move |_| keep());
    ui.add.add_row(&site);
    ui.add.add_row(&user);
    ui.add.add_row(&password);
    ui.add.add_row(&go);
}

/// The keyring, mid-sentence.
const STORE: &str = "the keyring";

/// Ask for a passwords CSV and keep its logins in the keyring.
fn import_csv(ui: &Rc<Ui>) {
    let Some(shell) = ui.shell.upgrade() else {
        return;
    };
    let dialog = gtk::FileDialog::new();
    dialog.set_title("A passwords export — from Chrome, Firefox or a password manager");
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("CSV files"));
    filter.add_suffix("csv");
    filter.add_mime_type("text/csv");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    dialog.set_filters(Some(&filters));
    let weak = Rc::downgrade(ui);
    dialog.open(Some(shell.window()), gio::Cancellable::NONE, move |r| {
        let (Some(ui), Ok(f)) = (weak.upgrade(), r) else {
            return;
        };
        let Some(path) = f.path() else { return };
        say(&ui, "Importing passwords…");
        let weak = Rc::downgrade(&ui);
        glib::spawn_future_local(async move {
            let (said, _) = copper_shell_core::forms::csv::import_file(path, STORE).await;
            if let Some(ui) = weak.upgrade() {
                say(&ui, &said);
                refresh(&ui);
            }
        });
    });
}

/// Ask for a yes, then where, then write every password as CSV.
fn export_csv(ui: &Rc<Ui>, from: &gtk::Widget) {
    use copper_shell_core::forms::csv;
    let d = adw::AlertDialog::new(Some(csv::EXPORT_TITLE), Some(csv::EXPORT_DETAIL));
    d.add_responses(&[("cancel", "Cancel"), ("export", csv::EXPORT_ALLOW)]);
    d.set_response_appearance("export", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    let weak = Rc::downgrade(ui);
    d.connect_response(None, move |_, resp| {
        let ("export", Some(ui)) = (resp, weak.upgrade()) else {
            return;
        };
        let Some(shell) = ui.shell.upgrade() else {
            return;
        };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Export passwords");
        dialog.set_initial_name(Some(csv::EXPORT_NAME));
        let weak = Rc::downgrade(&ui);
        dialog.save(Some(shell.window()), gio::Cancellable::NONE, move |r| {
            let (Some(ui), Ok(f)) = (weak.upgrade(), r) else {
                return;
            };
            let Some(path) = f.path() else { return };
            let weak = Rc::downgrade(&ui);
            glib::spawn_future_local(async move {
                let (said, _) = csv::export_file(path, STORE).await;
                if let Some(ui) = weak.upgrade() {
                    say(&ui, &said);
                }
            });
        });
    });
    d.present(Some(from));
}

fn build_saved(ui: &Rc<Ui>) {
    let ready = ui.snap.borrow().keyring_ready();
    if let Some(search) = ui.saved.header_suffix() {
        search.set_visible(ready);
    }
    ui.add.set_visible(ready);
    let needle = ui.search.borrow().trim().to_lowercase();
    let logins: Vec<Login> = ui
        .logins
        .borrow()
        .iter()
        .filter(|l| {
            needle.is_empty()
                || l.host.to_lowercase().contains(&needle)
                || l.user.to_lowercase().contains(&needle)
        })
        .cloned()
        .collect();
    let mut rows: Vec<gtk::Widget> = Vec::new();
    let total = ui.logins.borrow().len();
    ui.saved.set_description(Some(&match total {
        0 => String::new(),
        1 => "1 password".to_string(),
        n => format!("{n} passwords"),
    }));
    if !ready {
        rows.push(
            row(
                "Nothing to show",
                "Saved passwords appear here once a keyring is available",
            )
            .upcast(),
        );
    } else if logins.is_empty() {
        rows.push(
            row(
                if total == 0 {
                    "No saved passwords yet"
                } else {
                    "Nothing matches"
                },
                if total == 0 {
                    "Copper offers to save one after you sign in somewhere"
                } else {
                    ""
                },
            )
            .upcast(),
        );
    }
    for l in logins.iter().take(400) {
        rows.push(login_row(ui, l).upcast());
    }
    put(ui, "saved", &ui.saved, rows);
}

fn login_row(ui: &Rc<Ui>, l: &Login) -> adw::ActionRow {
    let user = if l.user.is_empty() {
        "No username"
    } else {
        l.user.as_str()
    };
    let r = row(&l.host, user);
    r.set_title_lines(1);
    let key = (l.host.clone(), l.user.clone());
    let shown = ui.shown.borrow().get(&key).cloned();
    if let Some(secret) = shown {
        let label = gtk::Label::new(Some(&secret));
        label.add_css_class("copper-forms-secret");
        label.set_selectable(true);
        label.set_valign(gtk::Align::Center);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        label.set_max_width_chars(24);
        r.add_suffix(&label);
        let hide = icon_button("view-conceal-symbolic", "Hide password");
        let weak = Rc::downgrade(ui);
        let k = key.clone();
        hide.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.shown.borrow_mut().remove(&k);
                build_saved(&ui);
            }
        });
        r.add_suffix(&hide);
    } else {
        let dots = gtk::Label::new(Some("••••••••"));
        dots.add_css_class("dim-label");
        dots.set_valign(gtk::Align::Center);
        r.add_suffix(&dots);
        let reveal = icon_button("view-reveal-symbolic", "Show password");
        let weak = Rc::downgrade(ui);
        let l2 = l.clone();
        reveal.connect_clicked(move |b| {
            if let Some(ui) = weak.upgrade() {
                confirm_reveal(&ui, b.upcast_ref(), &l2);
            }
        });
        r.add_suffix(&reveal);
    }
    let copy = icon_button("edit-copy-symbolic", "Copy password");
    let weak = Rc::downgrade(ui);
    let l2 = l.clone();
    copy.connect_clicked(move |b| {
        if let Some(ui) = weak.upgrade() {
            copy_password(&ui, b.upcast_ref(), &l2);
        }
    });
    r.add_suffix(&copy);
    let delete = icon_button("user-trash-symbolic", "Delete password");
    let weak = Rc::downgrade(ui);
    let l2 = l.clone();
    delete.connect_clicked(move |b| {
        if let Some(ui) = weak.upgrade() {
            confirm_delete(&ui, b.upcast_ref(), &l2);
        }
    });
    r.add_suffix(&delete);
    r
}

/// The password onto the clipboard (Mac `Browser.copy`): "Password copied".
fn copy_password(ui: &Rc<Ui>, from: &gtk::Widget, l: &Login) {
    let Some(v) = super::vault() else { return };
    let weak = Rc::downgrade(ui);
    let clipboard = from.clipboard();
    let id = copper_creds::CredentialId::Keychain {
        host: l.host.clone(),
        user: l.user.clone(),
    };
    glib::spawn_future_local(async move {
        let got = rt::run(async move { v.secret(&id).await })
            .await
            .unwrap_or_else(|| Err("no answer".into()));
        let Some(ui) = weak.upgrade() else { return };
        match got {
            Ok(secret) => {
                clipboard.set_text(&secret);
                say(&ui, "Password copied");
            }
            Err(_) => say(&ui, "The keyring didn't give up that password"),
        }
    });
}

fn confirm_reveal(ui: &Rc<Ui>, from: &gtk::Widget, l: &Login) {
    let d = adw::AlertDialog::new(
        Some("Show this password?"),
        Some(&format!(
            "The password for {} on {} will be shown on screen until you hide it.",
            if l.user.is_empty() {
                "this account"
            } else {
                &l.user
            },
            l.host
        )),
    );
    d.add_responses(&[("cancel", "Cancel"), ("show", "Show")]);
    d.set_response_appearance("show", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    let weak = Rc::downgrade(ui);
    let l = l.clone();
    d.connect_response(None, move |_, resp| {
        if resp != "show" {
            return;
        }
        let Some(v) = super::vault() else { return };
        let weak = weak.clone();
        let (host, user) = (l.host.clone(), l.user.clone());
        glib::spawn_future_local(async move {
            let id = copper_creds::CredentialId::Keychain {
                host: host.clone(),
                user: user.clone(),
            };
            let got = rt::run(async move { v.secret(&id).await })
                .await
                .unwrap_or_else(|| Err("no answer".into()));
            let Some(ui) = weak.upgrade() else { return };
            match got {
                Ok(secret) => {
                    ui.shown.borrow_mut().insert((host, user), secret);
                    build_saved(&ui);
                }
                Err(e) => say(&ui, &first_line(&e)),
            }
        });
    });
    d.present(Some(from));
}

fn confirm_delete(ui: &Rc<Ui>, from: &gtk::Widget, l: &Login) {
    let d = adw::AlertDialog::new(
        Some("Delete this password?"),
        Some(&format!(
            "The saved password for {} on {} is removed from the keyring. This can't be undone.",
            if l.user.is_empty() {
                "this account"
            } else {
                &l.user
            },
            l.host
        )),
    );
    d.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    d.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    d.set_default_response(Some("cancel"));
    d.set_close_response("cancel");
    let weak = Rc::downgrade(ui);
    let l = l.clone();
    d.connect_response(None, move |_, resp| {
        if resp != "delete" {
            return;
        }
        let Some(ui) = weak.upgrade() else { return };
        ui.shown
            .borrow_mut()
            .remove(&(l.host.clone(), l.user.clone()));
        let (host, user) = (l.host.clone(), l.user.clone());
        act(&ui, move |v| async move { v.forget(&host, &user).await });
    });
    d.present(Some(from));
}

fn build_never(ui: &Rc<Ui>) {
    let Some(shell) = ui.shell.upgrade() else {
        return;
    };
    let list = super::never_list(&shell);
    ui.never.set_visible(!list.is_empty());
    let mut rows = Vec::new();
    for host in list {
        let r = row(&host, "");
        let b = pill("Remove", false);
        let weak = Rc::downgrade(ui);
        let h = host.clone();
        b.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade()
                && let Some(s) = ui.shell.upgrade()
            {
                super::set_never(&s, &h, false);
                build_never(&ui);
            }
        });
        r.add_suffix(&b);
        rows.push(r.upcast());
    }
    put(ui, "never", &ui.never, rows);
}

// MARK: Bitwarden

fn entry(title: &str, text: &str) -> adw::EntryRow {
    let e = adw::EntryRow::new();
    e.set_title(title);
    e.set_text(text);
    e
}

fn password_entry(title: &str) -> adw::PasswordEntryRow {
    let e = adw::PasswordEntryRow::new();
    e.set_title(title);
    e
}

fn build_bw(ui: &Rc<Ui>) {
    let snap = ui.snap.borrow().clone();
    let mut rows: Vec<gtk::Widget> = Vec::new();
    match snap.bw_state.clone() {
        None => rows.push(row("Bitwarden", "Checking…").upcast()),
        Some(BwState::Missing) => rows.push(
            row(
                "Bitwarden",
                "Install the Bitwarden CLI (bw) to fill and save from an existing vault — for example: sudo snap install bw",
            )
            .upcast(),
        ),
        Some(BwState::Unauthenticated) => {
            let head = row("Not connected", "Sign in to fill and save from an existing vault");
            rows.push(head.upcast());
            let server = entry("Server", snap.bw_server.as_deref().unwrap_or("https://vault.bitwarden.com"));
            let email = entry("Email", "");
            email.set_input_purpose(gtk::InputPurpose::Email);
            let password = password_entry("Master password");
            let method = adw::ComboRow::new();
            method.set_title("Two-step login");
            method.set_subtitle("If the account has one; Bitwarden asks for the code with the password");
            method.set_model(Some(&gtk::StringList::new(&["None", "Authenticator app", "Email", "YubiKey"])));
            let code = entry("Two-step code", "");
            code.set_input_purpose(gtk::InputPurpose::Digits);
            let go = row("", "");
            let sign_in = pill("Sign In", true);
            go.add_suffix(&sign_in);
            {
                let weak = Rc::downgrade(ui);
                let (server, email, password, method, code) =
                    (server.clone(), email.clone(), password.clone(), method.clone(), code.clone());
                sign_in.connect_clicked(move |_| {
                    let Some(ui) = weak.upgrade() else { return };
                    let srv = server.text().to_string();
                    let who = email.text().to_string();
                    let pw = password.text().to_string();
                    password.set_text("");
                    let m: Option<u8> = match method.selected() {
                        1 => Some(0),
                        2 => Some(1),
                        3 => Some(3),
                        _ => None,
                    };
                    let c = code.text().to_string();
                    let default = srv.trim().is_empty() || srv.contains("vault.bitwarden.com");
                    if let Some(s) = ui.shell.upgrade() {
                        s.update_settings(|st| st.set(keys::BW_SERVER, srv.trim()));
                    }
                    act(&ui, move |v| async move {
                        let mut bw = v.bw.lock().await;
                        if !default {
                            bw.configure(&srv).await.map_err(|e| e.to_string())?;
                        }
                        let code = (!c.trim().is_empty()).then_some(c.as_str());
                        bw.login(&who, &pw, m, code).await.map_err(|e| e.to_string())
                    });
                });
            }
            for w in [server.upcast_ref::<gtk::Widget>(), email.upcast_ref(), password.upcast_ref(), method.upcast_ref(), code.upcast_ref(), go.upcast_ref()] {
                rows.push(w.clone());
            }
            let api = adw::ExpanderRow::new();
            api.set_title("Use a personal API key instead");
            api.set_subtitle("From the web vault: Account settings › Security › Keys");
            let id = entry("client_id", "");
            let secret = password_entry("client_secret");
            let unlock = password_entry("Master password (to unlock)");
            let go = row("", "");
            let b = pill("Sign In with API Key", true);
            go.add_suffix(&b);
            {
                let weak = Rc::downgrade(ui);
                let (id, secret, unlock) = (id.clone(), secret.clone(), unlock.clone());
                b.connect_clicked(move |_| {
                    let Some(ui) = weak.upgrade() else { return };
                    let (i, s, p) = (id.text().to_string(), secret.text().to_string(), unlock.text().to_string());
                    secret.set_text("");
                    unlock.set_text("");
                    act(&ui, move |v| async move {
                        let mut bw = v.bw.lock().await;
                        bw.login_api_key(&i, &s).await.map_err(|e| e.to_string())?;
                        if p.is_empty() {
                            return Ok(());
                        }
                        bw.unlock(&p).await.map_err(|e| e.to_string())
                    });
                });
            }
            api.add_row(&id);
            api.add_row(&secret);
            api.add_row(&unlock);
            api.add_row(&go);
            rows.push(api.upcast());
        }
        Some(BwState::Locked { email }) => {
            let head = row(
                "Locked",
                &email.map(|e| format!("Signed in as {e}")).unwrap_or_default(),
            );
            head.add_prefix(&gtk::Image::from_icon_name("system-lock-screen-symbolic"));
            let out = pill("Sign Out", false);
            let weak = Rc::downgrade(ui);
            out.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move {
                        v.bw.lock().await.logout().await;
                        Ok(())
                    });
                }
            });
            head.add_suffix(&out);
            rows.push(head.upcast());
            let pw = password_entry("Master password");
            pw.set_show_apply_button(true);
            let weak = Rc::downgrade(ui);
            pw.connect_apply(move |e| {
                let p = e.text().to_string();
                e.set_text("");
                if let Some(ui) = weak.upgrade() {
                    act(&ui, move |v| async move {
                        v.bw.lock().await.unlock(&p).await.map_err(|e| e.to_string())
                    });
                }
            });
            {
                let weak = Rc::downgrade(ui);
                pw.connect_entry_activated(move |e| {
                    let p = e.text().to_string();
                    e.set_text("");
                    if let Some(ui) = weak.upgrade() {
                        act(&ui, move |v| async move {
                            v.bw.lock().await.unlock(&p).await.map_err(|e| e.to_string())
                        });
                    }
                });
            }
            rows.push(pw.upcast());
        }
        Some(BwState::Unlocked { email, last_sync }) => {
            let head = row(
                "Unlocked",
                &format!(
                    "{}{}",
                    email.unwrap_or_else(|| "Signed in".into()),
                    last_sync.map(|s| format!(" · synced {}", short_time(&s))).unwrap_or_default()
                ),
            );
            head.add_prefix(&gtk::Image::from_icon_name("emblem-ok-symbolic"));
            let sync = pill("Sync Now", false);
            let weak = Rc::downgrade(ui);
            sync.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move { v.bw.lock().await.sync().await.map_err(|e| e.to_string()) });
                }
            });
            let lock = pill("Lock", false);
            let weak = Rc::downgrade(ui);
            lock.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move {
                        v.bw.lock().await.lock().await;
                        Ok(())
                    });
                }
            });
            head.add_suffix(&sync);
            head.add_suffix(&lock);
            rows.push(head.upcast());
            let (l, i, c) = snap.bw_counts;
            rows.push(row("Vault", &counts(l, i, c)).upcast());
            let stay = adw::SwitchRow::new();
            stay.set_title("Stay unlocked between launches");
            stay.set_subtitle("The session is kept beside Bitwarden's own data, readable by you only; off asks for the master password once per launch");
            stay.set_active(snap.bw_stay);
            let weak = Rc::downgrade(ui);
            stay.connect_active_notify(move |r| {
                let on = r.is_active();
                if let Some(ui) = weak.upgrade() {
                    if let Some(s) = ui.shell.upgrade() {
                        s.update_settings(|st| st.set(keys::BW_STAY, on));
                    }
                    act(&ui, move |v| async move {
                        v.bw.lock().await.set_stay_unlocked(on);
                        Ok(())
                    });
                }
            });
            rows.push(stay.upcast());
            rows.push(autolock_row(ui).upcast());
            let out = row("Account", "");
            let b = pill("Sign Out", false);
            let weak = Rc::downgrade(ui);
            b.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move {
                        v.bw.lock().await.logout().await;
                        Ok(())
                    });
                }
            });
            out.add_suffix(&b);
            rows.push(out.upcast());
        }
    }
    put(ui, "bw", &ui.bw, rows);
}

/// Mac `BitwardenCard` "Auto-lock": 5, 15 or 60 minutes, or never (0).
fn autolock_row(ui: &Rc<Ui>) -> adw::ComboRow {
    const MINUTES: [u32; 4] = [5, 15, 60, 0];
    let r = adw::ComboRow::new();
    r.set_title("Auto-lock");
    r.set_subtitle("Lock the local Bitwarden session after inactivity");
    r.set_model(Some(&gtk::StringList::new(&[
        "5 minutes",
        "15 minutes",
        "60 minutes",
        "Never",
    ])));
    let Some(shell) = ui.shell.upgrade() else {
        return r;
    };
    let now = shell.settings().number(keys::BW_AUTOLOCK).unwrap_or(0.0);
    let at = MINUTES
        .iter()
        .position(|&m| f64::from(m) == now)
        .unwrap_or(3);
    r.set_selected(at as u32);
    let weak = shell.downgrade();
    r.connect_selected_notify(move |r| {
        let m = MINUTES.get(r.selected() as usize).copied().unwrap_or(0);
        if let Some(s) = weak.upgrade() {
            s.update_settings(|st| st.set(keys::BW_AUTOLOCK, m));
        }
    });
    r
}

fn short_time(iso: &str) -> String {
    // "2026-10-12T22:01:03.000Z" → "2026-10-12 22:01"
    let t = iso.replace('T', " ");
    t.chars().take(16).collect()
}

fn counts(l: usize, i: usize, c: usize) -> String {
    let mut parts = Vec::new();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if l > 0 {
        parts.push(plural(l, "login", "logins"));
    }
    if i > 0 {
        parts.push(plural(i, "identity", "identities"));
    }
    if c > 0 {
        parts.push(plural(c, "card", "cards"));
    }
    if parts.is_empty() {
        "Nothing saved yet".into()
    } else {
        parts.join(" · ")
    }
}

// MARK: 1Password

fn build_op(ui: &Rc<Ui>) {
    let snap = ui.snap.borrow().clone();
    let mut rows: Vec<gtk::Widget> = Vec::new();
    use copper_creds::onepassword::OpState;
    match snap.op_state.clone() {
        None => rows.push(row("1Password", "Checking…").upcast()),
        Some(OpState::Missing) => rows.push(
            row(
                "1Password",
                "Install the 1Password CLI (op) to fill and save from your vaults — 1Password's site has a .deb and an apt repository",
            )
            .upcast(),
        ),
        Some(OpState::Unlocked) => {
            let head = row(
                "Unlocked",
                &match snap.op_mode {
                    Some(Mode::Service) => "Service account".to_string(),
                    _ => snap.op_account.clone().unwrap_or_else(|| "Signed in".into()),
                },
            );
            head.add_prefix(&gtk::Image::from_icon_name("emblem-ok-symbolic"));
            let sync = pill("Sync Now", false);
            let weak = Rc::downgrade(ui);
            sync.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move { v.op.lock().await.sync().await.map_err(|e| e.to_string()) });
                }
            });
            let lock = pill("Lock", false);
            let weak = Rc::downgrade(ui);
            lock.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move {
                        v.op.lock().await.lock().await;
                        Ok(())
                    });
                }
            });
            head.add_suffix(&sync);
            head.add_suffix(&lock);
            rows.push(head.upcast());
            let (l, i, c) = snap.op_counts;
            rows.push(row("Vaults", &counts(l, i, c)).upcast());
            rows.push(
                row(
                    "New logins go to",
                    snap.op_save_vault.as_deref().unwrap_or("No vault Copper can write to"),
                )
                .upcast(),
            );
            let out = row("Connection", "");
            let b = pill("Sign Out", false);
            let weak = Rc::downgrade(ui);
            b.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move {
                        v.op.lock().await.sign_out().await;
                        Ok(())
                    });
                }
            });
            out.add_suffix(&b);
            rows.push(out.upcast());
        }
        Some(state) => {
            let locked = state == OpState::Locked;
            let head = row(
                if locked { "Locked" } else { "Not connected" },
                &snap
                    .op_account
                    .clone()
                    .unwrap_or_else(|| "Connect to fill and save from your vaults".into()),
            );
            rows.push(head.upcast());
            if let Some(p) = &snap.op_problem {
                let r = row("1Password couldn't do that", &first_line(p));
                r.add_prefix(&gtk::Image::from_icon_name("dialog-warning-symbolic"));
                rows.push(r.upcast());
            }
            // Through the desktop app (its own approval; nothing typed here).
            let app = row("Unlock with the 1Password app", "Turn on “Integrate with 1Password CLI” in the app's Developer settings");
            let b = pill("Unlock", true);
            let weak = Rc::downgrade(ui);
            b.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    act(&ui, |v| async move { v.op.lock().await.unlock_with_app(None).await.map_err(|e| e.to_string()) });
                }
            });
            app.add_suffix(&b);
            rows.push(app.upcast());
            if !snap.op_accounts.is_empty() || locked {
                let pw = password_entry("Account password");
                pw.set_show_apply_button(true);
                let weak = Rc::downgrade(ui);
                pw.connect_apply(move |e| {
                    let p = e.text().to_string();
                    e.set_text("");
                    if let Some(ui) = weak.upgrade() {
                        act(&ui, move |v| async move {
                            let mut op = v.op.lock().await;
                            if op.mode() == Some(Mode::Password) {
                                op.unlock(Some(&p)).await.map_err(|e| e.to_string())
                            } else {
                                op.sign_in(&p, None).await.map_err(|e| e.to_string())
                            }
                        });
                    }
                });
                rows.push(pw.upcast());
            }
            let token = password_entry("Service account token (ops_…)");
            token.set_show_apply_button(true);
            let weak = Rc::downgrade(ui);
            token.connect_apply(move |e| {
                let t = e.text().to_string();
                e.set_text("");
                if let Some(ui) = weak.upgrade() {
                    act(&ui, move |v| async move { v.op.lock().await.connect(&t).await.map_err(|e| e.to_string()) });
                }
            });
            rows.push(token.upcast());
            let add = adw::ExpanderRow::new();
            add.set_title("Add a 1Password account");
            add.set_subtitle("Sign-in address, email, Secret Key and password");
            let address = entry("Sign-in address", "my.1password.com");
            let email = entry("Email", "");
            let key = password_entry("Secret Key");
            let password = password_entry("Password");
            let go = row("", "");
            let b = pill("Add and Sign In", true);
            go.add_suffix(&b);
            {
                let weak = Rc::downgrade(ui);
                let (address, email, key, password) = (address.clone(), email.clone(), key.clone(), password.clone());
                b.connect_clicked(move |_| {
                    let Some(ui) = weak.upgrade() else { return };
                    let (a, e, k, p) = (
                        address.text().to_string(),
                        email.text().to_string(),
                        key.text().to_string(),
                        password.text().to_string(),
                    );
                    key.set_text("");
                    password.set_text("");
                    act(&ui, move |v| async move {
                        v.op.lock().await.add_account(&a, &e, &k, &p).await.map_err(|e| e.to_string())
                    });
                });
            }
            add.add_row(&address);
            add.add_row(&email);
            add.add_row(&key);
            add.add_row(&password);
            add.add_row(&go);
            rows.push(add.upcast());
        }
    }
    put(ui, "op", &ui.op, rows);
}

// MARK: BotPass

fn build_bp(ui: &Rc<Ui>) {
    use copper_botpass::BpState;
    let snap = ui.snap.borrow().bp.clone();
    let mut rows: Vec<gtk::Widget> = Vec::new();
    if snap.paired() {
        let (title, icon) = match &snap.state {
            BpState::Locked => (
                "Locked — run bp unlock".to_string(),
                "system-lock-screen-symbolic",
            ),
            BpState::Revoked => (
                "BotPass no longer knows this device — pair again".to_string(),
                "dialog-warning-symbolic",
            ),
            BpState::Unreachable(why) => (
                format!("Not answering: {}", first_line(why)),
                "dialog-warning-symbolic",
            ),
            BpState::Ready => ("Unlocked".to_string(), "emblem-ok-symbolic"),
            BpState::Paired | BpState::Unpaired => ("Paired".to_string(), "emblem-ok-symbolic"),
        };
        let head = row(
            &title,
            &format!(
                "{}{}",
                snap.url.clone().unwrap_or_default(),
                snap.bot
                    .as_deref()
                    .map(|b| format!(" · bot {b}"))
                    .unwrap_or_default()
            ),
        );
        head.add_prefix(&gtk::Image::from_icon_name(icon));
        let check = pill("Check", false);
        let weak = Rc::downgrade(ui);
        check.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                act(&ui, |v| async move {
                    v.bp.refresh().await;
                    Ok(())
                });
            }
        });
        head.add_suffix(&check);
        rows.push(head.upcast());
        rows.push(
            row(
                "This device",
                snap.device_name.as_deref().unwrap_or("Copper"),
            )
            .upcast(),
        );
        rows.push(
            row(
                "Certificate",
                &match &snap.fingerprint {
                    Some(f) => format!("Pinned · SHA-256 {f}"),
                    None => "No pin (public authorities, or this machine over http)".to_string(),
                },
            )
            .upcast(),
        );
        let out = row(
            "Pairing",
            "Unpairing forgets the token here; remove the device in BotPass to revoke it",
        );
        let b = pill("Unpair", false);
        let weak = Rc::downgrade(ui);
        b.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                act(&ui, |v| async move {
                    v.bp.unpair().map(|_| ()).map_err(|e| e.to_string())
                });
            }
        });
        out.add_suffix(&b);
        rows.push(out.upcast());
    } else {
        rows.push(
            row(
                "Not paired",
                "On the BotPass machine run bp connect copper — or make a code with bp pair <bot> and enter it here",
            )
            .upcast(),
        );
        let server = entry("Server (https://host:port, or a botpass:// link)", "");
        server.set_input_purpose(gtk::InputPurpose::Url);
        let fingerprint = entry("Certificate fingerprint (SHA-256)", "");
        let code = entry("Pairing code", "");
        let name = entry("Device name", &copper_botpass::default_device_name());
        let go = row("", "");
        let pair = pill("Pair", true);
        go.add_suffix(&pair);
        {
            let weak = Rc::downgrade(ui);
            let (server, fingerprint, code, name) = (
                server.clone(),
                fingerprint.clone(),
                code.clone(),
                name.clone(),
            );
            pair.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                let mut srv = server.text().trim().to_string();
                let mut fp = fingerprint.text().trim().to_string();
                let mut c = code.text().trim().to_string();
                if let Some(link) = copper_botpass::parse_link(&srv) {
                    srv = link.server;
                    if c.is_empty() {
                        c = link.code;
                    }
                    if fp.is_empty() {
                        fp = link.fingerprint.unwrap_or_default();
                    }
                }
                let n = name.text().trim().to_string();
                code.set_text("");
                act(&ui, move |v| async move {
                    let fp = (!fp.is_empty()).then_some(fp.as_str());
                    v.bp.pair(&srv, fp, &c, &n)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string())
                });
            });
        }
        for w in [
            server.upcast_ref::<gtk::Widget>(),
            fingerprint.upcast_ref(),
            code.upcast_ref(),
            name.upcast_ref(),
            go.upcast_ref(),
        ] {
            rows.push(w.clone());
        }
    }
    put(ui, "bp", &ui.bp, rows);
}

// MARK: agent access

fn build_agents(ui: &Rc<Ui>) {
    let Some(shell) = ui.shell.upgrade() else {
        return;
    };
    let Some(v) = super::vault() else { return };
    let weak = Rc::downgrade(ui);
    glib::spawn_future_local(async move {
        let vv = v.clone();
        let (all, cards, ids) = rt::run(async move {
            (
                vv.all_credentials().await,
                vv.cards().await,
                vv.identities().await,
            )
        })
        .await
        .unwrap_or_default();
        let Some(ui) = weak.upgrade() else { return };
        let mut rows: Vec<gtk::Widget> = Vec::new();
        let share_all = adw::SwitchRow::new();
        share_all.set_title("Share every saved account");
        rows.push(share_all.clone().upcast());
        for c in all.iter().take(300) {
            rows.push(agent_row(&shell, c).upcast());
        }
        for (id, title, sub) in cards
            .iter()
            .map(|c| {
                let stable = if c.id.starts_with("op:") {
                    c.id.clone()
                } else {
                    format!("bw:{}", c.id)
                };
                (
                    stable,
                    c.name.clone(),
                    format!(
                        "Card · {}",
                        if c.id.starts_with("op:") {
                            "1Password"
                        } else {
                            "Bitwarden"
                        }
                    ),
                )
            })
            .chain(ids.iter().map(|i| {
                let stable = if i.id.starts_with("op:") {
                    i.id.clone()
                } else {
                    format!("bw:{}", i.id)
                };
                (
                    stable,
                    i.name.clone(),
                    format!(
                        "Identity · {}",
                        if i.id.starts_with("op:") {
                            "1Password"
                        } else {
                            "Bitwarden"
                        }
                    ),
                )
            }))
        {
            rows.push(id_row(&shell, &id, &title, &sub).upcast());
        }
        // Mac (Copper#116): sharing everything needs no list.
        let list = rows[1..].to_vec();
        let show = move |r: &adw::SwitchRow| {
            let on = r.is_active();
            r.set_subtitle(if on {
                "Agents can use every saved sign-in in Copper without receiving the password"
            } else {
                "Off, agents use only the accounts you allow (and a Bitwarden “Agents” folder)"
            });
            for w in &list {
                w.set_visible(!on);
            }
        };
        share_all.set_active(shell.settings().bool_or(keys::SHARE_ALL, false));
        show(&share_all);
        let w2 = shell.downgrade();
        share_all.connect_active_notify(move |r| {
            show(r);
            if let Some(s) = w2.upgrade() {
                let on = r.is_active();
                s.update_settings(|st| st.set(keys::SHARE_ALL, on));
            }
        });
        put(&ui, "agents", &ui.agents, rows);
    });
}

fn agent_row(shell: &Shell, c: &Credential) -> adw::SwitchRow {
    let r = adw::SwitchRow::new();
    r.set_title(&glib::markup_escape_text(&c.host));
    r.set_subtitle(&glib::markup_escape_text(&format!(
        "{} · {}",
        if c.user.is_empty() {
            "No username"
        } else {
            &c.user
        },
        source_title(c.source)
    )));
    r.set_active(super::agent_allowed(shell, c));
    if c.agent_hint == copper_creds::AgentHint::Deny {
        r.set_sensitive(false);
        r.set_subtitle("Marked copper-agent: deny in the vault");
    }
    let weak = shell.downgrade();
    let c = c.clone();
    r.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            super::set_agent_allowed(&s, &c, r.is_active());
        }
    });
    r
}

fn id_row(shell: &Shell, stable: &str, title: &str, sub: &str) -> adw::SwitchRow {
    let r = adw::SwitchRow::new();
    r.set_title(&glib::markup_escape_text(title));
    r.set_subtitle(&glib::markup_escape_text(sub));
    r.set_active(super::agent_allowed_id(shell, stable));
    let weak = shell.downgrade();
    let id = stable.to_string();
    r.connect_active_notify(move |r| {
        if let Some(s) = weak.upgrade() {
            let mut list: Vec<String> = s
                .settings()
                .get(keys::ALLOWED)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            list.retain(|x| *x != id);
            if r.is_active() {
                list.push(id.clone());
            }
            list.sort();
            let value = serde_json::Value::Array(list.into_iter().map(Into::into).collect());
            s.update_settings(|st| st.set(keys::ALLOWED, value));
        }
    });
    r
}
