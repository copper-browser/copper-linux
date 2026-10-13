//! `copper_mcp::Host` for the GTK shell: what the agent tools see of the
//! window and how they reach into it. Everything here runs on the main loop
//! (`Mcp::serve` is spawned there).

use super::backstage::Stage;
use super::mark::Marks;
use super::{crashes, groups, heat};
use crate::shell::{Shell, WeakShell};
use copper_core::TabId;
use copper_engine::PageView;
use copper_engine_webkitgtk::WebKitPage;
use copper_mcp::drive::DriveEvent;
use copper_mcp::{Content, EnginePage, GroupInfo, Host, IntelligenceSource, TabInfo};
use gtk4::prelude::*;
use serde_json::{Map, Value};
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

/// The page type the tools drive.
pub type AgentPage = EnginePage<WebKitPage>;

/// The shell, as the agent tools drive it.
pub struct AppHost {
    shell: WeakShell,
    stage: Rc<Stage>,
    marks: Rc<Marks>,
    heat: heat::Heat,
}

impl AppHost {
    pub fn new(shell: &Shell, stage: Rc<Stage>, marks: Rc<Marks>) -> AppHost {
        AppHost {
            shell: shell.downgrade(),
            stage,
            marks,
            heat: heat::Heat::default(),
        }
    }

    fn shell(&self) -> Option<Shell> {
        self.shell.upgrade()
    }
}

/// Wrap a live page for the tools.
pub fn engine_page(page: &WebKitPage) -> AgentPage {
    let widget = page.widget();
    let w = widget.downgrade();
    let size = move || {
        w.upgrade()
            .map(|w| (w.width() as f64, w.height() as f64))
            .unwrap_or((0.0, 0.0))
    };
    let m = widget.downgrade();
    let mapped = move || m.upgrade().is_some_and(|w| w.is_mapped());
    EnginePage {
        page: Rc::new(page.clone()),
        view_size: Rc::new(size),
        mapped: Rc::new(mapped),
    }
}

/// Every tab as the tools see it: the current space's row (pins first)
/// numbered, everything else after it without an index.
pub fn tab_list(shell: &Shell) -> Vec<TabInfo> {
    let names = groups::names(shell);
    let browser = shell.browser();
    let active = browser.active_id();
    let visible: Vec<TabId> = browser.visible().iter().map(|t| t.id.clone()).collect();
    let mut out = Vec::new();
    let info = |t: &copper_core::Tab, index: Option<usize>| {
        let slot = shell.slot(&t.id);
        let url = slot
            .as_ref()
            .and_then(|s| s.page.url())
            .filter(|u| !u.is_empty())
            .or_else(|| (!t.url.is_empty()).then(|| t.url.clone()));
        TabInfo {
            id: t.id.clone(),
            index,
            title: t.label(),
            url,
            pinned: t.is_pinned(),
            group: t.group.as_ref().and_then(|g| names.get(g).cloned()),
            sleeping: t.sleeping || slot.is_none(),
            selected: active.as_ref() == Some(&t.id),
            loading: slot
                .as_ref()
                .is_some_and(|s| s.loading.get() || s.page.is_loading()),
            cpu: None,
        }
    };
    for (i, id) in visible.iter().enumerate() {
        if let Some(t) = browser.tab(id) {
            out.push(info(t, Some(i)));
        }
    }
    for t in browser.all_tabs() {
        if !visible.contains(&t.id) {
            out.push(info(t, None));
        }
    }
    out
}

impl Host for AppHost {
    type Page = AgentPage;

    fn tabs(&self) -> Vec<TabInfo> {
        self.shell().map(|s| tab_list(&s)).unwrap_or_default()
    }

    fn active_tab(&self) -> Option<TabId> {
        self.shell()?.active_id()
    }

    fn open_tab(&self, url: &str, foreground: bool) -> TabId {
        let Some(shell) = self.shell() else {
            return TabId::new();
        };
        shell.open(
            url,
            crate::shell::OpenTab {
                foreground,
                ..Default::default()
            },
        )
    }

