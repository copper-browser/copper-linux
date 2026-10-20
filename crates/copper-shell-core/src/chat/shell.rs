//! The browser's [`ChatHost`]: the shell's store for `intelligence.json`,
//! `cloud-intelligence.json` and `chat.json`; the tab in front for the page
//! context; and the agents lane's [`copper_mcp::Mcp`] for the tools.
//!
//! Tools run in-process through [`copper_mcp::Mcp::tools_call`] as
//! [`copper_mcp::Who::pane`] — Copper's agent, the driver the marks, the
//! hands and the trail already know — so every call is announced, marked
//! and refused after a Stop on its capsule exactly as any other driver's.
//! The `Mcp` lives as long as the window, not as long as the loopback
//! listener: Settings › Agents › "Let agents drive this window" only opens
//! or closes the port.

use std::marker::PhantomData;
use std::time::Duration;

use copper_core::agent::ChatConfig;
use copper_core::intelligence::IntelligenceKeys;
use copper_engine::LocalFuture;
use copper_mcp::intelligence::Tier;
use copper_mcp::{Content, Host as _, IntelligenceSource, Page as _, Who};
use serde_json::{Map, Value};

use super::host::{ChatHost, PageInFront, ToolOutput};
use crate::main_loop::{MainLoop, timeout};
use crate::{Platform, WeakShell, agents};

/// How long the page's text may take to read before the question goes
/// without it.
const PAGE_TEXT_WAIT: Duration = Duration::from_secs(3);

/// The browser's host for the agent in the window.
pub struct ShellChat<P: Platform> {
    shell: WeakShell<P>,
    _platform: PhantomData<P>,
}

impl<P: Platform> ShellChat<P> {
    pub fn new(shell: WeakShell<P>) -> ShellChat<P> {
        ShellChat {
            shell,
            _platform: PhantomData,
        }
    }
}

/// A blank tab gets no page context (`Tab.isBlank`).
fn blank(url: Option<&str>) -> bool {
    matches!(url.map(str::trim), None | Some("" | "about:blank"))
}

impl<P: Platform> ChatHost for ShellChat<P> {
    fn spawn(&self, task: LocalFuture<()>) {
        P::Loop::spawn_local(task);
    }

    fn runtime(&self) -> tokio::runtime::Handle {
        crate::cloud::runtime().handle().clone()
    }

    fn intelligence(&self) -> IntelligenceSource {
        match self.shell.upgrade() {
            Some(s) => copper_mcp::intelligence::read_files(s.store()),
            None => IntelligenceSource::default(),
        }
    }

    fn set_tier(&self, tier: Tier) {
        let Some(shell) = self.shell.upgrade() else {
            return;
        };
        let mut keys = shell
            .store()
            .load::<IntelligenceKeys>()
            .map(|l| l.value)
            .unwrap_or_default();
        keys.tier = Some(tier.name().into());
        if let Err(e) = shell.store().save(&keys) {
            tracing::warn!(target: "copper::agent", "intelligence.json: {e}");
        }
    }

    fn config(&self) -> ChatConfig {
        self.shell
            .upgrade()
            .and_then(|s| s.store().load::<ChatConfig>().ok())
            .map(|l| l.value)
            .unwrap_or_default()
    }

    fn jev_mode(&self) -> bool {
        agents::get::<P>().is_some_and(|a| a.jev())
    }

    fn catalogue(&self, jev: bool) -> Vec<Value> {
        copper_mcp::catalogue::catalogue(jev, true)
    }

    fn page(&self) -> LocalFuture<Option<PageInFront>> {
        let agents = agents::get::<P>();
        Box::pin(async move {
            let mcp = agents?.mcp().clone();
            let id = mcp.host.active_tab()?;
            let tab = mcp.host.tab(&id)?;
            if blank(tab.url.as_deref()) {
                return None;
            }
            let url = tab.url.clone().unwrap_or_default();
            let mut front = PageInFront {
                url: url.clone(),
                title: tab.title.clone(),
                ..PageInFront::default()
            };
            if let Some(canvas) = crate::canvas::id_from_url(&url) {
                let name = crate::canvas::registry::entry(&canvas)
                    .map(|e| e.name)
                    .unwrap_or_else(|| canvas.clone());
                front.canvas = Some((canvas, name));
                return Some(front);
            }
            if let Some(page) = mcp.host.page(&id) {
                let call = copper_pagejs::page::text(None);
                let body = call.function_body();
                let read = page.call_async(&body, call.world().name());
                front.text = match timeout::<P::Loop, _>(PAGE_TEXT_WAIT, read).await {
                    Some(Ok(Value::String(s))) => Some(s),
                    _ => None,
                };
            }
            Some(front)
        })
    }

    fn call(&self, name: &str, args: &Map<String, Value>) -> LocalFuture<ToolOutput> {
        let agents = agents::get::<P>();
        let name = name.to_string();
        let args = args.clone();
        Box::pin(async move {
            let Some(agents) = agents else {
                return ToolOutput {
                    text: "Copper's tools are not running in this window".into(),
                    is_error: true,
                    ..ToolOutput::default()
                };
            };
            let mcp = agents.mcp().clone();
            let (content, is_error, line) = mcp.tools_call(&name, &args, Who::pane(), true).await;
            let mut out = ToolOutput {
                is_error,
                line,
                ..ToolOutput::default()
            };
            let mut texts = Vec::with_capacity(content.len());
            for part in content {
                match part {
                    Content::Text(s) => texts.push(s),
                    Content::Image { data, mime } => {
                        out.pictures.push((data, mime));
                        texts.push("[screenshot attached]".into());
                    }
                }
            }
            out.text = texts.join("\n");
            out
        })
    }

    fn stop_jev(&self) {
        if let Some(a) = agents::get::<P>() {
            a.stop_hand(super::JEV_KEY);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn blank_tabs_have_no_context() {
        assert!(super::blank(None));
        assert!(super::blank(Some("about:blank")));
        assert!(super::blank(Some(" ")));
        assert!(!super::blank(Some("https://example.com/")));
    }
}
