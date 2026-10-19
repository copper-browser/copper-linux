//! The passwords lane's end-to-end run (debug builds; `COPPER_SELFTEST=forms`).
//! Real pages over HTTP, real input through WebKit, the real Secret Service
//! (a session keyring), the Bitwarden mock CLI, Chrome files on disk.
//!
//! Env: `COPPER_FORMS_BASE` (where login.html, login-fail.html and
//! checkout.html are served), `COPPER_FORMS_OUT` (screenshots and
//! `result.json`). Bitwarden: `COPPER_BW_PATH` + `BW_MOCK_STATE`.
//! Prints `forms selftest: N passed, M failed` and exits 1 on a failure.

use super::{Forms, rt};
use crate::app::set_exit;
use crate::shell::GtkShell;
use crate::shell::{OpenTab, Shell};
use copper_core::TabId;
use copper_core::settings::Look;
use copper_engine::{InputEvent, PageView, World};
use gtk4 as gtk;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

const USER: &str = "ann@example.com";
const PASS: &str = "fixture-pw-7Hq";

struct Run {
    shell: Shell,
    forms: Rc<Forms>,
    out: PathBuf,
    base: String,
    results: RefCell<Vec<Value>>,
}

impl Run {
    fn check(&self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!(
            "{} {name}{}",
            if ok { "PASS" } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        );
        self.results
            .borrow_mut()
            .push(json!({"check": name, "ok": ok, "detail": detail}));
    }

    async fn sleep(ms: u64) {
        glib::timeout_future(Duration::from_millis(ms)).await;
    }

    async fn until(&self, secs: u64, mut f: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        loop {
            if f() {
                return true;
            }
            if start.elapsed() > Duration::from_secs(secs) {
                return false;
            }
            Self::sleep(80).await;
        }
    }

    async fn loaded(&self, id: &TabId) {
        let start = Instant::now();
        loop {
            Self::sleep(80).await;
            if let Some(slot) = self.shell.slot(id)
                && !slot.loading.get()
                && !slot.page.is_loading()
                && slot.page.url().is_some()
            {
                let ready = slot.page.eval("document.readyState", &World::Page).await;
                if ready
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .as_deref()
                    == Some("complete")
                {
                    Self::sleep(250).await;
                    return;
                }
            }
            if start.elapsed() > Duration::from_secs(15) {
                return;
            }
        }
    }

    async fn js(&self, id: &TabId, expr: &str) -> Value {
        match self.shell.page(id) {
            Some(p) => p.eval(expr, &World::Page).await.unwrap_or(Value::Null),
            None => Value::Null,
        }
    }

    /// A real click in the middle of `selector`.
    async fn click(&self, id: &TabId, selector: &str) -> bool {
        let r = self
            .js(
                id,
                &format!(
                    "(function(){{var e=document.querySelector({sel});if(!e)return null;e.scrollIntoView({{block:'center'}});var r=e.getBoundingClientRect();return [r.left+r.width/2,r.top+r.height/2];}})()",
                    sel = serde_json::to_string(selector).unwrap()
                ),
            )
            .await;
        let Some(xy) = r.as_array() else { return false };
        let (x, y) = (xy[0].as_f64().unwrap_or(0.0), xy[1].as_f64().unwrap_or(0.0));
        let Some(p) = self.shell.page(id) else {
            return false;
        };
        p.input(InputEvent::click(x, y)).await.is_ok()
    }

    async fn type_text(&self, id: &TabId, s: &str) {
        if let Some(p) = self.shell.page(id) {
            let _ = p.input(InputEvent::Type(s.into())).await;
        }
    }

    async fn key(&self, id: &TabId, k: &str) {
        if let Some(p) = self.shell.page(id) {
            let _ = p.input(InputEvent::key(k)).await;
        }
    }

    fn shot(&self, name: &str) {
        let path = self.out.join(format!("{name}.png"));
        match crate::screenshot::save(self.shell.window().upcast_ref(), &path) {
            Ok(()) => println!("screenshot {}", path.display()),
            Err(e) => println!("screenshot {name} failed: {e}"),
        }
    }

