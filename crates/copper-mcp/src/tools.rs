//! The tools an agent gets (Fork/MCP/Tools.swift), named and shaped like
//! Playwright MCP's. Under them is the tab the user has open, through
//! [`Host`] and [`Page`]; the page side is `copper_pagejs`.
//!
//! Elements are addressed by `ref` (from a snapshot) or a CSS `selector`.
//! Clicks and keys go in as real input ([`Page::input`]) when the view can
//! take it; otherwise the page script dispatches DOM events of the same shape.

use std::pin::pin;
use std::time::{Duration, Instant};

use copper_engine::{Bitmap, CaptureRequest, InputEvent, MouseButton, ViewRect};
use copper_pagejs::page::{self as pj, Locator, Rect};
use copper_pagejs::render::{pretty_json, render};
use copper_pagejs::{Call, trail};
use futures_util::future::{Either, select};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::agent_tabs::short;
use crate::catalogue;
use crate::drive::{looks_secret, string_list};
use crate::host::{Content, Host, Page, TabId, TabInfo};
use crate::input;
use crate::screenshot::{self, Answer, Geometry, Saved, ShotArgs, Subject};
use crate::shot;
use crate::{CallCtx, Failure, Mcp};

/// How long saving a screenshot may take before the call gives up on it.
const SAVE_LIMIT: Duration = Duration::from_secs(15);

/// A screenshot for the worker thread to finish.
struct Develop {
    bitmap: Bitmap,
    /// The picture's part of `bitmap`, in its pixels.
    src: (f64, f64, f64, f64),
    /// The picture's size, its CSS size and its scale.
    size: (u32, u32),
    css: (f64, f64),
    scale: f64,
    jpeg: bool,
    /// Where to save it, if anywhere.
    path: Option<String>,
}

/// What the worker made: the answer's image, the save, and the answer
/// image's size and scale when it is not the picture.
struct Developed {
    image: Vec<u8>,
    saved: Option<Saved>,
    view: Option<((u32, u32), f64)>,
}

/// Run a curated call in the page and decode its result.
pub(crate) async fn js<P: Page, T: DeserializeOwned>(
    page: &P,
    call: &Call<T>,
) -> Result<T, Failure> {
    let value = js_value(page, call).await?;
    call.decode_value(value)
        .map_err(|e| Failure(format!("JavaScript: {e}")))
}

/// Run a curated call and keep the raw JSON (for `Page.render`).
pub(crate) async fn js_value<P: Page, T>(page: &P, call: &Call<T>) -> Result<Value, Failure> {
    page.call_async(&call.function_body(), call.world().name())
        .await
        .map_err(Failure)
}

/// Fire-and-forget: the cosmetic trail never fails a tool.
pub(crate) async fn cosmetic<P: Page, T>(page: &P, call: &Call<T>) {
    let _ = page
        .call_async(&call.function_body(), call.world().name())
        .await;
}

