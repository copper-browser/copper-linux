//! The tools an agent gets (Fork/MCP/Tools.swift), named and shaped like
//! Playwright MCP's. Under them is the tab the user has open, through
//! [`Host`] and [`Page`]; the page side is `copper_pagejs`.
//!
//! Elements are addressed by `ref` (from a snapshot) or a CSS `selector`.
//! Clicks and keys go in as real input ([`Page::input`]) when the view can
//! take it; otherwise the page script dispatches DOM events of the same shape.

use std::pin::pin;
use std::time::{Duration, Instant};

use copper_engine::{InputEvent, MouseButton, SnapshotRegion};
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
use crate::shot;
use crate::{CallCtx, Failure, Mcp};

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
                self.close_tab(&tab.id);
                return Ok(vec![Content::text(format!(
                    "Closed. {}",
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
            "browser_take_screenshot" => {
                let jpeg = arg_str(args, "type") == Some("jpeg");
                let full = arg_bool(args, "fullPage").unwrap_or(false);
                let mut rect = None;
                if let Some(reference) = arg_str(args, "ref") {
                    let r = self
                        .prepare_target(&page, &Locator::Ref(reference.into()))
                        .await?;
                    rect = Some((r.x, r.y, r.width, r.height));
                }
                let region = if full && rect.is_none() {
                    SnapshotRegion::FullDocument
                } else {
                    SnapshotRegion::Visible
                };
                let image = page.snapshot(region).await.map_err(Failure)?;
                let (view_width, _) = page.view_size();
                let data =
                    shot::agent_picture(&image.png, view_width, rect, jpeg).map_err(Failure)?;
                if let Some(path) = arg_str(args, "filename") {
                    let _ = std::fs::write(expand_tilde(path), &data);
                }
                Ok(vec![
                    Content::Image {
                        data,
                        mime: if jpeg { "image/jpeg" } else { "image/png" }.into(),
                    },
                    Content::Text(self.page_line(&id)),
                ])
            }
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
                self.close_tab(&tab.id);
                Ok(vec![Content::text(format!(
                    "Closed. {}",
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

fn expand_tilde(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => std::path::PathBuf::from(home).join(rest),
        _ => std::path::PathBuf::from(path),
    }
}