    /// Light, then dark, then back.
    async fn shots(&self, name: &str) {
        self.shell.set_look(Look::Light);
        Self::sleep(350).await;
        self.shot(&format!("{name}-light"));
        self.shell.set_look(Look::Dark);
        Self::sleep(350).await;
        self.shot(&format!("{name}-dark"));
        self.shell.set_look(Look::Light);
        Self::sleep(200).await;
    }

    async fn open(&self, path: &str) -> TabId {
        let id = self.shell.open(
            &format!("{}{path}", self.base),
            OpenTab {
                foreground: true,
                ..Default::default()
            },
        );
        self.loaded(&id).await;
        // The forms sweep attaches within ~700 ms.
        Self::sleep(900).await;
        id
    }

    async fn sign_in_by_hand(&self, tab: &TabId, user: &str, pass: &str) {
        self.click(tab, "#username").await;
        Self::sleep(150).await;
        self.type_text(tab, user).await;
        self.click(tab, "#password").await;
        Self::sleep(150).await;
        self.type_text(tab, pass).await;
        Self::sleep(100).await;
        self.key(tab, "Enter").await;
    }
}

pub fn start(shell: &Shell) {
    let Some(forms) = super::forms() else { return };
    let out = PathBuf::from(
        std::env::var("COPPER_FORMS_OUT").unwrap_or_else(|_| "/tmp/copper-forms".into()),
    );
    let _ = std::fs::create_dir_all(&out);
    let base =
        std::env::var("COPPER_FORMS_BASE").unwrap_or_else(|_| "http://localhost:8765/".into());
    let run = Rc::new(Run {
        shell: shell.clone(),
        forms,
        out,
        base,
        results: RefCell::default(),
    });
    glib::spawn_future_local(async move {
        Run::sleep(1500).await;
        all(&run).await;
        let results = run.results.borrow().clone();
        let failed = results.iter().filter(|r| r["ok"] != json!(true)).count();
        println!(
            "forms selftest: {} passed, {failed} failed",
            results.len() - failed
        );
        let _ = std::fs::write(
            run.out.join(format!(
                "result-{}.json",
                std::env::var("COPPER_FORMS_PHASE").unwrap_or_else(|_| "main".into())
            )),
            serde_json::to_vec_pretty(&json!({"results": results, "failed": failed}))
                .unwrap_or_default(),
        );
        if failed > 0 {
            set_exit(1);
        }
        run.shell.save_now();
        run.shell.shutdown();
        run.shell.app().quit();
    });
}

async fn all(run: &Rc<Run>) {
    if std::env::var("COPPER_FORMS_PHASE").is_ok_and(|p| p == "nokeyring") {
        no_keyring(run).await;
        return;
    }
    let v = run.forms.vault.clone();
    let ready = run.until(25, || v.snapshot().keyring_ready()).await;
    run.check(
        "keyring: the session Secret Service answers",
        ready,
        format!("{:?}", v.snapshot().keyring),
    );

    save_offer(run).await;
    fill(run).await;
    settings_page(run).await;
    bitwarden_otp_and_agents(run).await;
    chrome_import(run).await;
}