/// Mask (or, `on: false`, unmask) what shows `tab`'s secrets as plain text
/// in its page, for a picture ([`crate::secrets::MASK_SCRIPT`]). Answers how
/// many things were masked; 0 when the tab holds no secret.
pub(crate) async fn mask_secrets<P: Page>(page: &P, tab: &TabId, on: bool) -> u64 {
    let held = crate::secrets::held(tab);
    if held.is_empty() {
        return 0;
    }
    let function = format!(
        "() => ({})({{ masking: {on}, secrets: {} }})",
        crate::secrets::MASK_SCRIPT,
        serde_json::to_string(&held).unwrap_or_else(|_| "[]".into())
    );
    match js_value(page, &pj::evaluate(&function, None)).await {
        Ok(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .unwrap_or(0),
        Err(_) => 0,
    }
}

pub(crate) fn arg_str<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(crate) fn arg_bool(args: &Map<String, Value>, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

pub(crate) fn arg_f64(args: &Map<String, Value>, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn locator(args: &Map<String, Value>) -> Result<Locator, Failure> {
    Locator::from_args(arg_str(args, "ref"), arg_str(args, "selector"))
        .ok_or_else(|| Failure::new("ref (from browser_snapshot) or selector required"))
}

fn button(raw: Option<&str>) -> MouseButton {
    match raw {
        Some("right") => MouseButton::Right,
        Some("middle") => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

impl<H: Host> Mcp<H> {
    /// The tab's page; every tab the host lists has one after `prepare`.
    pub(crate) fn page_of(&self, id: &TabId) -> Result<H::Page, Failure> {
        self.host
            .page(id)
            .ok_or_else(|| Failure::new("the tab has no page yet"))
    }

    /// Scroll the target into view and answer its box in view points.
    pub(crate) async fn prepare_target(
        &self,
        page: &H::Page,
        target: &Locator,
    ) -> Result<Rect, Failure> {
        let located = js(page, &pj::rect(target)).await?;
        let rect = located.into_result(target).map_err(Failure)?;
        Ok(rect.scaled(page.zoom()))
    }

    /// The trail's outline over the element about to be acted on (CSS px).
    async fn trail_target(
        &self,
        page: &H::Page,
        rect: &Rect,
        label: Option<&str>,
        operation: &str,
    ) {
        let z = page.zoom().max(0.01);
        cosmetic(
            page,
            &trail::target(
                rect.x / z,
                rect.y / z,
                rect.width / z,
                rect.height / z,
                label.unwrap_or(""),
                operation,
            ),
        )
        .await;
    }

    /// The pointer travels to (x, y) CSS px; capped at 400 ms.
    pub(crate) async fn glide(&self, page: &H::Page, x: f64, y: f64) {
        let call = trail::glide(x, y);
        let travel = pin!(cosmetic(page, &call));
        let cap = pin!(self.host.sleep(Duration::from_millis(400)));
        match select(travel, cap).await {
            Either::Left(_) | Either::Right(_) => {}
        }
    }

    /// One tool call, with the `tab` argument pinned (`Tools.call`).
    pub async fn call_tool(
        &self,
        name: &str,
        args: &Map<String, Value>,
        ctx: &mut CallCtx,
    ) -> Result<Vec<Content>, Failure> {
        if name != "browser_tabs"
            && let Some(named) = self.resolve(args.get("tab"), ctx)?
        {
            ctx.pinned = Some(named.id);
        }
        self.dispatch(name, args, ctx).await
    }

    async fn dispatch(
        &self,
        name: &str,
        args: &Map<String, Value>,
        ctx: &mut CallCtx,
    ) -> Result<Vec<Content>, Failure> {
        let unstaged = [
            "browser_tabs",
            "browser_close",
            "browser_resize",
            "browser_groups",
            "browser_crashes",
            catalogue::CLOUD_HISTORY,
        ];
        if !unstaged.contains(&name)
            && !catalogue::is_canvas(name)
            && !(name == "jev_run" && arg_bool(args, "newTab") == Some(true))
        {
            self.ready(ctx).await;
        }
        match name {
            "browser_tabs" => return self.tabs_tool(args, ctx).await,
            "browser_close" => {
                let tab = self.current(ctx)?;
                let said = self.close_tab(&tab.id);
                return Ok(vec![Content::text(format!(
                    "{said} {}",
                    self.tab_list(ctx)
                ))]);
            }
            "browser_resize" => {
                let (Some(w), Some(h)) = (arg_f64(args, "width"), arg_f64(args, "height")) else {
                    return Err("width and height".into());
                };
                self.host.resize_window(w, h).map_err(Failure)?;
                return Ok(vec![Content::text(format!(
                    "Window is {}×{}",
                    w as i64, h as i64
                ))]);
            }
            "browser_groups" => return Ok(vec![Content::text(self.groups_tool(args, ctx))]),
            "jev_run" | "jev_step" | "jev_observe" | "jev_extract" => {
                return self.jev_tool(name, args, ctx).await;
            }
            n if catalogue::is_canvas(n) => {
                return self.host.canvas(n, args).await.map_err(Failure);
            }
            "browser_crashes" => {
                return Ok(vec![Content::Text(
                    self.host.crashes(args).map_err(Failure)?,
                )]);
            }
            catalogue::CLOUD_HISTORY => {
                return self.host.cloud_history_delete(args).await.map_err(Failure);
            }
            _ => {}
        }

        let tab = self.current(ctx)?;
        let id = tab.id.clone();
        let page = self.page_of(&id)?;
        let real = page.can_post_input();

        match name {
            "browser_sign_in" | "browser_autofill" => {
                let result = if name == "browser_sign_in" {
                    self.host.sign_in(&id, args).await
                } else {
                    self.host.autofill(&id, args).await
                }
                .map_err(Failure)?;
                Ok(vec![Content::Text(sorted_compact(&result))])
            }
            "browser_perf_probe" => {
                let (seconds, top, format) = copper_pagejs::probe::clamp(
                    arg_f64(args, "seconds"),
                    arg_f64(args, "top"),
                    arg_str(args, "format") == Some("json"),
                );
                let call = copper_pagejs::probe::perf(seconds, top, format);
                let run = pin!(js_value(&page, &call));
                let cap = pin!(self.host.sleep(Duration::from_secs_f64(seconds + 5.0)));
                let value = match select(run, cap).await {
                    Either::Left((value, _)) => value?,
                    Either::Right(_) => return Err("performance probe timed out".into()),
                };
                if format == "json" {
                    return Ok(vec![Content::Text(render(Some(&value)))]);
                }
                let Some(text) = value.as_str() else {
                    return Err("performance probe returned no text".into());
                };
                Ok(vec![Content::Text(format!(
                    "{}{text}",
                    self.host.processes_line(&id)
                ))])
            }
            "browser_navigate" => {
                let raw = arg_str(args, "url").ok_or("url required")?;
                let url = copper_core::url_from_typed(raw)
                    .ok_or_else(|| Failure(format!("not an address: {raw}")))?;
                self.host.navigate(&id, url.as_str());
                self.settle(&id, Duration::from_secs(15)).await;
                Ok(vec![Content::Text(self.page_line(&id))])
            }
            "browser_navigate_back" => {
                self.host.go_back(&id);
                self.settle(&id, Duration::from_secs(15)).await;
                Ok(vec![Content::Text(self.page_line(&id))])
            }
            "browser_navigate_forward" => {
                self.host.go_forward(&id);
                self.settle(&id, Duration::from_secs(15)).await;
                Ok(vec![Content::Text(self.page_line(&id))])
            }
            "browser_snapshot" => {
                let interactive = arg_bool(args, "interactive").unwrap_or(false);
                let limit = arg_f64(args, "limit").map_or(600, |l| l.max(0.0) as u32);
                let selector = arg_str(args, "selector");
                let text = js(&page, &pj::snapshot(selector, interactive, limit))
                    .await
                    .map_err(|_| Failure::new("snapshot gave nothing"))?;
                Ok(vec![Content::Text(format!(
                    "{}\n\n{text}",
                    self.page_line(&id)
                ))])
            }
            "browser_click" => {
                let target = locator(args)?;
                let rect = self.prepare_target(&page, &target).await?;
                let double = arg_bool(args, "doubleClick").unwrap_or(false);
                let button_name = arg_str(args, "button").unwrap_or("left");
                let mods = input::modifiers(&string_list(args.get("modifiers")));
                let element = arg_str(args, "element");
                self.trail_target(&page, &rect, element, "CLICK").await;
                let z = page.zoom().max(0.01);
                let (cx, cy) = rect.center();
                self.glide(&page, cx / z, cy / z).await;
                cosmetic(&page, &trail::click(cx / z, cy / z)).await;
                if real {
                    page.input(InputEvent::Click {
                        x: cx / z,
                        y: cy / z,
                        button: button(Some(button_name)),
                        count: if double { 2 } else { 1 },
                        modifiers: mods,
                    })
                    .await
                    .map_err(Failure)?;
                } else {
                    js(&page, &pj::click(&target, double, button_name)).await?;
                }
                self.settle(&id, Duration::from_secs(3)).await;
                Ok(vec![Content::Text(format!(
                    "Clicked {}. {}",
                    element.map_or_else(|| target.script(), str::to_string),
                    self.page_line(&id)
                ))])
            }
            "browser_hover" => {
                let target = locator(args)?;
                let rect = self.prepare_target(&page, &target).await?;
                let element = arg_str(args, "element");
                self.trail_target(&page, &rect, element, "HOVER").await;
                let z = page.zoom().max(0.01);
                let (cx, cy) = rect.center();
                self.glide(&page, cx / z, cy / z).await;
                if real {
                    page.input(InputEvent::Move {
                        x: cx / z,
                        y: cy / z,
                        modifiers: copper_engine::Modifiers::NONE,
                    })
                    .await
                    .map_err(Failure)?;
                } else {
                    js(&page, &pj::hover(&target)).await?;
                }
                Ok(vec![Content::Text(format!(
                    "Hovering {}",
                    element.map_or_else(|| target.script(), str::to_string)
                ))])
            }
            "browser_type" => {
                let target = locator(args)?;
                let text = arg_str(args, "text").ok_or("text required")?;
                let submit = arg_bool(args, "submit").unwrap_or(false);
                let slowly = arg_bool(args, "slowly").unwrap_or(false);
                let element = arg_str(args, "element");
                let rect = self.prepare_target(&page, &target).await?;
                self.trail_target(&page, &rect, element, "TYPE_TEXT").await;
                let z = page.zoom().max(0.01);
                let (cx, cy) = rect.center();
                self.glide(&page, cx / z, cy / z).await;
                cosmetic(&page, &trail::click(cx / z, cy / z)).await;
                if real {
                    page.input(InputEvent::click(cx / z, cy / z))
                        .await
                        .map_err(Failure)?;
                    self.host.sleep(Duration::from_millis(60)).await;
                    js(&page, &pj::focus(&target)).await?;
                    if slowly {
                        page.input(InputEvent::Type(text.to_string()))
                            .await
                            .map_err(Failure)?;
                    } else {
                        js(&page, &pj::set_value(&target, text)).await?;
                    }
                } else {
                    js(&page, &pj::set_value(&target, text)).await?;
                }
                let label = format!("{} {}", element.unwrap_or(""), target.script());
                let shown = if looks_secret(&label) {
                    "•••"
                } else {
                    text
                };
                cosmetic(&page, &trail::typed(cx / z, cy / z, shown)).await;
                if submit {
                    if real {
                        page.input(input::key("Enter", copper_engine::Modifiers::NONE))
                            .await
                            .map_err(Failure)?;
                    } else {
                        js(&page, &pj::submit(&target)).await?;
                    }
                    self.settle(&id, Duration::from_secs(3)).await;
                }
                Ok(vec![Content::Text(format!(
                    "Typed into {}{}. {}",
                    element.map_or_else(|| target.script(), str::to_string),
                    if submit { " and submitted" } else { "" },
                    self.page_line(&id)
                ))])
            }
            "browser_fill_form" => {
                let fields = args
                    .get("fields")
                    .and_then(Value::as_array)
                    .ok_or("fields required")?;
                let mut lines = Vec::new();
                for field in fields {
                    let Some(field) = field.as_object() else {
                        continue;
                    };
                    let Some(reference) = arg_str(field, "ref") else {
                        continue;
                    };
                    let value = arg_str(field, "value").unwrap_or("");
                    let kind = arg_str(field, "type").unwrap_or("textbox");
                    let result = js_value(&page, &pj::fill(reference, kind, value)).await?;
                    lines.push(format!(
                        "{}: {}",
                        arg_str(field, "name").unwrap_or(reference),
                        render(Some(&result))
                    ));
                }
                Ok(vec![Content::Text(lines.join("\n"))])
            }
            "browser_press_key" => {
                let key = arg_str(args, "key").ok_or("key required")?;
                let mods = input::modifiers(&string_list(args.get("modifiers")));
                if real {
                    page.input(input::key(key, mods)).await.map_err(Failure)?;
                } else {
                    js(&page, &pj::key(key)).await?;
                }
                self.settle(&id, Duration::from_secs(2)).await;
                Ok(vec![Content::Text(format!(
                    "Pressed {key}. {}",
                    self.page_line(&id)
                ))])
            }
            "browser_select_option" => {
                let target = locator(args)?;
                let values = string_list(args.get("values"));
                let result = js_value(&page, &pj::select(&target, &values)).await?;
                Ok(vec![Content::Text(render(Some(&result)))])
            }
            "browser_drag" => {
                let (Some(a), Some(b)) = (arg_str(args, "startRef"), arg_str(args, "endRef"))
                else {
                    return Err("startRef and endRef".into());
                };
                let from = self.prepare_target(&page, &Locator::Ref(a.into())).await?;
                let to = self.prepare_target(&page, &Locator::Ref(b.into())).await?;
                self.trail_target(&page, &from, arg_str(args, "startElement"), "DRAG")
                    .await;
                let z = page.zoom().max(0.01);
                let (fx, fy) = from.center();
                let (tx, ty) = to.center();
                cosmetic(&page, &trail::click(fx / z, fy / z)).await;
                self.glide(&page, tx / z, ty / z).await;
                cosmetic(&page, &trail::click(tx / z, ty / z)).await;
                if real {
                    page.input(InputEvent::Drag {
                        from_x: fx / z,
                        from_y: fy / z,
                        to_x: tx / z,
                        to_y: ty / z,
                        modifiers: copper_engine::Modifiers::NONE,
                    })
                    .await
                    .map_err(Failure)?;
                } else {
                    js(&page, &pj::drag(a, b)).await?;
                }
                Ok(vec![Content::Text(format!("Dragged {a} to {b}"))])
            }
            "browser_take_screenshot" => self.screenshot(&page, &id, args).await,
            "browser_evaluate" => {
                let function = arg_str(args, "function").ok_or("function required")?;
                let result = js_value(&page, &pj::evaluate(function, arg_str(args, "ref"))).await?;
                Ok(vec![Content::Text(render(Some(&result)))])
            }
            "browser_wait_for" => {
                if let Some(seconds) = arg_f64(args, "time") {
                    self.host
                        .sleep(Duration::from_secs_f64(seconds.clamp(0.0, 30.0)))
                        .await;
                }
                let deadline = Instant::now() + Duration::from_secs(15);
                if let Some(text) = arg_str(args, "text") {
                    while Instant::now() < deadline {
                        if js(&page, &pj::has_text(text)).await? == Some(true) {
                            break;
                        }
                        self.host.sleep(Duration::from_millis(150)).await;
                    }
                }
                if let Some(text) = arg_str(args, "textGone") {
                    while Instant::now() < deadline {
                        if js(&page, &pj::has_text(text)).await? == Some(false) {
                            break;
                        }
                        self.host.sleep(Duration::from_millis(150)).await;
                    }
                }
                Ok(vec![Content::Text(self.page_line(&id))])
            }
            "browser_scroll" => {
                let dy = arg_f64(args, "deltaY");
                let dx = arg_f64(args, "deltaX").unwrap_or(0.0);
                if let (Some(reference), None) = (arg_str(args, "ref"), dy) {
                    self.prepare_target(&page, &Locator::Ref(reference.into()))
                        .await?;
                    return Ok(vec![Content::Text(format!(
                        "Scrolled {reference} into view"
                    ))]);
                }
                let y = dy.unwrap_or_else(|| page.view_size().1 * 0.85);
                cosmetic(&page, &trail::scroll(y)).await;
                js_value(&page, &pj::scroll(arg_str(args, "ref"), dx, y)).await?;
                self.host.sleep(Duration::from_millis(120)).await;
                Ok(vec![Content::Text(format!(
                    "Scrolled by {},{}. {}",
                    dx as i64,
                    y as i64,
                    self.page_line(&id)
                ))])
            }
            "browser_get_text" => {
                let target = locator(args).ok();
                let result = js_value(&page, &pj::text(target.as_ref())).await?;
                Ok(vec![Content::Text(render(Some(&result)))])
            }
            "browser_console_messages" => {
                let result = js_value(&page, &pj::console()).await?;
                Ok(vec![Content::Text(render(Some(&result)))])
            }
            "browser_find" => {
                let text = arg_str(args, "text").ok_or("text required")?;
                let result = js_value(&page, &pj::find_text(text)).await?;
                Ok(vec![Content::Text(render(Some(&result)))])
            }
            _ => Err(Failure(format!("Unknown tool {name}"))),
        }
    }

    /// `browser_take_screenshot`: an element, a region, the viewport or
    /// the whole page, at 1x to 4x, maybe saved. `screenshot.rs` has the
    /// rules (all in CSS pixels); the engine paints (`PageView::capture`,
    /// in view pixels: CSS times the zoom); a worker thread crops, scales,
    /// encodes and saves.
    async fn screenshot(
        &self,
        page: &H::Page,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> Result<Vec<Content>, Failure> {
        let ask = ShotArgs::parse(args).map_err(Failure)?;
        let zoom = page.zoom().max(0.01);
        let target = match &ask.subject {
            Subject::Ref(r) => Some(Locator::Ref(r.clone())),
            Subject::Selector(css) => Some(Locator::Css(css.clone())),
            Subject::Page | Subject::Region(_) => None,
        };
        let element = match &target {
            Some(target) => {
                let r = self.prepare_target(page, target).await?;
                Some(ViewRect::new(
                    r.x / zoom,
                    r.y / zoom,
                    r.width / zoom,
                    r.height / zoom,
                ))
            }
            None => None,
        };
        let (vw, vh) = page.view_size();
        let viewport = (vw / zoom, vh / zoom);
        // The page's size and scroll: for an element and anything fullPage.
        let metrics = if element.is_some() || ask.full_page {
            Some(
                js(page, &pj::metrics())
                    .await
                    .map_err(|_| Failure::new("couldn't measure the page"))?,
            )
        } else {
            None
        };
        let geometry = Geometry {
            viewport,
            document: metrics.map_or(viewport, |m| (m.width, m.height)),
            scroll: metrics.map_or((0.0, 0.0), |m| (m.x, m.y)),
        };
        let label = target.as_ref().map(Locator::script).unwrap_or_default();
        let plan = screenshot::plan(&ask, &geometry, element, &label).map_err(Failure)?;
        let first = screenshot::fit(plan.rect.width, plan.rect.height, ask.scale);
        // Secret redaction: a picture can't be scrubbed after it is taken, so
        // what shows a secret Copper filled is masked for the capture.
        let masked = mask_secrets(page, id, true).await;
        let captured = page
            .capture(CaptureRequest {
                area: plan.area(zoom),
                scale: first.scale / zoom,
            })
            .await;
        if masked > 0 {
            mask_secrets(page, id, false).await;
        }
        let captured = captured.map_err(Failure)?;
        // Picture pixels per CSS pixel the engine painted at.
        let mut painted = captured.scale * zoom;
        if !(painted.is_finite() && painted > 0.0) || captured.bitmap.width == 0 {
            return Err("the page gave an empty picture".into());
        }
        // A scale read off a picture's whole pixels is off by up to one
        // pixel across it: within that, it is the scale asked for (else a
        // tall picture comes out a few pixels short).
        let across = f64::from(captured.bitmap.width) / painted;
        if (painted - first.scale).abs() * across <= 1.0 {
            painted = first.scale;
        }
        // What was painted, in CSS pixels (and the pixel rounding may have
        // taken off its far edges); the picture is what of the plan it
        // holds (all of it, unless the page changed meanwhile).
        let extent = ViewRect::new(
            captured.origin.0 / zoom,
            captured.origin.1 / zoom,
            (f64::from(captured.bitmap.width) + 1.0) / painted,
            (f64::from(captured.bitmap.height) + 1.0) / painted,
        );
        let rect = screenshot::intersect(plan.rect, extent)
            .ok_or_else(|| Failure::new("the page gave an empty picture"))?;
        let fit = screenshot::fit(rect.width, rect.height, first.scale);
        let lowered = (fit.scale < ask.scale).then(|| {
            let limit = first.lowered.or(fit.lowered);
            (ask.scale, limit.unwrap_or(screenshot::Limit::Side))
        });
        let upscaled = (painted < fit.scale * 0.995).then_some(painted);
        let src = (
            (rect.x - extent.x) * painted,
            (rect.y - extent.y) * painted,
            rect.width * painted,
            rect.height * painted,
        );
        let developed = self
            .develop(Develop {
                bitmap: captured.bitmap,
                src,
                size: fit.size,
                css: (rect.width, rect.height),
                scale: fit.scale,
                jpeg: ask.jpeg,
                path: ask.filename.clone(),
            })
            .await?;
        let answer = Answer {
            size: fit.size,
            format: ask.format(),
            scale: fit.scale,
            subject: ask.subject.words(ask.full_page),
            saved: developed.saved,
            lowered,
            upscaled,
            view: developed.view,
        };
        Ok(vec![
            Content::Image {
                data: developed.image,
                mime: ask.mime().into(),
            },
            Content::Text(answer.text(&self.page_line(id))),
        ])
    }

    /// Crop, scale, encode and save the picture on a worker thread, and
    /// make the answer's own image ([`screenshot::view_fit`]: scale 1 when
    /// the HD picture went to a file, at most 8000 px a side). The save is
    /// bounded like the Mac's (`OffMainWrite`): a folder that hangs (a
    /// network or removable volume) fails the call after [`SAVE_LIMIT`]
    /// instead of holding up every other call.
    async fn develop(&self, job: Develop) -> Result<Developed, Failure> {
        let (encoded_tx, encoded_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<Developed, String>>();
        let path = job.path.clone();
        std::thread::Builder::new()
            .name("copper-screenshot".into())
            .spawn(move || {
                let encode = |size: (u32, u32)| {
                    let picture = shot::resample(&job.bitmap, job.src, size);
                    if job.jpeg {
                        shot::encode_jpeg(&picture)
                    } else {
                        shot::encode_png(&picture)
                    }
                };
                let full = match encode(job.size) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        let _ = encoded_tx.send(Err(e));
                        return;
                    }
                };
                let _ = encoded_tx.send(Ok(()));
                let saved = job
                    .path
                    .as_deref()
                    .map(|to| screenshot::save(&full, to, job.jpeg));
                let to_file = matches!(saved, Some(Saved::To(_)));
                let (scale, size) = screenshot::view_fit(job.css.0, job.css.1, job.scale, to_file);
                let done = if size == job.size {
                    Ok(Developed {
                        image: full,
                        saved,
                        view: None,
                    })
                } else {
                    encode(size).map(|image| Developed {
                        image,
                        saved,
                        view: Some((size, scale)),
                    })
                };
                let _ = done_tx.send(done);
            })
            .map_err(|e| Failure(format!("couldn't make the picture: {e}")))?;
        encoded_rx
            .await
            .map_err(|_| Failure::new("couldn't make the picture"))?
            .map_err(|e| Failure(format!("couldn't encode the picture: {e}")))?;
        let done = match path {
            None => done_rx.await,
            Some(path) => {
                let wait = pin!(done_rx);
                let cap = pin!(self.host.sleep(SAVE_LIMIT));
                match select(wait, cap).await {
                    Either::Left((done, _)) => done,
                    Either::Right(_) => {
                        return Err(Failure(format!(
                            "couldn't write {path} within {} s — the folder isn't answering (a network or removable volume?); try a path under /tmp or your home folder",
                            SAVE_LIMIT.as_secs()
                        )));
                    }
                }
            }
        };
        done.map_err(|_| Failure::new("couldn't make the picture"))?
            .map_err(|e| Failure(format!("couldn't encode the picture: {e}")))
    }

    async fn tabs_tool(
        &self,
        args: &Map<String, Value>,
        ctx: &mut CallCtx,
    ) -> Result<Vec<Content>, Failure> {
        let action = arg_str(args, "action").unwrap_or("list");
        let focus = arg_bool(args, "focus").unwrap_or(false);
        let pane = ctx.who.key == "pane";
        let pick = |or_screen: bool| -> Result<TabInfo, Failure> {
            let named = args.get("tab").or_else(|| args.get("index"));
            if let Some(tab) = self.resolve(named, ctx)? {
                return Ok(tab);
            }
            if or_screen
                && let Some(active) = self.host.active_tab().and_then(|id| self.host.tab(&id))
            {
                return Ok(active);
            }
            self.target(ctx)
                .ok_or_else(|| Failure::new("no active tab"))
        };
        match action {
            "list" => Ok(vec![Content::Text(self.tab_list(ctx))]),
            "new" => {
                let raw = arg_str(args, "url").filter(|u| !u.is_empty());
                let url = match raw {
                    Some(r) => copper_core::url_from_typed(r)
                        .map(|u| u.to_string())
                        .ok_or_else(|| Failure(format!("not an address: {r}")))?,
                    None => "about:blank".to_string(),
                };
                let id = self.host.open_tab(&url, focus || pane);
                self.bind(&id, ctx);
                self.prepare(&id);
                if raw.is_some() {
                    self.settle(&id, Duration::from_secs(15)).await;
                }
                let note = if focus || pane {
                    "Opened and shown to the user."
                } else {
                    "Opened in the background — the user's tab did not change; your calls now land on it."
                };
                Ok(vec![Content::Text(format!(
                    "{note} {{tab {}}}\n{}\n\n{}",
                    short(&id),
                    self.page_line(&id),
                    self.tab_list(ctx)
                ))])
            }
            "close" => {
                let tab = pick(false)?;
                let said = self.close_tab(&tab.id);
                Ok(vec![Content::text(format!(
                    "{said} {}",
                    self.tab_list(ctx)
                ))])
            }
            "select" => {
                let tab = pick(true)?;
                self.bind(&tab.id, ctx);
                if focus || pane {
                    self.host.show_tab(&tab.id);
                } else {
                    self.prepare(&tab.id);
                }
                let note = if focus || pane {
                    "Selected and shown to the user."
                } else {
                    "Selected — your calls now land on this tab; the user's view did not change."
                };
                Ok(vec![Content::Text(format!(
                    "{note} {{tab {}}}\n{}",
                    short(&tab.id),
                    self.tab_list(ctx)
                ))])
            }
            _ => Err("action must be list, new, close or select".into()),
        }
    }

    fn groups_tool(&self, args: &Map<String, Value>, ctx: &CallCtx) -> String {
        match arg_str(args, "action").unwrap_or("list") {
            "assign" => {
                let name = arg_str(args, "group").filter(|n| !n.is_empty());
                match (self.target(ctx), name) {
                    (Some(tab), Some(name)) => {
                        self.host.group_assign(&tab.id, name);
                        format!("Grouped into {name}")
                    }
                    _ => "group name required".into(),
                }
            }
            "remove" => {
                if let Some(tab) = self.target(ctx) {
                    self.host.group_remove(&tab.id);
                }
                "Removed from its group".into()
            }
            "list" => {
                let lines: Vec<String> = self
                    .host
                    .groups()
                    .iter()
                    .map(|g| {
                        format!(
                            "- {}: {}",
                            g.name,
                            g.members
                                .iter()
                                .map(|t| format!("[{t}]"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })
                    .collect();
                if lines.is_empty() {
                    "No groups".into()
                } else {
                    lines.join("\n")
                }
            }
            _ => "action must be list, assign or remove".into(),
        }
    }
}

/// `JSONSerialization` with `.sortedKeys` and no other options.
pub(crate) fn sorted_compact(value: &Value) -> String {
    copper_pagejs::render::compact_json(value)
}

/// Pretty, sorted, slashes unescaped (`[.prettyPrinted, .sortedKeys]`).
pub(crate) fn sorted_pretty(value: &Value) -> String {
    pretty_json(value)
}
