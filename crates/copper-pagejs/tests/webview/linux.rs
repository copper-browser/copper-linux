//! The WebKitGTK body of the `webview` test target (see `tests/webview.rs`).

use std::collections::BTreeMap;

use crate::support::{H, Res, sleep};
use copper_pagejs::page::{self, Locator};
use copper_pagejs::render::render;
use copper_pagejs::tab::{self, ReaderOutcome};
use copper_pagejs::{Script, canvas, curtain, forms, jev, messages, probe, trail};
use serde_json::{Value, json};

pub fn main() {
    crate::support::main(&[
        case!(every_script_parses),
        case!(mcp_sequence_matches_mac),
        case!(mcp_rect_and_centres),
        case!(mcp_click_hover_key),
        case!(mcp_typing_native_setter),
        case!(mcp_drag),
        case!(mcp_scroll_wait_height),
        case!(mcp_console_after_install),
        case!(mcp_console_from_document_start),
        case!(mcp_rect_on_smooth_scroll_page),
        case!(mcp_expression_path),
        case!(jev_observe_matches_mac),
        case!(jev_actions),
        case!(trail_layer),
        case!(perf_probe_text),
        case!(perf_probe_json),
        case!(forms_login),
        case!(forms_autofill),
        case!(forms_without_passkeys),
        case!(curtain_style_and_peek),
        case!(curtain_picker),
        case!(reader_mode),
        case!(tab_plumbing),
        case!(tab_relays),
        case!(canvas_bridge),
        case!(canvas_frames),
    ]);
}