/// A failed sign-in is not offered; one that worked is, and Save keeps it.
async fn save_offer(run: &Rc<Run>) {
    let tab = run.open("login-fail.html").await;
    run.sign_in_by_hand(&tab, USER, "wrong-password").await;
    Run::sleep(5000).await;
    let still = run
        .js(&tab, "!!document.querySelector('input[type=password]')")
        .await;
    run.check(
        "save offer: not shown after a sign-in that failed",
        !run.forms.offering(&tab) && still == json!(true),
        format!("form still there: {still}"),
    );
    run.shell.close(&tab);

    let tab = run.open("login.html").await;
    run.shot("01-login-page-light");
    run.sign_in_by_hand(&tab, USER, PASS).await;
    let signed_at = Instant::now();
    let offered = run.until(12, || run.forms.offering(&tab)).await;
    let page_says = run
        .js(&tab, "document.getElementById('result').textContent")
        .await;
    run.check(
        "save offer: shown once the sign-in worked",
        offered
            && page_says
                .as_str()
                .is_some_and(|s| s.contains("Signed in as")),
        format!("after {:?}; page: {page_says}", signed_at.elapsed()),
    );
    run.shots("02-save-offer").await;

    let pressed = run.forms.with_offer(&tab, |o| o.press_save()).is_some();
    let v = run.forms.vault.clone();
    let mut kept = false;
    for _ in 0..40 {
        Run::sleep(150).await;
        let vv = v.clone();
        let logins = rt::run(async move { vv.logins().await })
            .await
            .and_then(Result::ok)
            .unwrap_or_default();
        if logins
            .iter()
            .any(|l| l.host == "localhost" && l.user == USER)
        {
            kept = true;
            break;
        }
    }
    let vv = v.clone();
    let secret = rt::run(async move {
        vv.secret(&copper_creds::CredentialId::Keychain {
            host: "localhost".into(),
            user: USER.into(),
        })
        .await
    })
    .await
    .and_then(Result::ok);
    run.check(
        "save offer: Save keeps it in the keyring",
        pressed && kept && secret.as_deref() == Some(PASS) && !run.forms.offering(&tab),
        "",
    );
    run.shell.close(&tab);
}

/// Back on the page: a click in the box offers the account; nothing is
/// filled until the row is clicked; then both boxes are.
async fn fill(run: &Rc<Run>) {
    let tab = run.open("login.html").await;
    let empty_before = run.js(&tab, "document.getElementById('username').value + '|' + document.getElementById('password').value").await;
    run.click(&tab, "#username").await;
    let offered = run
        .until(8, || {
            run.forms
                .picker_rows(&tab)
                .is_some_and(|r| r.iter().any(|u| u == USER))
        })
        .await;
    Run::sleep(300).await;
    let still_empty = run.js(&tab, "document.getElementById('username').value + '|' + document.getElementById('password').value").await;
    run.check(
        "fill: clicking the field offers the saved account, nothing filled yet",
        offered && empty_before == json!("|") && still_empty == json!("|"),
        format!("rows: {:?}", run.forms.picker_rows(&tab)),
    );
    run.shots("03-account-picker").await;
    // The dark/light switch can move focus; make sure the list is up.
    if run.forms.picker_rows(&tab).is_none() {
        run.click(&tab, "#password").await;
        run.until(5, || run.forms.picker_rows(&tab).is_some()).await;
    }
    let pressed = run.forms.with_picker(&tab, |p| p.press(0)).is_some();
    let mut ok = false;
    for _ in 0..50 {
        let v = run
            .js(&tab, &format!("document.getElementById('username').value === {} && document.getElementById('password').value === {}", json!(USER), json!(PASS)))
            .await;
        if v == json!(true) {
            ok = true;
            break;
        }
        Run::sleep(100).await;
    }
    run.check(
        "fill: picking the row fills name and password",
        pressed && ok,
        "",
    );
    run.shot("04-filled-light");
    let lowered = run.until(3, || run.forms.picker_rows(&tab).is_none()).await;
    run.check("fill: the list goes away after the fill", lowered, "");
    run.shell.close(&tab);
}

fn walk(w: &gtk::Widget, f: &mut dyn FnMut(&gtk::Widget)) {
    f(w);
    let mut c = w.first_child();
    while let Some(ch) = c {
        walk(&ch, f);
        c = ch.next_sibling();
    }
}

fn find_row(root: &gtk::Widget, title: &str) -> Option<adw::ActionRow> {
    let mut found = None;
    walk(root, &mut |w| {
        if found.is_none()
            && let Some(r) = w.downcast_ref::<adw::ActionRow>()
            && r.title() == title
        {
            found = Some(r.clone());
        }
    });
    found
}

fn find_button(root: &gtk::Widget, tip: &str) -> Option<gtk::Button> {
    let mut found = None;
    walk(root, &mut |w| {
        if found.is_none()
            && let Some(b) = w.downcast_ref::<gtk::Button>()
            && b.tooltip_text().as_deref() == Some(tip)
        {
            found = Some(b.clone());
        }
    });
    found
}

