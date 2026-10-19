//! Engine capabilities, engine-wide page scripts, and the file chooser tied
//! to its page (QA W5 P1-4, P1-6, P2-6).

use std::time::{Duration, Instant};

use copper_engine_webkitgtk::copper_engine::{
    Engine, InjectionTime, InputEvent, PageOptions, PageView, Trust, UserScript,
};
use gtk4::prelude::*;
use serde_json::json;

use crate::{Ctx, Res, Tab, ensure, sleep};

/// The file input on `/file`, its centre.
const FILE: (f64, f64) = (120.0, 35.0);

/// Poll `cond` every 25 ms until it holds or `ms` pass.
async fn until(ms: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while !cond() {
        if Instant::now() > deadline {
            return false;
        }
        sleep(25).await;
    }
    true
}

/// Poll a page expression until it equals `want`.
async fn until_js(tab: &Tab, code: &str, want: serde_json::Value, ms: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        let got = tab.js(code).await?;
        if got == want {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("{code} = {got}, want {want}"));
        }
        sleep(25).await;
    }
}

/// The in-process chooser dialogs open now (GtkFileDialog without a portal).
fn dialogs() -> Vec<gtk4::Window> {
    gtk4::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk4::Window>().ok())
        .filter(|w| w.type_().name() == "GtkFileChooserDialog" && w.is_visible())
        .collect()
}

/// Select `path` in the in-process chooser dialog, as a person would.
#[allow(deprecated)] // GtkFileChooserDialog: what GtkFileDialog shows without a portal.
fn pick(dialog: &gtk4::Window, path: &std::path::Path) -> Result<(), String> {
    use gtk4::prelude::FileChooserExt as _;
    let chooser = dialog
        .clone()
        .downcast::<gtk4::FileChooserDialog>()
        .map_err(|_| "not a GtkFileChooserDialog".to_string())?;
    chooser
        .set_file(&gio::File::for_path(path))
        .map_err(|e| e.to_string())
}

/// Press the dialog's Open.
#[allow(deprecated)]
fn accept(dialog: &gtk4::Window) -> Result<(), String> {
    let chooser = dialog
        .clone()
        .downcast::<gtk4::FileChooserDialog>()
        .map_err(|_| "not a GtkFileChooserDialog".to_string())?;
    chooser.response(gtk4::ResponseType::Accept);
    Ok(())
}

/// `Engine::capabilities` agrees with what pages actually get, and each
/// capability the build has is switched on.
pub async fn check_capabilities(ctx: &mut Ctx) -> Res {
    let caps = ctx.engine.capabilities();
    let tab = &mut ctx.b;
    tab.load(&ctx.srv.url("/")).await?;
    let page = tab
        .js("({webrtc: typeof RTCPeerConnection === 'function', \
              media: !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia), \
              eme: typeof navigator.requestMediaKeySystemAccess === 'function', \
              authn: typeof PublicKeyCredential === 'function', \
              secure: window.isSecureContext})")
        .await?;
    ensure(page["secure"] == true, || {
        format!("not a secure context: {page}")
    })?;
    let want = json!({
        "webrtc": caps.webrtc,
        "media": caps.media_stream,
        "eme": caps.encrypted_media,
        "authn": caps.web_authn,
        "secure": true,
    });
    ensure(page == want, || {
        format!("capabilities {caps:?} but the page sees {page}")
    })?;
    let missing: Vec<&str> = caps.unavailable().iter().map(|c| c.name()).collect();
    Ok(format!(
        "{caps:?}; page agrees; this build lacks: [{}]",
        missing.join(", ")
    ))
}

/// `Engine::add_page_script` reaches open pages (next document) and new
/// ones; removing it takes it off both.
pub async fn check_page_scripts(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/");
    let mut script = UserScript::new("window.__everyPage = (window.__everyPage || 0) + 1;");
    script.injection_time = InjectionTime::DocumentStart;
    let id = ctx.engine.add_page_script(&script);
    let probe = "window.__everyPage ?? null";
    ctx.a.load(&url).await?;
    let open = ctx.a.js(probe).await?;
    let mut fresh = Tab::new(ctx.engine.new_page(&ctx.beta, PageOptions::default()));
    fresh.load(&url).await?;
    let new = fresh.js(probe).await?;
    ensure(open == 1 && new == 1, || {
        format!("open page {open}, new page {new}")
    })?;
    ensure(ctx.engine.remove_page_script(id), || {
        "remove returned false".into()
    })?;
    ensure(!ctx.engine.remove_page_script(id), || {
        "second remove returned true".into()
    })?;
    ctx.a.load(&url).await?;
    fresh.load(&url).await?;
    let (a, f) = (ctx.a.js(probe).await?, fresh.js(probe).await?);
    ensure(a.is_null() && f.is_null(), || {
        format!("after removal: open page {a}, new page {f}")
    })?;
    Ok("added to an open page (next load) and a new one; removed from both".into())
}