    fn show_tab(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            shell.select(id);
        }
    }

    fn close_tab(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            self.stage.release(&shell, id);
            shell.close(id);
        }
    }

    fn prepare_tab(&self, id: &TabId) -> bool {
        let Some(shell) = self.shell() else {
            return false;
        };
        self.heat.mark();
        self.stage.prepare(&shell, id)
    }

    fn release_tab(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            self.stage.release(&shell, id);
        }
    }

    fn page(&self, id: &TabId) -> Option<AgentPage> {
        self.shell()?.page(id).map(|p| engine_page(&p))
    }

    fn navigate(&self, id: &TabId, url: &str) {
        if let Some(shell) = self.shell() {
            shell.navigate(id, url);
        }
    }

    fn go_back(&self, id: &TabId) {
        if let Some(p) = self.shell().and_then(|s| s.page(id)) {
            p.back();
        }
    }

    fn go_forward(&self, id: &TabId) {
        if let Some(p) = self.shell().and_then(|s| s.page(id)) {
            p.forward();
        }
    }

    fn is_loading(&self, id: &TabId) -> bool {
        self.shell()
            .and_then(|s| s.slot(id))
            .is_some_and(|s| s.loading.get() || s.page.is_loading())
    }

    fn resize_window(&self, width: f64, height: f64) -> Result<(), String> {
        let shell = self.shell().ok_or("no window")?;
        if !(width >= 200.0 && height >= 150.0) {
            return Err("width and height must be at least 200×150".into());
        }
        shell
            .window()
            .set_default_size(width.round() as i32, height.round() as i32);
        Ok(())
    }

    fn groups(&self) -> Vec<GroupInfo> {
        self.shell().map(|s| groups::list(&s)).unwrap_or_default()
    }

    fn group_assign(&self, id: &TabId, name: &str) {
        if let Some(shell) = self.shell() {
            groups::assign(&shell, id, name);
        }
    }

    fn group_remove(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            groups::remove(&shell, id);
        }
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> {
        glib::timeout_future(duration)
    }

    fn announce(&self, text: &str) {
        self.marks.announce(text);
    }

    fn drive(&self, event: &DriveEvent) {
        self.marks.event(event);
    }

    fn processes_line(&self, _id: &TabId) -> String {
        self.heat.line()
    }

    fn sign_in(
        &self,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Value, String>> {
        let shell = self.shell();
        let (id, args) = (id.clone(), args.clone());
        async move {
            let shell = shell.ok_or("no window")?;
            crate::forms::mcp::sign_in(shell, id, args).await
        }
    }

    fn autofill(
        &self,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Value, String>> {
        let shell = self.shell();
        let (id, args) = (id.clone(), args.clone());
        async move {
            let shell = shell.ok_or("no window")?;
            crate::forms::mcp::autofill(shell, id, args).await
        }
    }

    fn canvas(
        &self,
        tool: &str,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Vec<Content>, String>> {
        let shell = self.shell();
        let (tool, args) = (tool.to_string(), args.clone());
        async move {
            let shell = shell.ok_or("no window")?;
            let items = crate::canvas::mcp::call(shell, tool, args).await?;
            Ok(items.into_iter().map(content).collect())
        }
    }

    fn cloud_history_delete(
        &self,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Vec<Content>, String>> {
        let shell = self.shell();
        let args = args.clone();
        async move {
            let shell = shell.ok_or("no window")?;
            let text = crate::cloud::mcp::history_delete(shell, args).await?;
            Ok(vec![Content::Text(text)])
        }
    }

    fn crashes(&self, args: &Map<String, Value>) -> Result<String, String> {
        let shell = self.shell().ok_or("no window")?;
        crashes::answer(shell.store(), args)
    }

    fn intelligence(&self) -> IntelligenceSource {
        match self.shell() {
            Some(s) => copper_mcp::intelligence::read_files(s.store()),
            None => IntelligenceSource::default(),
        }
    }
}

/// One canvas result item as MCP content: a string is text, an
/// `{type: "image", data, mimeType}` object is a picture, an
/// `{type: "text", text}` object is text, anything else is its JSON.
pub fn content(item: Value) -> Content {
    match item {
        Value::String(s) => Content::Text(s),
        Value::Object(ref o) if o.get("type").and_then(Value::as_str) == Some("image") => {
            let data = o.get("data").and_then(Value::as_str).unwrap_or("");
            let mime = o
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_string();
            Content::Image {
                data: glib::base64_decode(data),
                mime,
            }
        }
        Value::Object(ref o) if o.get("type").and_then(Value::as_str) == Some("text") => {
            Content::Text(
                o.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            )
        }
        other => Content::Text(copper_pagejs::render::compact_json(&other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canvas_items_become_content() {
        assert_eq!(content(json!("hi")), Content::Text("hi".into()));
        assert_eq!(
            content(json!({"type": "text", "text": "a"})),
            Content::Text("a".into())
        );
        assert_eq!(
            content(json!({"type": "image", "data": "aGk=", "mimeType": "image/png"})),
            Content::Image {
                data: b"hi".to_vec(),
                mime: "image/png".into()
            }
        );
        assert_eq!(
            content(json!({"ids": [1]})),
            Content::Text("{\"ids\":[1]}".into())
        );
    }
}