fn has_label(root: &gtk::Widget, text: &str) -> bool {
    let mut yes = false;
    walk(root, &mut |w| {
        if let Some(l) = w.downcast_ref::<gtk::Label>()
            && l.text() == text
        {
            yes = true;
        }
    });
    yes
}

/// Scroll the settings page so `w` is near the top.
fn scroll_to(w: &gtk::Widget) {
    let Some(sw) = w
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
    else {
        return;
    };
    let Some(child) = sw.child() else { return };
    if let Some(p) = w.compute_point(&child, &gtk::graphene::Point::zero()) {
        let adj = sw.vadjustment();
        adj.set_value((p.y() as f64 - 16.0).clamp(0.0, adj.upper() - adj.page_size()));
    }
}

fn find_group(root: &gtk::Widget, title: &str) -> Option<gtk::Widget> {
    let mut found = None;
    walk(root, &mut |w| {
        if found.is_none()
            && let Some(g) = w.downcast_ref::<adw::PreferencesGroup>()
            && g.title() == title
        {
            found = Some(g.clone().upcast());
        }
    });
    found
}

/// Settings › Passwords: the saved login is listed, shown only after the
/// confirmation, and the page renders in both looks.
async fn settings_page(run: &Rc<Run>) {
    let dialog = crate::window::settings::dialog(&run.shell);
    dialog.set_visible_page_name("passwords");
    Run::sleep(1500).await;
    let root: gtk::Widget = dialog.clone().upcast();
    let saved = || find_group(&root, "Saved passwords").unwrap_or_else(|| root.clone());
    let row = find_row(&saved(), "localhost");
    run.check("settings: the saved login is listed", row.is_some(), "");
    run.shots("05-settings-passwords").await;
    if let Some(row) = row {
        let w: gtk::Widget = row.upcast();
        let hidden_first = !has_label(&w, PASS);
        if let Some(b) = find_button(&w, "Show password") {
            b.emit_clicked();
            Run::sleep(500).await;
            let alert = run
                .shell
                .window()
                .visible_dialog()
                .and_downcast::<adw::AlertDialog>();
            let asked = alert.is_some();
            if let Some(a) = &alert {
                a.emit_by_name::<()>("response", &[&"show"]);
                a.close();
            }
            Run::sleep(800).await;
            let row = find_row(&saved(), "localhost").map(|r| r.upcast::<gtk::Widget>());
            let shown = row.as_ref().is_some_and(|r| has_label(r, PASS));
            run.check(
                "settings: reveal asks first, then shows",
                hidden_first && asked && shown,
                "",
            );
            run.shot("06-settings-revealed-light");
            if let Some(r) = row
                && let Some(h) = find_button(&r, "Hide password")
            {
                h.emit_clicked();
            }
        } else {
            run.check("settings: reveal button", false, "not found");
        }
    }
    let search_ok = {
        let mut entry = None;
        walk(&root, &mut |w| {
            if entry.is_none()
                && let Some(e) = w.downcast_ref::<gtk::SearchEntry>()
                && e.widget_name() == "copper-passwords-search"
            {
                entry = Some(e.clone());
            }
        });
        if let Some(e) = entry {
            e.set_text("nothing-like-this");
            e.emit_by_name::<()>("search-changed", &[]);
            Run::sleep(500).await;
            let saved_group = find_group(&root, "Saved passwords").unwrap_or_else(|| root.clone());
            let hidden = find_row(&saved_group, "localhost").is_none();
            let said = find_row(&root, "Nothing matches").is_some();
            e.set_text("local");
            e.emit_by_name::<()>("search-changed", &[]);
            Run::sleep(500).await;
            let back = find_row(&saved_group, "localhost").is_some();
            format!("hidden {hidden}, said {said}, back {back}")
        } else {
            "no search entry".to_string()
        }
    };
    run.check(
        "settings: search filters the list",
        search_ok == "hidden true, said true, back true",
        search_ok,
    );
    if let Some(g) = find_group(&root, "Bitwarden") {
        scroll_to(&g);
        Run::sleep(400).await;
        run.check(
            "settings: Bitwarden sign-in form (email, password, two-step, API key)",
            find_row(&g, "Email").is_some() || has_label(&g, "Email"),
            "",
        );
        run.shots("05b-settings-bitwarden-signin").await;
    }
    if let Some(g) = find_group(&root, "1Password") {
        scroll_to(&g);
        Run::sleep(400).await;
        run.shots("05c-settings-1password-agents").await;
    }
    dialog.close();
    Run::sleep(300).await;
}

