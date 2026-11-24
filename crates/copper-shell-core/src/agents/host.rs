//! `copper_mcp::Host` for the shell: what the agent tools see of the
//! window and how they reach into it. Everything here runs on the main loop
//! (`Mcp::serve` is spawned there).

use super::{AgentsApp, crashes, groups, heat};
use crate::main_loop::MainLoop;
use crate::shell::{Shell, WeakShell};
use crate::{PageOf, Platform, ShellView};
use copper_core::TabId;
use copper_engine::{PageView, Profile as _, WeakPage};
use copper_mcp::drive::DriveEvent;
use copper_mcp::{Content, EnginePage, GroupInfo, Host, IntelligenceSource, TabInfo};
use serde_json::{Map, Value};
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

/// The page type the tools drive.
pub type AgentPage<P> = EnginePage<PageOf<P>>;

/// The shell, as the agent tools drive it.
pub struct AppHost<P: Platform> {
    shell: WeakShell<P>,
    app: Rc<dyn AgentsApp<P>>,
    heat: heat::Heat,
}

impl<P: Platform> AppHost<P> {
    pub fn new(shell: &Shell<P>, app: Rc<dyn AgentsApp<P>>) -> AppHost<P> {
        AppHost {
            shell: shell.downgrade(),
            app,
            heat: heat::Heat::default(),
        }
    }

    fn shell(&self) -> Option<Shell<P>> {
        self.shell.upgrade()
    }

    /// The tab's cookie jar (its profile).
    fn jar(&self, id: &TabId) -> Result<crate::ProfileOf<P>, String> {
        self.shell()
            .and_then(|s| s.tab_profile(id))
            .ok_or_else(|| "the tab has no page yet".to_string())
    }
}

/// `## Pages' memory`: each loaded page's web process and what it held at
/// the last idle sweep, heaviest first (the Mac's `bench heat` megabytes).
/// Empty before the first sweep.
fn memory_line<P: Platform>(shell: &Shell<P>) -> String {
    let mut by: Vec<(u32, u64, Vec<String>)> = Vec::new();
    for id in shell.pages_map().borrow().keys() {
        let Some(w) = shell.weight_of(id) else {
            continue;
        };
        let host = shell
            .browser()
            .tab(id)
            .map(|t| crate::shell::host_of(&t.url))
            .unwrap_or_default();
        match by.iter_mut().find(|(p, ..)| *p == w.process) {
            Some((_, _, hosts)) => hosts.push(host),
            None => by.push((w.process, w.bytes, vec![host])),
        }
    }
    if by.is_empty() {
        return String::new();
    }
    by.sort_by_key(|a| std::cmp::Reverse(a.1));
    let mut out = String::from("## Pages' memory (at the last idle sweep)\n");
    for (pid, bytes, hosts) in &by {
        out.push_str(&format!(
            "- {} (pid {pid}): {} MB\n",
            hosts.join(", "),
            bytes >> 20
        ));
    }
    let total: u64 = by.iter().map(|b| b.1).sum();
    out.push_str(&format!(
        "Total: {} MB in {} web processes\n\n",
        total >> 20,
        by.len()
    ));
    out
}

/// Wrap a live page for the tools.
pub fn engine_page<V: PageView + 'static>(page: &V) -> EnginePage<V> {
    let w = page.downgrade();
    let size = move || {
        w.upgrade()
            .map(|p| {
                let (w, h) = p.view_size();
                (f64::from(w), f64::from(h))
            })
            .unwrap_or((0.0, 0.0))
    };
    let m = page.downgrade();
    let mapped = move || m.upgrade().is_some_and(|p| p.is_mapped());
    EnginePage {
        page: Rc::new(page.clone()),
        view_size: Rc::new(size),
        mapped: Rc::new(mapped),
    }
}