fn golden(name: &str) -> String {
    let path = format!("{}/tests/golden/mac/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The JavaScript-produced part of a tool result: what follows `Tools.pageLine`.
fn after_page(text: &str) -> &str {
    if text.starts_with("### Page\n") {
        text.split_once("\n\n").map(|(_, rest)| rest).unwrap_or("")
    } else {
        text
    }
}

fn r(id: &str) -> Locator {
    Locator::Ref(id.into())
}

fn css(sel: &str) -> Locator {
    Locator::Css(sel.into())
}

/// The ref in a `browser_find` line: `- text "Drag me" [ref=e24]`.
fn ref_in(line: &str) -> Option<String> {
    let start = line.find("[ref=")? + 5;
    let end = line[start..].find(']')? + start;
    Some(line[start..end].to_string())
}

// ------------------------------------------------------------------ all

/// Every asset compiles in WebKit: its function expression evaluates to a function.
async fn every_script_parses(h: &H) -> Res {
    h.load("mcp.html").await?;
    for &script in Script::ALL {
        let js = format!("typeof ({})", script.function());
        let kind = h
            .js(&js)
            .await
            .map_err(|e| format!("{}: {e}", script.file()))?;
        ensure!(kind == "function", "{} is {kind}", script.file());
    }
    Ok(())
}

// ------------------------------------------------------------------ MCP page tools

/// The same calls, in the same order, as the macOS capture: byte-identical text.
async fn mcp_sequence_matches_mac(h: &H) -> Res {
    h.load("mcp.html?golden=1").await?;
    let same = |name: &str, got: String| -> Res {
        let want = golden(name);
        let want = after_page(&want);
        ensure!(
            got == want,
            "{name} differs from the Mac\n--- linux\n{got}\n--- mac\n{want}"
        );
        Ok(())
    };
    same(
        "mcp-01-snapshot",
        h.run(&page::snapshot(None, false, 600)).await?,
    )?;
    same(
        "mcp-02-snapshot-interactive",
        h.run(&page::snapshot(None, true, 600)).await?,
    )?;
    same(
        "mcp-03-snapshot-form-limit",
        h.run(&page::snapshot(Some("form"), false, 5)).await?,
    )?;
    same("mcp-04-find", h.run(&page::find_text("item")).await?)?;
    same("mcp-05-find-none", h.run(&page::find_text("zebra")).await?)?;
    // Known engine difference, not a script one: Safari's innerText includes a
    // <select>'s option texts, WebKitGTK 2.52's does not. Everything else matches.
    let text = h.run(&page::text(None)).await?;
    let mac = golden("mcp-06-text");
    let mac = mac.replace("Red\nGreen\nBlue", "");
    ensure!(
        text == mac,
        "mcp-06-text differs beyond the <select> innerText gap\n--- linux\n{text}\n--- mac\n{mac}"
    );
    same(
        "mcp-07-text-out",
        h.run(&page::text(Some(&css("#out")))).await?,
    )?;
    same("mcp-08-console", h.run(&page::console()).await?)?;
    same(
        "mcp-09-select",
        h.run(&page::select(&r("e13"), &["Green".into()])).await?,
    )?;
    same(
        "mcp-10-select-none",
        h.run(&page::select(&r("e13"), &["Purple".into()])).await?,
    )?;
    // browser_fill_form: "<name>: <render(result)>" per field.
    let fields = [
        ("Name", "e4", "textbox", "Ada"),
        ("Agree", "e8", "checkbox", "true"),
        ("Color", "e13", "combobox", "b"),
        ("Volume", "e18", "slider", "7"),
        ("Ghost", "e999", "textbox", "x"),
    ];
    let mut lines = Vec::new();
    for (name, reference, kind, value) in fields {
        let call = page::fill(reference, kind, value);
        let json = h.run_json(&call).await?;
        let value: Option<Value> = json.map(|j| serde_json::from_str(&j).unwrap());
        lines.push(format!("{name}: {}", render(value.as_ref())));
    }
    same("mcp-11-fill-form", lines.join("\n"))?;
    same(
        "mcp-12-text-out",
        h.run(&page::text(Some(&css("#out")))).await?,
    )?;
    same(
        "mcp-13-snapshot-interactive",
        h.run(&page::snapshot(None, true, 600)).await?,
    )?;
    let evaluate = async |function: &str, reference: Option<&str>| -> Result<String, String> {
        let call = page::evaluate(function, reference);
        let json = h.run_json(&call).await?;
        let value: Option<Value> = json.map(|j| serde_json::from_str(&j).unwrap());
        Ok(render(value.as_ref()))
    };
    same(
        "mcp-14-evaluate-object",
        evaluate(
            "() => ({b: [1, 2.5, 'x/y'], a: null, c: {}, callSites: 1, callbacks: 2})",
            None,
        )
        .await?,
    )?;
    same(
        "mcp-15-evaluate-ref",
        evaluate("(el) => el.id", Some("e4")).await?,
    )?;
    same(
        "mcp-16-evaluate-number",
        evaluate("() => 1 + 1", None).await?,
    )?;
    same("mcp-17-evaluate-bool", evaluate("() => true", None).await?)?;
    same(
        "mcp-18-evaluate-undefined",
        evaluate("() => undefined", None).await?,
    )?;
    same(
        "mcp-19-text-missing",
        h.run(&page::text(Some(&css("#nope")))).await?,
    )?;
    Ok(())
}

async fn mcp_rect_and_centres(h: &H) -> Res {
    h.load("mcp.html").await?;
    h.run(&page::snapshot(None, true, 600)).await?;
    let target = r("e19");
    let rect = h.run(&page::rect(&target)).await?.into_result(&target)?;
    ensure!(
        rect.width > 10.0 && rect.height > 5.0,
        "send button rect {rect:?}"
    );
    let (x, y) = rect.center();
    let hit = h
        .js(&format!(
            "document.elementFromPoint({x}, {y}) && document.elementFromPoint({x}, {y}).id"
        ))
        .await?;
    ensure_eq!(hit, json!("go"));
    // A css locator works too, and a stale ref says what to do.
    let rect = h
        .run(&page::rect(&css("#name")))
        .await?
        .into_result(&css("#name"))?;
    ensure!(rect.width > 0.0, "name rect {rect:?}");
    let missing = h
        .run(&page::rect(&r("e404")))
        .await?
        .into_result(&r("e404"));
    ensure_eq!(
        missing,
        Err::<page::Rect, String>(
            "ref:e404: no element for ref:e404. Take a fresh browser_snapshot — refs change when the page does.".into()
        )
    );
    Ok(())
}

async fn mcp_click_hover_key(h: &H) -> Res {
    h.load("mcp.html").await?;
    h.run(&page::snapshot(None, false, 600)).await?;
    ensure_eq!(h.run(&page::click(&r("e19"), false, "left")).await?, "ok");
    ensure_eq!(h.run(&page::hover(&r("e22"))).await?, "ok");
    ensure_eq!(h.run(&page::click(&r("e23"), true, "left")).await?, "ok");
    ensure_eq!(h.run(&page::click(&r("e23"), false, "right")).await?, "ok");
    ensure_eq!(h.run(&page::key("Escape")).await?, "ok");
    ensure_eq!(h.run(&page::focus(&r("e4"))).await?, "ok");
    ensure_eq!(h.run(&page::focus(&r("e404"))).await?, "no element");
    ensure_eq!(h.run(&page::submit(&r("e4"))).await?, "ok");
    ensure_eq!(
        h.run(&page::click(&r("e404"), false, "left")).await?,
        "no element for ref:e404"
    );
    let out = h
        .js_str("document.getElementById('out').textContent")
        .await?;
    ensure_eq!(
        out,
        "clicked Send | submitted | hovered | double | context | escape | submitted"
    );
    Ok(())
}

async fn mcp_typing_native_setter(h: &H) -> Res {
    h.load("mcp.html").await?;
    // A framework that tracks the value through the prototype setter still sees it.
    h.js("(function(){ var el = document.getElementById('name'); var d = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value'); \
          Object.defineProperty(el, 'value', { get: function(){ return d.get.call(this); }, set: function(v){ window.__ownSetter = (window.__ownSetter||0)+1; d.set.call(this, v); } }); return true; })()")
        .await?;
    ensure_eq!(h.run(&page::set_value(&css("#name"), "Linus")).await?, "ok");
    ensure_eq!(
        h.js("document.getElementById('name').value").await?,
        json!("Linus")
    );
    ensure_eq!(h.js("window.__ownSetter || 0").await?, json!(0));
    ensure_eq!(h.js("document.activeElement.id").await?, json!("name"));
    ensure_eq!(
        h.run(&page::set_value(&css("#editor"), "rich")).await?,
        "ok"
    );
    ensure_eq!(
        h.js("document.getElementById('editor').textContent")
            .await?,
        json!("rich")
    );
    ensure_eq!(
        h.run(&page::set_value(&css("label[for=name]"), "x"))
            .await?,
        "not editable: LABEL"
    );
    ensure_eq!(h.run(&page::set_value(&css("main"), "x")).await?, "ok"); // the first field inside
    let out = h
        .js_str("document.getElementById('out').textContent")
        .await?;
    ensure!(
        out.starts_with("input Linus | change Linus"),
        "events: {out}"
    );
    // A key into a text box types it, through the same setter.
    h.js("document.getElementById('notes').focus()").await?;
    h.run(&page::key("z")).await?;
    ensure_eq!(
        h.js("document.getElementById('notes').value").await?,
        json!("z")
    );
    Ok(())
}

async fn mcp_drag(h: &H) -> Res {
    h.load("mcp.html").await?;
    let from = ref_in(&h.run(&page::find_text("Drag me")).await?).ok_or("no ref for drag")?;
    let to = ref_in(&h.run(&page::find_text("Drop here")).await?).ok_or("no ref for drop")?;
    ensure_eq!(h.run(&page::drag(&from, &to)).await?, "ok");
    ensure_eq!(
        h.js("document.getElementById('out').textContent").await?,
        json!("dropped")
    );
    ensure_eq!(h.run(&page::drag(&from, "e404")).await?, "no element");
    Ok(())
}

async fn mcp_scroll_wait_height(h: &H) -> Res {
    h.load("jev.html").await?;
    ensure_eq!(h.run(&page::scroll(None, 0.0, 250.0)).await?, "ok");
    ensure_eq!(h.js("window.scrollY").await?, json!(250));
    ensure_eq!(h.run(&page::has_text("Book a trip")).await?, Some(true));
    ensure_eq!(h.run(&page::has_text("zebra")).await?, Some(false));
    let height = h.run(&page::document_height()).await?;
    ensure!(height > 2400.0 && height <= 20000.0, "height {height}");
    // Scrolling within an element by ref.
    h.load("mcp.html").await?;
    let inner = ref_in(&h.run(&page::find_text("Tall inner content")).await?).ok_or("no ref")?;
    h.js("document.getElementById('pane').firstElementChild.style.overflow = 'auto'; document.getElementById('pane').firstElementChild.style.height = '50px'; document.getElementById('pane').firstElementChild.innerHTML = '<div style=\"height:400px\">x</div>'").await?;
    ensure_eq!(h.run(&page::scroll(Some(&inner), 0.0, 30.0)).await?, "ok");
    ensure_eq!(
        h.js("document.getElementById('pane').firstElementChild.scrollTop")
            .await?,
        json!(30)
    );
    Ok(())
}

async fn mcp_console_after_install(h: &H) -> Res {
    h.load("mcp.html").await?;
    // Captured from the helper's install on (as on the Mac: lazily).
    ensure_eq!(h.run(&page::console()).await?, "[]");
    h.js(
        "console.log('a', {b: 1}); console.error('bad'); console.info(2); console.warn('w'); true",
    )
    .await?;
    h.js("setTimeout(function(){ throw new Error('boom'); }, 0); true")
        .await?;
    sleep(100).await;
    let json = h.run(&page::console()).await?;
    let entries = page::parse_console(&json).map_err(|e| e.to_string())?;
    let got: Vec<(String, String)> = entries.into_iter().map(|e| (e.level, e.text)).collect();
    let want: Vec<(String, String)> = [
        ("log", "a {\"b\":1}"),
        ("error", "bad"),
        ("info", "2"),
        ("warn", "w"),
        ("error", "Error: boom"),
    ]
    .iter()
    .map(|(l, t)| (l.to_string(), t.to_string()))
    .collect();
    ensure_eq!(got, want);
    Ok(())
}

/// Linux: `page/console_early.js` from document start keeps what the page
/// logged while loading, in the helper's own entries; the helper adopts that
/// log rather than hooking `console` twice.
async fn mcp_console_from_document_start(h: &H) -> Res {
    h.add_user_script(&page::console_early());
    h.load("console.html").await?;
    h.js("console.log('after', [1, 2]); true").await?;
    let json = h.run(&page::console()).await?;
    let entries = page::parse_console(&json).map_err(|e| e.to_string())?;
    let got: Vec<(String, String)> = entries.into_iter().map(|e| (e.level, e.text)).collect();
    let want: Vec<(String, String)> = [
        ("log", "head {\"n\":1}"),
        ("info", "3"),
        ("warn", "body"),
        ("error", "Error: load boom"),
        ("error", "loaded"),
        ("log", "after [1,2]"),
    ]
    .iter()
    .map(|(l, t)| (l.to_string(), t.to_string()))
    .collect();
    ensure_eq!(got, want);
    // Installed once: the helper did not wrap console a second time.
    h.js("console.warn('once'); true").await?;
    let json = h.run(&page::console()).await?;
    let entries = page::parse_console(&json).map_err(|e| e.to_string())?;
    let once = entries.iter().filter(|e| e.text == "once").count();
    ensure_eq!(once, 1);
    // The early log is hidden from enumeration and cannot be replaced.
    ensure_eq!(
        h.js("Object.keys(window).includes('__copperLogs')").await?,
        json!(false)
    );
    ensure_eq!(
        h.js(
            "(function () { window.__copperLogs = []; return window.__copperLogs.length > 0; })()"
        )
        .await?,
        json!(true)
    );
    // Without the user script: the Mac's lazy capture, unchanged.
    h.clear_user_scripts();
    h.load("console.html").await?;
    ensure_eq!(h.run(&page::console()).await?, "[]");
    Ok(())
}

/// Linux: on a page with `scroll-behavior: smooth`, `rect` scrolls the
/// target into view at once, so the box it answers is where the element is
/// and a click at its centre lands on it (QA W5 P1-3).
async fn mcp_rect_on_smooth_scroll_page(h: &H) -> Res {
    h.load("smooth.html").await?;
    ensure_eq!(h.js("window.scrollY").await?, json!(0));
    for day in ["d15", "d29", "d3"] {
        let located = h.run(&page::rect(&css(&format!("#{day}")))).await?;
        let rect = located
            .into_result(&css(day))
            .map_err(|e| format!("{day}: {e}"))?;
        let (x, y) = rect.center();
        let hit = h
            .js(&format!(
                "(function () {{ var e = document.elementFromPoint({x}, {y}); return e ? e.id : null; }})()"
            ))
            .await?;
        ensure!(
            hit == json!(day),
            "{day}: centre ({x}, {y}) hits {hit} (scrollY {})",
            h.js("window.scrollY").await?
        );
        // Nothing still animating: the box is the same a beat later.
        sleep(250).await;
        let again = h
            .run(&page::rect(&css(&format!("#{day}"))))
            .await?
            .into_result(&css(day))?;
        ensure_eq!(again, rect);
    }
    // The DOM click path (no real input) picks the same cell.
    ensure_eq!(
        h.run(&page::click(&css("#d22"), false, "left")).await?,
        "ok"
    );
    ensure_eq!(h.js("window.picked").await?, json!(22));
    Ok(())
}

async fn mcp_expression_path(h: &H) -> Res {
    h.load("mcp.html").await?;
    // The evaluate_javascript path (one expression, installer included) agrees.
    let a = h.run_expression(&page::snapshot(None, true, 600)).await?;
    let b = h.run(&page::snapshot(None, true, 600)).await?;
    ensure_eq!(a, b);
    ensure_eq!(h.run_expression(&forms::has_password()).await?, false);
    Ok(())
}

// ------------------------------------------------------------------ Jev

async fn jev_observe_matches_mac(h: &H) -> Res {
    h.load("jev.html?golden=1").await?;
    let obs = h.run(&jev::observe()).await?.ok_or("no observation")?;
    let text: String = obs.text.chars().take(4000).collect();
    let got = format!(
        "### Elements\n{}\n\n### Visible text\n{text}",
        jev::table(&obs, 80, jev::Extras::default())
    );
    let want = golden("jev-01-observe");
    let want = after_page(&want);
    ensure!(
        got == want,
        "jev_observe differs\n--- linux\n{got}\n--- mac\n{want}"
    );
    let got = format!(
        "### Elements\n{}",
        jev::table(&obs, 3, jev::Extras::default())
    );
    let want = golden("jev-02-observe-limit");
    ensure_eq!(got.as_str(), after_page(&want));
    ensure_eq!(jev::read(&obs), "6 controls · Copper Jev fixture");
    ensure!(
        obs.action("scroll_down").is_some() && obs.action("wait").is_some(),
        "controls {:?}",
        obs.actions
    );
    ensure!(obs.action("scroll_up").is_none(), "scroll_up at the top");
    Ok(())
}

async fn jev_actions(h: &H) -> Res {
    h.load("jev.html").await?;
    let obs = h.run(&jev::observe()).await?.ok_or("no observation")?;
    ensure_eq!(h.run(&jev::marker()).await?, Some(obs.marker.clone()));
    let search = obs
        .actions
        .iter()
        .find(|a| a.label == "Search")
        .ok_or("no Search action")?
        .clone();
    let node = search.node.ok_or("no node")?;
    let (key, guard) = h.run(&jev::check(node)).await?;
    ensure_eq!(key, obs.page_key);
    ensure_eq!(guard.as_deref(), obs.guard_value(node));
    let point = h
        .run(&jev::resolve(&search))
        .await?
        .ok_or("search not resolvable")?;
    let hit = h
        .js(&format!(
            "document.elementFromPoint({}, {}).id",
            point.x, point.y
        ))
        .await?;
    ensure_eq!(hit, json!("search"));
    // Type into "Where to?" the way the run does without real input.
    let fill = obs
        .actions
        .iter()
        .find(|a| a.kind == "fill")
        .ok_or("no fill action")?
        .clone();
    let field = fill.node.ok_or("no node")?;
    ensure_eq!(h.run(&jev::dom_click(field)).await?, true);
    ensure_eq!(h.run(&jev::select_all(field)).await?, true);
    ensure_eq!(h.run(&jev::set_value(field, "Paris")).await?, true);
    ensure_eq!(h.run(&jev::settle(&fill)).await?, true);
    ensure_eq!(h.run(&jev::dom_click(node)).await?, true);
    ensure_eq!(
        h.js("document.getElementById('status').textContent")
            .await?,
        json!("searched Paris")
    );
    // The marker moved with the page; the old observation is stale.
    ensure!(
        h.run(&jev::marker()).await? != Some(obs.marker.clone()),
        "marker did not change"
    );
    // A select is done in full by resolve.
    let business = obs
        .actions
        .iter()
        .find(|a| a.kind == "select" && a.value.as_deref() == Some("b"))
        .ok_or("no Business option")?
        .clone();
    ensure!(
        h.run(&jev::resolve(&business)).await?.is_some(),
        "select not resolved"
    );
    ensure_eq!(
        h.js("document.getElementById('cabin').value").await?,
        json!("b")
    );
    // A node that is gone resolves to nothing.
    h.js("document.getElementById('search').remove(); true")
        .await?;
    ensure_eq!(h.run(&jev::resolve(&search)).await?, None);
    ensure_eq!(h.run(&jev::check(node)).await?.1, None);
    // Scroll, and the page offers the way back up.
    ensure_eq!(h.run(&jev::scroll_by(560.0)).await?, true);
    ensure_eq!(h.js("window.scrollY").await?, json!(560));
    let obs = h.run(&jev::observe()).await?.ok_or("no observation")?;
    ensure!(
        obs.action("scroll_up").is_some(),
        "no scroll_up after scrolling"
    );
    // `watch`: false at the cap on a still page, true on the first change.
    ensure_eq!(h.run(&jev::wait_for_change(100)).await?, false);
    h.js("setTimeout(() => document.body.appendChild(document.createElement('p')), 50); true")
        .await?;
    ensure_eq!(h.run(&jev::wait_for_change(1500)).await?, true);
    // Observation round-trips through the typed struct for resolve().
    let again: Value = serde_json::to_value(&search).map_err(|e| e.to_string())?;
    ensure!(
        again.get("rect").is_some() && again.get("node").is_some(),
        "{again}"
    );
    Ok(())
}

// ------------------------------------------------------------------ trail

async fn trail_layer(h: &H) -> Res {
    h.load("jev.html").await?;
    let glide = glib::future_with_timeout(
        std::time::Duration::from_secs(5),
        h.run_json(&trail::glide(120.0, 80.0)),
    )
    .await;
    ensure!(glide.is_ok(), "glide never resolved");
    glide.unwrap()?;
    ensure_eq!(
        h.js("!!document.getElementById('__jev-trail')").await?,
        json!(true)
    );
    for call in [
        trail::target(10.0, 10.0, 100.0, 30.0, "Search", "CLICK"),
        trail::click(60.0, 25.0),
        trail::typed(60.0, 25.0, "hello"),
        trail::scroll(560.0),
        trail::seen(&[json!({"x": 1, "y": 2, "w": 3, "h": 4})]),
        trail::wait(),
    ] {
        h.run_json(&call).await?;
    }
    ensure!(
        h.js_str("document.getElementById('__jev-trail').textContent")
            .await?
            .contains("Search"),
        "the target label is not drawn"
    );
    ensure_eq!(h.run(&trail::clear()).await?, true);
    ensure_eq!(
        h.js("!!document.getElementById('__jev-trail')").await?,
        json!(false)
    );
    // Clearing an untouched page is harmless.
    h.load("mcp.html").await?;
    ensure_eq!(h.run(&trail::clear()).await?, true);
    Ok(())
}

// ------------------------------------------------------------------ perf probe

async fn perf_probe_text(h: &H) -> Res {
    h.load("perf.html").await?;
    let (seconds, top, format) = probe::clamp(Some(0.2), None, false);
    ensure_eq!((seconds, top, format), (1.0, 8, "text"));
    let out = h.run(&probe::perf(seconds, top, format)).await?;
    let probe::PerfOutput::Text(text) = out else {
        return Err(format!("not text: {out:?}"));
    };
    for section in [
        "# Summary",
        "# Frame loop",
        "# DOM churn",
        "# Animations",
        "# Filters",
        "# Canvases",
        "# Timers/long tasks",
        "# Resources",
        "# Page",
    ] {
        ensure!(text.contains(section), "missing {section} in\n{text}");
    }
    ensure!(
        text.contains("animates box-shadow"),
        "no box-shadow finding in\n{text}"
    );
    // The probe puts back what it patched.
    ensure_eq!(
        h.js("String(window.requestAnimationFrame).includes('[native code]')")
            .await?,
        json!(true)
    );
    Ok(())
}

async fn perf_probe_json(h: &H) -> Res {
    h.load("perf.html").await?;
    let out = h.run(&probe::perf(1.0, 3, "json")).await?;
    let probe::PerfOutput::Json(value) = out else {
        return Err(format!("not JSON: {out:?}"));
    };
    let report: probe::PerfReport =
        serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    ensure!(
        report.window_seconds >= 1.0,
        "window {}",
        report.window_seconds
    );
    ensure!(!report.heuristics.is_empty(), "no heuristics");
    let callbacks = value["frameLoop"]["callbacks"].as_f64().unwrap_or(0.0);
    ensure!(callbacks > 0.0, "no rAF callbacks: {}", value["frameLoop"]);
    ensure!(
        value["animations"]["count"].as_f64().unwrap_or(0.0) >= 1.0,
        "animations {}",
        value["animations"]
    );
    ensure!(
        value["canvases"]["count"] == json!(1),
        "canvases {}",
        value["canvases"]
    );
    // Rendered as the Mac renders JSON: sorted, " : ".
    let text = render(Some(&value));
    ensure!(
        text.starts_with("{\n  \"animations\" : {"),
        "{}",
        &text[..80.min(text.len())]
    );
    Ok(())
}

// ------------------------------------------------------------------ forms

async fn forms_login(h: &H) -> Res {
    h.add_user_script(&forms::relay());
    h.load("login.html").await?;
    h.message(messages::FORMS, |m| m["kind"] == "form").await?;
    // Its code box is told once ("Fill two-step codes").
    let otp = h.message(messages::FORMS, |m| m["kind"] == "otp").await?;
    ensure_eq!(
        serde_json::from_value::<messages::FormsMessage>(otp).map_err(|e| e.to_string())?,
        messages::FormsMessage::Otp
    );
    ensure_eq!(h.run(&forms::has_password()).await?, true);
    ensure_eq!(h.run(&forms::has_otp()).await?, true);
    ensure_eq!(
        h.run(&forms::fields_present()).await?,
        vec!["username", "password", "otp", "other"]
    );
    ensure_eq!(h.run(&forms::has_fields("login")).await?, true);
    ensure_eq!(h.run(&forms::has_fields("card")).await?, false);
    ensure_eq!(h.run(&forms::unsaved()).await?, false);
    // Focus reports the field and where it is.
    h.clear_messages();
    h.js("document.getElementById('username').focus(); true")
        .await?;
    let focus = h
        .message(messages::FORMS, |m| {
            m["kind"] == "focus" && m["typing"] == true
        })
        .await?;
    let focus: messages::FormsMessage = serde_json::from_value(focus).map_err(|e| e.to_string())?;
    let messages::FormsMessage::Focus { field, rect, .. } = focus else {
        return Err("not a focus message".into());
    };
    ensure_eq!(
        field,
        Some(messages::FieldInfo {
            kind: "username".into(),
            group: "login".into(),
            label: "Email".into()
        })
    );
    ensure!(rect.is_some_and(|r| r.w > 0.0), "no rect");
    // Fill (never on its own: on request) and send.
    ensure_eq!(
        h.run(&forms::fill_login("ada@example.test", "hunter2"))
            .await?,
        Some(true)
    );
    ensure_eq!(
        h.js("document.getElementById('username').value").await?,
        json!("ada@example.test")
    );
    ensure_eq!(
        h.js("document.getElementById('password').value").await?,
        json!("hunter2")
    );
    ensure_eq!(h.run(&forms::fill_otp("123456")).await?, Some(true));
    ensure_eq!(
        h.js("document.getElementById('otp').value").await?,
        json!("123456")
    );
    h.clear_messages();
    ensure_eq!(h.run(&forms::submit()).await?, Some(true));
    let sent = h
        .message(messages::FORMS, |m| m["kind"] == "submit")
        .await?;
    let sent: messages::FormsMessage = serde_json::from_value(sent).map_err(|e| e.to_string())?;
    ensure_eq!(
        sent,
        messages::FormsMessage::Submit {
            user: "ada@example.test".into(),
            password: "hunter2".into()
        }
    );
    // The form goes away in place: the sign-in settled.
    h.message(messages::FORMS, |m| m["kind"] == "settled")
        .await?;
    ensure_eq!(h.run(&forms::has_password()).await?, false);
    ensure_eq!(
        h.js("document.getElementById('result').textContent")
            .await?,
        json!("Signed in as ada@example.test")
    );
    Ok(())
}

async fn forms_autofill(h: &H) -> Res {
    // Without the relay every call is a harmless no-op.
    h.load("autofill.html").await?;
    ensure_eq!(h.run(&forms::fill_login("a", "b")).await?, None);
    ensure_eq!(h.run(&forms::fields_present()).await?, Vec::<String>::new());
    h.add_user_script(&forms::relay());
    h.load("autofill.html").await?;
    ensure_eq!(h.run(&forms::has_fields("card")).await?, true);
    ensure_eq!(h.run(&forms::has_fields("identity")).await?, true);
    let mut values = BTreeMap::new();
    for (k, v) in [
        ("firstName", "Ada"),
        ("lastName", "Lovelace"),
        ("email", "ada@example.test"),
        ("phone", "555-0100"),
        ("address1", "1 Main St"),
        ("city", "Austin"),
        ("state", "TX"),
        ("postalCode", "78701"),
        ("country", "United Kingdom"),
        ("cardName", "Ada Lovelace"),
        ("cardNumber", "4111111111111111"),
        ("cardExp", "4/29"),
        ("cardCode", "123"),
    ] {
        values.insert(k.to_string(), v.to_string());
    }
    let filled = h
        .run(&forms::fill_values(&values))
        .await?
        .ok_or("no count")?;
    ensure!(filled >= 14, "filled {filled}");
    let read = h
        .js("['first-name','last-name','email','phone','address-line-1','city','state','zip','country','cardholder','card-number','card-exp','cvc','exp-month','exp-year'].map(function(id){ return document.getElementById(id).value; })")
        .await?;
    ensure_eq!(
        read,
        json!([
            "Ada",
            "Lovelace",
            "ada@example.test",
            "555-0100",
            "1 Main St",
            "Austin",
            "TX",
            "78701",
            "GB",
            "Ada Lovelace",
            "4111111111111111",
            "04/29",
            "123",
            "04",
            "2029"
        ])
    );
    // Events a keystroke would fire.
    let events = h
        .js("JSON.parse(document.getElementById('events').textContent)['first-name']")
        .await?;
    ensure_eq!(events, json!({"input": 1, "change": 1}));
    ensure_eq!(
        h.run(&forms::fill_field("Company", "Exowatt")).await?,
        Some(true)
    );
    ensure_eq!(
        h.js("document.getElementById('company').value").await?,
        json!("Exowatt")
    );
    ensure_eq!(
        h.run(&forms::fill_field("Nonexistent field", "x")).await?,
        Some(false)
    );
    h.js("document.getElementById('apt').focus(); true").await?;
    ensure_eq!(h.run(&forms::fill_focused("Apt 2")).await?, Some(true));
    ensure_eq!(
        h.js("document.getElementById('apt').value").await?,
        json!("Apt 2")
    );
    Ok(())
}

async fn forms_without_passkeys(h: &H) -> Res {
    h.add_user_script(&forms::without_passkeys());
    h.load("login.html").await?;
    ensure_eq!(
        h.js("typeof window.PublicKeyCredential").await?,
        json!("undefined")
    );
    ensure_eq!(h.js("'PublicKeyCredential' in window").await?, json!(true));
    Ok(())
}

// ------------------------------------------------------------------ curtain

async fn curtain_style_and_peek(h: &H) -> Res {
    let sheet = curtain::css(["#cookie-bar", "div[data-testid=\"promo\"]"], None);
    h.add_user_script(&curtain::picker());
    h.add_user_script(&curtain::style(&sheet));
    h.load("curtain.html").await?;
    let shown = "['cookie-bar'].map(function(){ return [getComputedStyle(document.getElementById('cookie-bar')).display, getComputedStyle(document.querySelector('[data-testid=promo]')).display]; })[0]";
    ensure_eq!(h.js(shown).await?, json!(["none", "none"]));
    // Peek: the sheet without one selector, and an outline on it.
    let without = curtain::css(
        ["#cookie-bar", "div[data-testid=\"promo\"]"],
        Some("#cookie-bar"),
    );
    h.run_json(&curtain::peek(&without, "#cookie-bar")).await?;
    ensure_eq!(h.js(shown).await?, json!(["block", "none"]));
    ensure!(
        h.js_str("document.getElementById('office-peek').textContent")
            .await?
            .starts_with("#cookie-bar { outline: 2px solid"),
        "no peek outline"
    );
    h.run_json(&curtain::unpeek(&sheet)).await?;
    ensure_eq!(h.js(shown).await?, json!(["none", "none"]));
    ensure_eq!(
        h.js("document.getElementById('office-peek').textContent")
            .await?,
        json!("")
    );
    // Restoring everything: an empty sheet in place.
    h.run_json(&curtain::restyle("")).await?;
    ensure_eq!(h.js(shown).await?, json!(["block", "block"]));
    Ok(())
}

async fn curtain_picker(h: &H) -> Res {
    h.add_user_script(&curtain::picker());
    h.load("curtain.html").await?;
    h.run_json(&curtain::start()).await?;
    ensure_eq!(
        h.js("document.documentElement.style.cursor").await?,
        json!("crosshair")
    );
    let press = |sel: &str| {
        format!(
            "(function(){{ var el = document.querySelector({sel:?}); var r = el.getBoundingClientRect(); var x = r.left + Math.min(5, r.width/2), y = r.top + Math.min(5, r.height/2); \
             document.dispatchEvent(new PointerEvent('pointermove', {{clientX: x, clientY: y, bubbles: true}})); \
             var e = new PointerEvent('pointerdown', {{clientX: x, clientY: y, bubbles: true, cancelable: true}}); el.dispatchEvent(e); return e.defaultPrevented; }})()"
        )
    };
    ensure_eq!(h.js(&press(".newsletter-box")).await?, json!(true));
    ensure_eq!(h.js(&press("[data-testid=promo] span")).await?, json!(true));
    ensure_eq!(h.js(&press("section p:nth-of-type(2)")).await?, json!(true));
    ensure_eq!(h.js(&press("header h1")).await?, json!(true));
    let picks: Vec<messages::VeilMessage> = h
        .messages(messages::VEIL)
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let selectors: Vec<String> = picks
        .iter()
        .filter_map(|m| match m {
            messages::VeilMessage::Picked { selector, .. } => Some(selector.clone()),
            _ => None,
        })
        .collect();
    ensure_eq!(
        selectors,
        vec![
            "div.newsletter-box",
            "body > div:nth-of-type(2) > span",
            "body > section > div > p:nth-of-type(2)",
            "body > header > h1",
        ]
    );
    let messages::VeilMessage::Picked { label, note, .. } = &picks[0] else {
        return Err("first message is not a pick".into());
    };
    ensure_eq!(label.as_deref(), Some("Subscribe to our newsletter"));
    ensure!(
        note.as_deref().is_some_and(|n| n.starts_with("300×200 · ")),
        "note {note:?}"
    );
    // The page itself never saw the press.
    h.clear_messages();
    h.run_json(&curtain::stop()).await?;
    let off = h
        .message(messages::VEIL, |m| m.get("off").is_some())
        .await?;
    ensure_eq!(
        serde_json::from_value::<messages::VeilMessage>(off).map_err(|e| e.to_string())?,
        messages::VeilMessage::Off { off: true }
    );
    ensure_eq!(
        h.js("document.documentElement.style.cursor").await?,
        json!("")
    );
    Ok(())
}

// ------------------------------------------------------------------ reader

async fn reader_mode(h: &H) -> Res {
    h.load("article.html").await?;
    ensure_eq!(h.run(&tab::reader()).await?, ReaderOutcome::Read);
    let host = h.base.trim_start_matches("http://").to_string();
    let page = h
        .js("({title: document.querySelector('#office-reader h1').textContent, from: document.querySelector('#office-reader .office-from').textContent, \
              paragraphs: document.querySelectorAll('#office-reader p').length, clutter: document.querySelectorAll('#office-reader nav, #office-reader aside, #office-reader button, #office-reader iframe').length, \
              images: document.querySelectorAll('#office-reader img').length, links: document.querySelectorAll('#office-reader a').length})")
        .await?;
    ensure_eq!(
        page,
        json!({"title": "The quiet browser", "from": host, "paragraphs": 4, "clutter": 0, "images": 0, "links": 0})
    );
    h.load("misc.html").await?;
    ensure_eq!(h.run(&tab::reader()).await?, ReaderOutcome::None);
    Ok(())
}

// ------------------------------------------------------------------ tab plumbing

async fn tab_plumbing(h: &H) -> Res {
    h.load("misc.html").await?;
    let icons = h.run(&tab::favicons()).await?;
    ensure_eq!(icons.len(), 2);
    ensure_eq!(icons[0].href, h.url("favicon-32.png"));
    ensure_eq!(
        (
            icons[0].rel.as_str(),
            icons[0].sizes.as_str(),
            icons[0].kind.as_str()
        ),
        ("icon", "32x32", "image/png")
    );
    ensure_eq!(
        (icons[1].rel.as_str(), icons[1].sizes.as_str()),
        ("apple-touch-icon", "180x180")
    );
    ensure_eq!(h.run(&tab::ready_state()).await?, "complete");
    ensure_eq!(h.run(&tab::origin()).await?, h.base);
    // Selection cleared when find closes.
    h.js("(function(){ var r = document.createRange(); r.selectNodeContents(document.getElementById('sel')); getSelection().addRange(r); return true; })()").await?;
    ensure_eq!(
        h.js("getSelection().toString()").await?,
        json!("Some selectable words")
    );
    ensure_eq!(h.run(&tab::clear_selection()).await?, true);
    ensure_eq!(h.js("getSelection().rangeCount").await?, json!(0));
    // localStorage import.
    let mut items = BTreeMap::new();
    items.insert("copper.a".to_string(), "1".to_string());
    items.insert("copper.b".to_string(), "two".to_string());
    ensure_eq!(
        h.run(&tab::storage_put(&items)).await?,
        Vec::<String>::new()
    );
    ensure_eq!(
        h.js("[localStorage.getItem('copper.a'), localStorage.getItem('copper.b')]")
            .await?,
        json!(["1", "two"])
    );
    // Smart zoom: in on a paragraph's column, back out when zoomed.
    let rect = h.js("(function(){ var r = document.getElementById('para').getBoundingClientRect(); return {x: r.left + 20, y: r.top + 5}; })()").await?;
    let (x, y) = (rect["x"].as_f64().unwrap(), rect["y"].as_f64().unwrap());
    let zoom = h
        .run(&tab::smart_zoom(x, y, 1.0, 1024.0))
        .await?
        .ok_or("nothing under the point")?;
    let zoom = tab::parse_zoom(&zoom).map_err(|e| e.to_string())?;
    ensure!(zoom.scale > 1.15 && zoom.scale <= 3.0, "{zoom:?}");
    let out = tab::parse_zoom(
        &h.run(&tab::smart_zoom(x, y, 2.0, 1024.0))
            .await?
            .ok_or("none")?,
    )
    .map_err(|e| e.to_string())?;
    ensure_eq!(out.scale, 1.0);
    ensure_eq!(h.run(&tab::scroll_to(0.0, 400.0)).await?, true);
    ensure_eq!(h.js("window.scrollY").await?, json!(400));
    Ok(())
}

async fn tab_relays(h: &H) -> Res {
    for us in [
        tab::scroll_relay(),
        tab::swipe_calm(),
        tab::swipe_watch(),
        tab::image_relay(),
    ] {
        h.add_user_script(&us);
    }
    h.load("misc.html").await?;
    h.message(messages::SCROLL, |m| m["y"] == 0).await?;
    h.run(&tab::scroll_to(0.0, 500.0)).await?;
    let at = h.message(messages::SCROLL, |m| m["y"] == 500).await?;
    let at: messages::ScrollMessage = serde_json::from_value(at).map_err(|e| e.to_string())?;
    ensure!(
        matches!(at, messages::ScrollMessage::Position { max, .. } if max > 500.0),
        "{at:?}"
    );
    ensure_eq!(
        h.js("getComputedStyle(document.documentElement).overscrollBehaviorY")
            .await?,
        json!("none")
    );
    // A sideways wheel over the strip belongs to the page; over the body it doesn't.
    h.run(&tab::scroll_to(0.0, 0.0)).await?;
    h.js("document.getElementById('wide').dispatchEvent(new WheelEvent('wheel', {deltaX: 40, deltaY: 0, bubbles: true})); true").await?;
    h.message(messages::SCROLL, |m| m["side"] == "taken")
        .await?;
    h.js("document.getElementById('para').dispatchEvent(new WheelEvent('wheel', {deltaX: 40, deltaY: 0, bubbles: true})); true").await?;
    h.message(messages::SCROLL, |m| m["side"] == "free").await?;
    // Right-click on a picture.
    h.js("document.getElementById('pic').dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, cancelable: true})); true").await?;
    let image = h.message(messages::IMAGES, |_| true).await?;
    let image: messages::ImageMessage = serde_json::from_value(image).map_err(|e| e.to_string())?;
    ensure!(image.src.starts_with("data:image/svg+xml"), "{image:?}");
    Ok(())
}

// ------------------------------------------------------------------ canvas

async fn canvas_bridge(h: &H) -> Res {
    h.add_user_script(&canvas::typing());
    h.load("canvas-fixture.html").await?;
    h.message(messages::CANVAS, |m| m["type"] == "ready")
        .await?;
    ensure_eq!(h.run(&canvas::is_live()).await?, true);
    let options = json!({"name": "Board", "kind": "personal", "me": {"id": "me", "name": "Me", "color": "#c8743c"}});
    ensure_eq!(
        h.run(&canvas::ready(&options, &[], "dark", "Board", "ticket-1"))
            .await?,
        true
    );
    ensure_eq!(
        h.js("[document.title, window.__copperCanvasHost, document.documentElement.classList.contains('dark')]").await?,
        json!(["Board", "ticket-1", true])
    );
    h.run_json(&canvas::theme("light")).await?;
    ensure_eq!(
        h.js("document.documentElement.classList.contains('dark')")
            .await?,
        json!(false)
    );
    let applied = h
        .run(&canvas::apply(r#"{"ops":[{"op":"add","shape":{"type":"sticky","id":"n1","text":"hi","x":10,"y":10}}],"as":"Tester"}"#))
        .await?;
    let applied: Value = serde_json::from_str(&applied).map_err(|e| e.to_string())?;
    ensure_eq!(applied["applied"], json!(1));
    let read: Value =
        serde_json::from_str(&h.run(&canvas::read(false)).await?).map_err(|e| e.to_string())?;
    ensure_eq!(read["shapes"][0]["text"], json!("hi"));
    ensure_eq!(read["canvas"]["name"], json!("Board"));
    h.clear_messages();
    h.run_json(&canvas::select(&["n1".into()])).await?;
    h.message(messages::CANVAS, |m| {
        m["type"] == "selection" && m["ids"] == json!(["n1"])
    })
    .await?;
    h.run_json(&canvas::zoom_to(&["n1".into()])).await?;
    let state = h.run(&canvas::export_state()).await?;
    ensure!(
        state.as_str().is_some_and(|s| !s.is_empty()),
        "state {state}"
    );
    h.run_json(&canvas::apply_update(state.as_str().unwrap()))
        .await?;
    for call in [
        canvas::ws_open(),
        canvas::ws_closed(),
        canvas::ws_message("AAAA"),
        canvas::set_share(&json!({"shared": false})),
        canvas::set_status(&json!({"online": false})),
        canvas::set_chat(&json!({"messages": []})),
        canvas::show_chat(true),
        canvas::reveal_chat("m1"),
        canvas::frame_host(&json!([])),
    ] {
        h.run_json(&call).await?;
    }
    ensure_eq!(h.run(&canvas::rename("Renamed")).await?, false); // the fixture has no rename()
    ensure_eq!(h.js("document.title").await?, json!("Renamed"));
    ensure_eq!(
        h.run(&canvas::frame_event(&json!({"kind": "nav"}))).await?,
        false
    );
    let sent = h
        .run(&canvas::chat_send(r#"{"text":"hi","mentions":[]}"#))
        .await?;
    ensure_eq!(
        sent,
        json!("{\"ok\":false,\"error\":\"this canvas page has no chat\"}")
    );
    // The fixture has no presence(): the call fails loudly instead of inventing one.
    ensure!(
        h.run(&canvas::presence()).await.is_err(),
        "presence() on the fixture should throw"
    );
    // The lean board's typing relay.
    h.clear_messages();
    h.js("var i = document.createElement('input'); document.body.appendChild(i); i.focus(); true")
        .await?;
    h.message(messages::FORMS, |m| {
        m["kind"] == "focus" && m["typing"] == true
    })
    .await?;
    Ok(())
}

async fn canvas_frames(h: &H) -> Res {
    h.add_user_script(&canvas::frame_reporter());
    h.load("frames.html").await?;
    let nav = h.message(messages::FRAME, |m| m["kind"] == "nav").await?;
    let nav: messages::FrameMessage = serde_json::from_value(nav).map_err(|e| e.to_string())?;
    ensure_eq!(nav.name, "copper-frame:one");
    let frame_url = h.url("frame.html");
    ensure_eq!(nav.url.as_deref(), Some(frame_url.as_str()));
    ensure_eq!(nav.title.as_deref(), Some("Framed page"));
    ensure_eq!(nav.icon.as_deref(), Some(""));
    sleep(300).await;
    ensure!(
        h.messages(messages::FRAME)
            .iter()
            .all(|m| m["name"] == "copper-frame:one"),
        "a frame outside the board reported: {:?}",
        h.messages(messages::FRAME)
    );
    // The page world never sees the reporter.
    ensure_eq!(
        h.js("typeof window.webkit.messageHandlers.copperFrame")
            .await?,
        json!("undefined")
    );
    Ok(())
}