async fn bitwarden_otp_and_agents(run: &Rc<Run>) {
    if std::env::var("COPPER_BW_PATH").is_err() {
        run.check("bitwarden: mock configured", false, "COPPER_BW_PATH unset");
        return;
    }
    let v = run.forms.vault.clone();
    let vv = v.clone();
    let r = rt::run(async move {
        let r = vv
            .bw
            .lock()
            .await
            .login(USER, "correct horse", None, None)
            .await;
        vv.refresh().await;
        r.map_err(|e| e.to_string())
    })
    .await;
    let unlocked = v.snapshot().bw_unlocked();
    run.check(
        "bitwarden: sign-in with email + master password unlocks",
        unlocked,
        format!("{r:?}"),
    );

    let dialog = crate::window::settings::dialog(&run.shell);
    dialog.set_visible_page_name("passwords");
    Run::sleep(1500).await;
    let root: gtk::Widget = dialog.clone().upcast();
    run.check(
        "settings: Bitwarden shows Unlocked",
        find_row(&root, "Unlocked").is_some(),
        "",
    );
    // Scroll down to Bitwarden for the picture.
    if let Some(r) = find_row(&root, "Unlocked") {
        r.grab_focus();
    }
    Run::sleep(300).await;
    run.shots("07-settings-bitwarden").await;
    dialog.close();
    Run::sleep(300).await;

    // One-time code: after picking the account the code box offers its code.
    let tab = run.open("login.html").await;
    run.click(&tab, "#otp").await;
    let coded = run
        .until(8, || {
            run.forms
                .picker_rows(&tab)
                .is_some_and(|r| r.iter().any(|u| u.starts_with("code:")))
        })
        .await;
    run.check(
        "otp: the code box offers the vault's one-time code",
        coded,
        format!("{:?}", run.forms.picker_rows(&tab)),
    );
    run.shot("08-otp-picker-light");
    run.forms.with_picker(&tab, |p| p.press(0));
    let mut code = Value::Null;
    for _ in 0..50 {
        code = run.js(&tab, "document.getElementById('otp').value").await;
        if code
            .as_str()
            .is_some_and(|s| s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()))
        {
            break;
        }
        Run::sleep(100).await;
    }
    run.check(
        "otp: picking it fills a 6-digit code",
        code.as_str()
            .is_some_and(|s| s.len() == 6 && s.chars().all(|c| c.is_ascii_digit())),
        "",
    );
    run.shell.close(&tab);

    // Agents: nothing shared → refused; shared → filled, no secret returned.
    let tab = run.open("login.html").await;
    let refused = super::mcp::sign_in::<super::GtkForms>(
        run.shell.clone(),
        tab.clone(),
        serde_json::Map::new(),
    )
    .await;
    run.check(
        "mcp sign_in: refused while nothing is shared",
        refused.as_ref().is_err_and(|e| e.contains("not shared")),
        format!("{refused:?}"),
    );
    run.shell
        .update_settings(|s| s.set(super::keys::SHARE_ALL, true));
    let mut args = serde_json::Map::new();
    args.insert("account".into(), json!(USER));
    args.insert("submit".into(), json!(false));
    let r = super::mcp::sign_in::<super::GtkForms>(run.shell.clone(), tab.clone(), args).await;
    let text = format!("{r:?}");
    let filled = run
        .js(&tab, &format!("document.getElementById('password').value.length > 0 && document.getElementById('username').value === {}", json!(USER)))
        .await;
    run.check(
        "mcp sign_in: fills in-process; the result has no secret",
        r.as_ref().is_ok_and(|v| v["filled"] == json!(true))
            && filled == json!(true)
            && !text.contains(PASS)
            && !text.contains("dev-pw"),
        text.clone(),
    );
    let mut args = serde_json::Map::new();
    args.insert("account".into(), json!("dev"));
    args.insert("what".into(), json!("otp"));
    args.insert("submit".into(), json!(false));
    let r = super::mcp::sign_in::<super::GtkForms>(run.shell.clone(), tab.clone(), args).await;
    let code = run.js(&tab, "document.getElementById('otp').value").await;
    run.check(
        "mcp sign_in what=otp: fills the code, returns none",
        r.as_ref()
            .is_ok_and(|v| v["filled"] == json!(true) && v.get("code").is_none())
            && code.as_str().is_some_and(|s| s.len() == 6)
            && !format!("{r:?}").contains(code.as_str().unwrap_or("x")),
        format!("{r:?}"),
    );
    let mut args = serde_json::Map::new();
    args.insert("kind".into(), json!("field"));
    args.insert("name".into(), json!("PIN"));
    let r = super::mcp::autofill(run.shell.clone(), tab.clone(), args).await;
    let pin = run.js(&tab, "document.getElementById('pin').value").await;
    run.check(
        "mcp autofill kind=field: fills the PIN, returns no value",
        r.is_ok() && pin == json!("9876") && !format!("{r:?}").contains("9876"),
        format!("{r:?}"),
    );
    run.shell.close(&tab);

    let tab = run.open("checkout.html").await;
    let mut args = serde_json::Map::new();
    args.insert("kind".into(), json!("card"));
    let r = super::mcp::autofill(run.shell.clone(), tab.clone(), args).await;
    let number = run
        .js(
            &tab,
            "document.getElementById('cc-number').value.replace(/\\s/g,'')",
        )
        .await;
    run.check(
        "mcp autofill kind=card: fills the card, returns no number",
        r.is_ok() && number == json!("4111111111111111") && !format!("{r:?}").contains("4111"),
        format!("{r:?}"),
    );
    let mut args = serde_json::Map::new();
    args.insert("kind".into(), json!("identity"));
    let r = super::mcp::autofill(run.shell.clone(), tab.clone(), args).await;
    let city = run.js(&tab, "document.getElementById('city').value").await;
    run.check(
        "mcp autofill kind=identity: fills the address",
        r.is_ok() && city == json!("Springfield"),
        format!("{r:?}"),
    );
    run.shot("09-checkout-filled-light");
    run.shell.close(&tab);
    run.shell
        .update_settings(|s| s.set(super::keys::SHARE_ALL, false));
}