/// The file chooser belongs to its page: one at a time, closed by Escape
/// from an agent, by navigation, by leaving the window and by the page
/// going away; a choice reaches the page.
pub async fn check_file_chooser(ctx: &mut Ctx) -> Res {
    let url = ctx.srv.url("/file");
    ctx.a.load(&url).await?;
    ctx.b.load(&url).await?;
    sleep(200).await;
    let a = &ctx.a;
    let b = &ctx.b;

    // Open: a trusted click on the input.
    ensure(dialogs().is_empty(), || "a chooser is already open".into())?;
    a.input(InputEvent::click(FILE.0, FILE.1)).await?;
    ensure(until(3000, || a.page.has_file_chooser()).await, || {
        "clicking the file input opened no chooser".into()
    })?;
    ensure(until(3000, || dialogs().len() == 1).await, || {
        format!("{} chooser dialogs", dialogs().len())
    })?;

    // One at a time: another page's request is answered "cancelled".
    b.input(InputEvent::click(FILE.0, FILE.1)).await?;
    until_js(b, "fc.cancels", json!(1), 3000).await?;
    ensure(!b.page.has_file_chooser() && dialogs().len() == 1, || {
        format!(
            "second chooser: page b has one {}, {} dialogs",
            b.page.has_file_chooser(),
            dialogs().len()
        )
    })?;

    // Escape from an agent closes it; the page hears `cancel`.
    let t = a.input(InputEvent::key("Escape")).await?;
    ensure(t == Trust::Trusted && !a.page.has_file_chooser(), || {
        format!("Escape: {t:?}, still open {}", a.page.has_file_chooser())
    })?;
    until_js(a, "fc.cancels", json!(1), 3000).await?;
    ensure(until(3000, || dialogs().is_empty()).await, || {
        format!("{} dialogs after Escape", dialogs().len())
    })?;
    // With no chooser, Escape is an ordinary key again.
    a.js("window.esc = 0; document.addEventListener('keydown', (e) => { if (e.key === 'Escape') esc++; }); true")
        .await?;
    a.input(InputEvent::key("Escape")).await?;
    let esc = a.js("esc").await?;
    ensure(esc == 1, || format!("page saw {esc} Escape keydowns"))?;

    // Navigation closes it.
    a.input(InputEvent::click(FILE.0, FILE.1)).await?;
    ensure(until(3000, || a.page.has_file_chooser()).await, || {
        "chooser did not reopen".into()
    })?;
    ctx.a.load(&url).await?;
    ensure(!ctx.a.page.has_file_chooser(), || {
        "navigating left the chooser open".into()
    })?;
    ensure(until(3000, || dialogs().is_empty()).await, || {
        format!("{} dialogs after navigating", dialogs().len())
    })?;

    // A choice reaches the page.
    let path = std::env::temp_dir().join(format!("copper-chooser-{}.txt", std::process::id()));
    std::fs::write(&path, b"chosen").map_err(|e| e.to_string())?;
    let a = &ctx.a;
    a.input(InputEvent::click(FILE.0, FILE.1)).await?;
    ensure(until(3000, || dialogs().len() == 1).await, || {
        "chooser did not open for the choice".into()
    })?;
    pick(&dialogs().remove(0), &path)?;
    sleep(500).await;
    accept(&dialogs().remove(0))?;
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    until_js(a, "fc.names", json!([name]), 3000).await?;
    ensure(
        !a.page.has_file_chooser() && until(3000, || dialogs().is_empty()).await,
        || "chooser still open after the choice".into(),
    )?;
    let _ = std::fs::remove_file(&path);

    // Leaving the window (a closed tab's view is taken out) closes it.
    let mut temp = Tab::new(ctx.engine.new_page(&ctx.alpha, PageOptions::default()));
    let window = gtk4::Window::builder()
        .title("chooser owner")
        .default_width(400)
        .default_height(200)
        .child(&temp.page.widget())
        .build();
    window.present();
    temp.load(&url).await?;
    sleep(200).await;
    temp.input(InputEvent::click(FILE.0, FILE.1)).await?;
    ensure(until(3000, || temp.page.has_file_chooser()).await, || {
        "temp page opened no chooser".into()
    })?;
    window.set_child(gtk4::Widget::NONE);
    ensure(!temp.page.has_file_chooser(), || {
        "chooser outlived its view leaving the window".into()
    })?;
    ensure(until(3000, || dialogs().is_empty()).await, || {
        format!("{} dialogs after the view left", dialogs().len())
    })?;

    // Dropping the page closes it too, even with its view still shown.
    window.set_child(Some(&temp.page.widget()));
    temp.load(&url).await?;
    sleep(200).await;
    temp.input(InputEvent::click(FILE.0, FILE.1)).await?;
    ensure(until(3000, || temp.page.has_file_chooser()).await, || {
        "temp page opened no chooser the second time".into()
    })?;
    drop(temp);
    ensure(until(3000, || dialogs().is_empty()).await, || {
        format!("{} dialogs after the page was dropped", dialogs().len())
    })?;
    window.destroy();
    Ok(format!(
        "opens on a trusted click; second request cancelled; Escape, navigation, choice ({name}), leaving the window and dropping the page each close it"
    ))
}