/// Every tab as the tools see it: the current space's row (pins first)
/// numbered, everything else after it without an index.
pub fn tab_list<P: Platform>(shell: &Shell<P>) -> Vec<TabInfo> {
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

impl<P: Platform> Host for AppHost<P> {
    type Page = AgentPage<P>;

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
        let id = shell.open(
            url,
            crate::shell::OpenTab {
                foreground,
                ..Default::default()
            },
        );
        super::OPENED.with_borrow_mut(|o| o.insert(id.clone()));
        id
    }

    fn show_tab(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            shell.select(id);
        }
    }

    fn close_tab(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            self.app.release(&shell, id);
            shell.close(id);
        }
    }

    fn prepare_tab(&self, id: &TabId) -> bool {
        let Some(shell) = self.shell() else {
            return false;
        };
        self.heat.mark();
        self.app.prepare(&shell, id)
    }

    fn release_tab(&self, id: &TabId) {
        if let Some(shell) = self.shell() {
            self.app.release(&shell, id);
        }
    }

    fn page(&self, id: &TabId) -> Option<AgentPage<P>> {
        self.shell()?.page(id).map(|p| engine_page(&p))
    }

    fn navigate(&self, id: &TabId, url: &str) {
        if let Some(shell) = self.shell() {
            shell.navigate(id, url);
        }
    }

    fn go_back(&self, id: &TabId) -> bool {
        let page = self.shell().and_then(|s| s.page(id));
        let Some(p) = page.filter(|p| p.can_go_back()) else {
            return false;
        };
        p.back();
        true
    }

    fn go_forward(&self, id: &TabId) -> bool {
        let page = self.shell().and_then(|s| s.page(id));
        let Some(p) = page.filter(|p| p.can_go_forward()) else {
            return false;
        };
        p.forward();
        true
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
            .view()
            .resize_window(width.round() as i32, height.round() as i32);
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
        P::Loop::sleep(duration)
    }

    fn announce(&self, text: &str) {
        self.app.announce(text);
    }

    fn drive(&self, event: &DriveEvent) {
        self.app.drive(event);
        super::tell_drive(event);
    }

    fn processes_line(&self, _id: &TabId) -> String {
        let memory = self.shell().map(|s| memory_line(&s)).unwrap_or_default();
        format!("{}{memory}", self.heat.line())
    }

    // -- page tools (tools-page) ----------------------------------------------

    fn spawn(&self, task: std::pin::Pin<Box<dyn Future<Output = ()>>>) {
        P::Loop::spawn_local(task);
    }

    fn downloads_folder(&self) -> std::path::PathBuf {
        match self.shell() {
            Some(shell) => shell.downloads().folder(&shell),
            None => copper_core::platform::home_dir()
                .unwrap_or_default()
                .join("Downloads"),
        }
    }

    fn cookie_jar(
        &self,
        id: &TabId,
    ) -> impl Future<Output = Result<Vec<copper_engine::Cookie>, String>> {
        let jar = self.jar(id);
        async move { jar?.all_cookies().await.map_err(|e| e.to_string()) }
    }

    fn set_cookie(
        &self,
        id: &TabId,
        cookie: copper_engine::Cookie,
    ) -> impl Future<Output = Result<(), String>> {
        let jar = self.jar(id);
        async move { jar?.set_cookie(&cookie).await.map_err(|e| e.to_string()) }
    }

    fn delete_cookie(
        &self,
        id: &TabId,
        cookie: copper_engine::Cookie,
    ) -> impl Future<Output = Result<(), String>> {
        let jar = self.jar(id);
        async move { jar?.delete_cookie(&cookie).await.map_err(|e| e.to_string()) }
    }

    fn site_shared_with_agents(&self, url: &str) -> impl Future<Output = bool> {
        let (shell, url) = (self.shell(), url.to_string());
        async move {
            match shell {
                Some(shell) => crate::forms::site_shared(&shell, &url).await,
                None => false,
            }
        }
    }

    fn site_permission(&self, site: &str, kind: &str) -> bool {
        self.shell().is_some_and(|s| s.site_granted(kind, site))
    }

    fn set_site_permission(&self, site: &str, kind: &str, allowed: bool) {
        if let Some(shell) = self.shell() {
            shell.set_site_granted(kind, site, allowed);
        }
    }

    fn geolocation(&self) -> Option<copper_engine::GeoPosition> {
        self.shell().and_then(|s| s.position())
    }

    fn set_geolocation(&self, position: Option<copper_engine::GeoPosition>) {
        if let Some(shell) = self.shell() {
            shell.set_position(position);
        }
    }

    fn sign_in(
        &self,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Value, String>> {
        let shell = self.shell();
        let (id, args) = (id.clone(), args.clone());
        let app = self.app.clone();
        async move {
            let shell = shell.ok_or("no window")?;
            app.sign_in(shell, id, args).await
        }
    }

    fn autofill(
        &self,
        id: &TabId,
        args: &Map<String, Value>,
    ) -> impl Future<Output = Result<Value, String>> {
        let shell = self.shell();
        let (id, args) = (id.clone(), args.clone());
        let app = self.app.clone();
        async move {
            let shell = shell.ok_or("no window")?;
            app.autofill(shell, id, args).await
        }
    }

    fn jev_extras(&self, id: &TabId) -> impl Future<Output = copper_pagejs::jev::Extras> {
        let shell = self.shell();
        let id = id.clone();
        async move {
            match shell {
                Some(shell) => crate::forms::mcp::jev_extras(shell, id).await,
                None => Default::default(),
            }
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

    fn control(&self, method: &str, params: &Map<String, Value>) -> impl Future<Output = Value> {
        let answer = match self.shell() {
            Some(shell) if method == "copper/intelligence" => {
                let cloud = crate::cloud::get();
                let fetch = cloud.map_or("never".into(), |c| c.intelligence.last_outcome());
                let mut out = copper_mcp::intelligence::control(
                    params,
                    shell.store(),
                    self.claude_ready(),
                    &fetch,
                    || {
                        if let Some(c) = cloud {
                            let i = c.intelligence.clone();
                            crate::cloud::spawn(async move { i.fetch().await });
                        }
                    },
                );
                if let Some(account) = out.get_mut("claudeAccount") {
                    *account = Value::String(crate::claude::account(shell.store()).status().email);
                }
                out
            }
            Some(shell) if method == "copper/claude" => crate::claude::account(shell.store())
                .control(params, || crate::claude::sign_in(&shell)),
            Some(shell) if method == "copper/pins" => {
                shell.pins_status(params.get("on").and_then(Value::as_bool))
            }
            // `copper/link`, `copper/bitwarden`: not here yet.
            _ => {
                serde_json::json!({ "error": format!("{method} is not available in this build yet") })
            }
        };
        async move { answer }
    }

    fn intelligence(&self) -> IntelligenceSource {
        match self.shell() {
            Some(s) => copper_mcp::intelligence::read_files(s.store()),
            None => IntelligenceSource::default(),
        }
    }

    fn claude_ask(
        &self,
        system: &str,
        user: &str,
        model: &str,
        max_tokens: u32,
        timeout: Duration,
    ) -> impl Future<Output = Result<(String, String), String>> {
        let asked = self.shell().map(|s| {
            crate::claude::account(s.store()).ask(system, user, model, max_tokens, timeout)
        });
        async move {
            match asked {
                Some(asked) => asked.await.map_err(|f| f.text()),
                None => Err("Copper's window is gone".to_string()),
            }
        }
    }

    fn claude_ready(&self) -> bool {
        self.shell()
            .is_some_and(|s| crate::claude::ready(s.store()))
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
                data: crate::canvas::unb64(data),
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