async fn chrome_import(run: &Rc<Run>) {
    let found = super::import::browsers();
    let chrome = found.iter().find(|b| b.name == "Chrome").cloned();
    run.check(
        "import: Chrome profile found",
        chrome.is_some(),
        format!("{:?}", found.iter().map(|b| &b.name).collect::<Vec<_>>()),
    );
    let Some(chrome) = chrome else { return };

    super::import::present(&run.shell);
    Run::sleep(800).await;
    run.shots("10-import-sheet").await;
    let spaces_before = run.shell.browser().spaces().count();
    let bookmarks_before = run.shell.bookmarks().count();
    super::import::LAST.with(|l| l.borrow_mut().take());
    let Some(sheet) = super::import::current() else {
        run.check("import: the sheet opens", false, "");
        return;
    };
    let sheet_w: gtk::Widget = sheet.clone().upcast();
    let mut go = None;
    walk(&sheet_w, &mut |w| {
        if go.is_none()
            && let Some(b) = w.downcast_ref::<gtk::Button>()
            && b.label().as_deref() == Some("Bring In")
        {
            go = Some(b.clone());
        }
    });
    if let Some(b) = go {
        b.emit_clicked();
    }
    let finished = run
        .until(60, || super::import::LAST.with(|l| l.borrow().is_some()))
        .await;
    Run::sleep(400).await;
    run.check("import: the sheet runs and ends on a summary", finished, "");
    run.shots("10b-import-summary").await;
    sheet.close();
    Run::sleep(300).await;
    let Some(sum) = super::import::LAST.with(|l| l.borrow().clone()) else {
        return;
    };
    let choice = super::import::Choice {
        browser: chrome,
        profile: None,
        bookmarks: true,
        history: true,
        tabs: true,
        passwords: true,
    };
    println!("import summary: {sum:?}; lines: {:?}", sum.lines(&choice));
    run.check(
        "import: bookmarks",
        sum.bookmarks > 0 && run.shell.bookmarks().count() > bookmarks_before,
        format!(
            "{} read; now {}",
            sum.bookmarks,
            run.shell.bookmarks().count()
        ),
    );
    let in_history = run
        .shell
        .history()
        .get("https://alpha.example/one")
        .is_some()
        || run
            .shell
            .history()
            .visits()
            .any(|v| v.url.contains("alpha.example"));
    run.check(
        "import: history",
        sum.history >= 2 && in_history,
        format!("{} places", sum.history),
    );
    let sleeping = {
        let b = run.shell.browser();
        let spaces: Vec<String> = b
            .spaces()
            .skip(spaces_before)
            .map(|s| s.id.clone())
            .collect();
        let tabs: Vec<bool> = spaces
            .iter()
            .flat_map(|s| b.row(s))
            .map(|t| t.sleeping)
            .collect();
        (spaces.len(), tabs.len(), tabs.iter().all(|s| *s))
    };
    run.check(
        "import: open tabs as sleeping tabs in new spaces",
        sum.spaces >= 1
            && sleeping.0 == sum.spaces
            && sleeping.1 == sum.tabs
            && sleeping.2
            && sum.tabs > 0,
        format!("{} tabs in {} spaces; {sleeping:?}", sum.tabs, sum.spaces),
    );
    let v = run.forms.vault.clone();
    let logins = rt::run(async move { v.logins().await })
        .await
        .and_then(Result::ok)
        .unwrap_or_default();
    run.check(
        "import: passwords into the keyring",
        sum.passwords >= 1
            && logins
                .iter()
                .any(|l| l.host == "example.com" && l.user == "ann"),
        format!("{} saved; problems {:?}", sum.passwords, sum.problems),
    );
    let first_new = run
        .shell
        .browser()
        .spaces()
        .nth(spaces_before)
        .map(|s| s.id.clone());
    if let Some(id) = first_new {
        run.shell.switch_space(&id);
    }
    Run::sleep(600).await;
    run.shots("11-after-import").await;
}

/// No Secret Service at all: Settings says so plainly, nothing is offered
/// for saving, and the import leaves passwords out.
async fn no_keyring(run: &Rc<Run>) {
    let v = run.forms.vault.clone();
    let settled = run
        .until(25, || {
            matches!(v.snapshot().keyring, super::vault::Keyring::Unavailable(_))
        })
        .await;
    run.check(
        "no keyring: detected",
        settled,
        format!("{:?}", v.snapshot().keyring),
    );
    let dialog = crate::window::settings::dialog(&run.shell);
    dialog.set_visible_page_name("passwords");
    Run::sleep(1500).await;
    let root: gtk::Widget = dialog.clone().upcast();
    run.check(
        "no keyring: Settings › Passwords says so",
        find_row(&root, "No keyring is available").is_some(),
        "",
    );
    if let Some(g) = find_group(&root, "Keyring") {
        scroll_to(&g);
    }
    Run::sleep(300).await;
    run.shots("12-settings-no-keyring").await;
    dialog.close();
    Run::sleep(300).await;
    let tab = run.open("login.html").await;
    run.sign_in_by_hand(&tab, USER, PASS).await;
    Run::sleep(6000).await;
    let signed = run
        .js(&tab, "document.getElementById('result').textContent")
        .await;
    run.check(
        "no keyring: a sign-in that worked is not offered (nowhere to keep it)",
        !run.forms.offering(&tab) && signed.as_str().is_some_and(|s| s.contains("Signed in")),
        "",
    );
    run.shell.close(&tab);
}
