//! Which agent session a call comes from (Fork/MCP/Hands.swift `Drive.Who`).
//!
//! The HTTP transport has no sessions of its own, so identity comes from the
//! agent's side: the `--mcp-stdio` bridge and the CLI read the environment
//! the agent gave them and send it on every request as one base64-JSON
//! `X-Copper-Agent` header; a request may say more in
//! `params._meta["copper/agent"]`; a client that talks HTTP directly gets an
//! `Mcp-Session-Id` at `initialize`.

use base64::Engine as _;
use serde_json::{Map, Value};

/// One agent session, as the tab rows and the hover card name it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Who {
    /// Stable for the session: the same thread keeps its tab and its colour.
    pub key: String,
    /// "Claude Code", "Jev", "phi", "Copper's agent".
    pub agent: String,
    /// The thread's title, else project folder and a short session id.
    pub thread: String,
    /// What the colour is hashed from.
    pub seed: String,
    /// For Jev: whose goal it is.
    pub via: String,
    pub project: String,
    /// The short session id (8 characters).
    pub session: String,
}

impl Who {
    /// Copper's own agent in the pane beside the page. It never keeps a tab
    /// of its own: it works on what the user is looking at.
    pub fn pane() -> Who {
        Who {
            key: "pane".into(),
            agent: "Copper's agent".into(),
            thread: "this window".into(),
            seed: "pane".into(),
            ..Who::default()
        }
    }

    /// A bare driver name.
    pub fn plain(name: &str) -> Who {
        Who {
            key: name.into(),
            agent: name.into(),
            seed: name.into(),
            ..Who::default()
        }
    }

    /// A Jev run on someone's behalf: named Jev, labelled with the thread
    /// that asked, coloured like it.
    pub fn jev(caller: Option<&Who>) -> Who {
        match caller {
            Some(c) if c.key != "jev" => Who {
                key: format!("jev:{}", c.key),
                agent: "Jev".into(),
                thread: c.thread.clone(),
                seed: c.seed.clone(),
                via: c.agent.clone(),
                project: c.project.clone(),
                session: c.session.clone(),
            },
            _ => Who {
                key: "jev".into(),
                agent: "Jev".into(),
                seed: "jev".into(),
                ..Who::default()
            },
        }
    }

    /// "Jev · Download Copper", or just the name.
    pub fn line(&self) -> String {
        if self.thread.is_empty() {
            self.agent.clone()
        } else {
            format!("{} · {}", self.agent, self.thread)
        }
    }

    /// Whether this caller keeps a tab of its own (everyone but the pane).
    pub fn binds(&self) -> bool {
        !self.key.is_empty() && self.key != "pane"
    }
}

/// What an agent says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Said {
    /// The MCP client's own name, from initialize.
    pub name: String,
    /// COPPER_AGENT_LABEL: overrides everything else.
    pub label: String,
    /// COPPER_AGENT_THREAD.
    pub thread: String,
    /// A session or thread id from the agent's environment.
    pub session: String,
    /// The agent the CLI or bridge runs under, as detected.
    pub harness: String,
    /// "copper-cli" / "copper-bridge".
    pub client: String,
    /// One bridge process.
    pub instance: String,
    /// The working folder's name.
    pub project: String,
    pub t3home: String,
}

impl Said {
    pub fn from_map(d: &Map<String, Value>) -> Said {
        let s = |k: &str| {
            d.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        Said {
            name: s("name"),
            label: s("label"),
            thread: s("thread"),
            session: s("session"),
            harness: s("harness"),
            client: s("client"),
            instance: s("instance"),
            project: s("project"),
            t3home: s("t3home"),
        }
    }

    /// Fields set here win over `other`'s.
    pub fn over(&self, other: &Said) -> Said {
        let pick = |mine: &String, theirs: &String| {
            if mine.is_empty() {
                theirs.clone()
            } else {
                mine.clone()
            }
        };
        Said {
            name: pick(&self.name, &other.name),
            label: pick(&self.label, &other.label),
            thread: pick(&self.thread, &other.thread),
            session: pick(&self.session, &other.session),
            harness: pick(&self.harness, &other.harness),
            client: pick(&self.client, &other.client),
            instance: pick(&self.instance, &other.instance),
            project: pick(&self.project, &other.project),
            t3home: pick(&self.t3home, &other.t3home),
        }
    }

    /// The `X-Copper-Agent` header: base64 of a JSON object.
    pub fn header(raw: Option<&str>) -> Said {
        let Some(raw) = raw else {
            return Said::default();
        };
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(raw.trim()) else {
            return Said::default();
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(d)) => Said::from_map(&d),
            _ => Said::default(),
        }
    }

    /// The request's own `_meta["copper/agent"]` over the header (`MCP.said`).
    pub fn of_request(params: &Map<String, Value>, header: Option<&str>) -> Said {
        let from_header = Said::header(header);
        match params
            .get("_meta")
            .and_then(|m| m.get("copper/agent"))
            .and_then(Value::as_object)
        {
            Some(meta) => Said::from_map(meta).over(&from_header),
            None => from_header,
        }
    }
}

/// A client's `initialize` name as a person would say it (`MCP.pretty`).
pub fn pretty(raw: &str) -> String {
    let name = raw.trim();
    let low = name.to_lowercase();
    if low.is_empty() {
        return String::new();
    }
    if low.contains("claude") {
        return "Claude Code".into();
    }
    if low == "pi"
        || low == "phi"
        || low.starts_with("pi-")
        || low.starts_with("phi-")
        || low.contains("pi-coding")
    {
        return "phi".into();
    }
    if low.contains("copper-cli") || low == "copper" {
        return "copper CLI".into();
    }
    if low.contains("cursor") {
        return "Cursor".into();
    }
    if low.contains("codex") {
        return "Codex".into();
    }
    if low.contains("mcp-remote") || low.contains("inspector") {
        return "MCP client".into();
    }
    if name.chars().count() > 24 {
        let cut: String = name.chars().take(23).collect();
        return format!("{cut}…");
    }
    name.to_string()
}

/// Put a name and a thread to what was said (`Drive.Who.from`). Thread
/// titles from P3/T3's sqlite are not looked up on Linux.
pub fn from(said: &Said, mcp_session: Option<&str>, fallback_name: &str) -> Who {
    let agent = if !said.label.is_empty() {
        said.label.clone()
    } else if !said.name.is_empty() {
        pretty(&said.name)
    } else if !said.harness.is_empty() {
        pretty(&said.harness)
    } else if !said.client.is_empty() {
        pretty(&said.client)
    } else if fallback_name.is_empty() {
        "Agent".into()
    } else {
        fallback_name.to_string()
    };
    let session = if !said.session.is_empty() {
        said.session.clone()
    } else if !said.instance.is_empty() {
        said.instance.clone()
    } else {
        mcp_session.unwrap_or("").to_string()
    };
    let short: String = session.chars().take(8).collect::<String>().to_lowercase();
    let thread = if !said.thread.is_empty() {
        said.thread.clone()
    } else {
        match (said.project.is_empty(), short.is_empty()) {
            (false, false) => format!("{} · {}", said.project, short),
            (false, true) => said.project.clone(),
            (true, false) => short.clone(),
            (true, true) => String::new(),
        }
    };
    let key = format!(
        "{agent}:{}",
        if session.is_empty() {
            &said.project
        } else {
            &session
        }
    );
    Who {
        seed: if session.is_empty() {
            key.clone()
        } else {
            session.clone()
        },
        key,
        agent,
        thread,
        via: String::new(),
        project: said.project.clone(),
        session: short,
    }
}

/// What the bridge and the CLI say about the agent they run under, from the
/// environment it gave them (`AgentEnvironment`).
pub mod environment {
    use super::*;
    use std::sync::OnceLock;

    /// One bridge process: its own id stands in for a session when the
    /// agent's environment names none.
    pub fn instance() -> &'static str {
        static INSTANCE: OnceLock<String> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            uuid::Uuid::new_v4()
                .hyphenated()
                .to_string()
                .chars()
                .take(8)
                .collect()
        })
    }

    /// The fields, read through `env` (a variable lookup) and `cwd`/`home`.
    pub fn fields_with(
        client: &str,
        name: &str,
        env: &dyn Fn(&str) -> Option<String>,
        cwd: Option<&std::path::Path>,
        home: Option<&std::path::Path>,
    ) -> Map<String, Value> {
        let first = |keys: &[&str]| -> String {
            for k in keys {
                if let Some(v) = env(k) {
                    let v = v.trim();
                    if !v.is_empty() {
                        return v.to_string();
                    }
                }
            }
            String::new()
        };
        let has = |k: &str| env(k).is_some();
        let mut out = Map::new();
        let mut put = |k: &str, v: String| {
            if !v.is_empty() {
                out.insert(k.into(), Value::String(v));
            }
        };
        put("client", client.to_string());
        put("label", first(&["COPPER_AGENT_LABEL"]));
        put("thread", first(&["COPPER_AGENT_THREAD"]));
        put(
            "session",
            first(&[
                "COPPER_AGENT_SESSION",
                "CLAUDE_CODE_SESSION_ID",
                "CODEX_THREAD_ID",
                "CODEX_SESSION_ID",
                "T3CODE_THREAD_ID",
                "P3_THREAD_ID",
                "P3CODE_THREAD_ID",
            ]),
        );
        let harness = if env("CLAUDECODE").as_deref() == Some("1") || has("CLAUDE_CODE_SESSION_ID")
        {
            "Claude Code"
        } else if has("CODEX_THREAD_ID") || has("CODEX_SESSION_ID") || has("CODEX_MANAGED_BY_NPM") {
            "Codex"
        } else if has("PI_CODING_AGENT") || has("PHI_SESSION_ID") {
            "phi"
        } else {
            ""
        };
        put("harness", harness.to_string());
        put("t3home", first(&["T3CODE_HOME"]));
        if let Some(cwd) = cwd
            && cwd != std::path::Path::new("/")
            && Some(cwd) != home
            && let Some(last) = cwd.file_name()
        {
            put("project", last.to_string_lossy().into_owned());
        }
        if client == "copper-bridge" {
            put("instance", instance().to_string());
        }
        put("name", name.to_string());
        out
    }

    /// From the real environment.
    pub fn fields(client: &str, name: &str) -> Map<String, Value> {
        let cwd = std::env::current_dir().ok();
        let home = copper_core::platform::home_dir();
        fields_with(
            client,
            name,
            &|k| std::env::var(k).ok(),
            cwd.as_deref(),
            home.as_deref(),
        )
    }

    /// The `X-Copper-Agent` header value.
    pub fn header(client: &str, name: &str) -> String {
        let json = Value::Object(fields(client, name)).to_string();
        base64::engine::general_purpose::STANDARD.encode(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_names() {
        assert_eq!(pretty("claude-code"), "Claude Code");
        assert_eq!(pretty("pi-coding-agent"), "phi");
        assert_eq!(pretty("copper-cli"), "copper CLI");
        assert_eq!(pretty(""), "");
        assert_eq!(
            pretty("a-very-long-client-name-that-goes-on"),
            "a-very-long-client-name…"
        );
    }

    #[test]
    fn identity_prefers_session_then_instance_then_mcp() {
        let said = Said {
            name: "claude-code".into(),
            project: "copper".into(),
            ..Said::default()
        };
        let who = from(&said, Some("0123456789abcdef"), "");
        assert_eq!(who.agent, "Claude Code");
        assert_eq!(who.key, "Claude Code:0123456789abcdef");
        assert_eq!(who.thread, "copper · 01234567");
        let said = Said {
            label: "Bot".into(),
            instance: "abcd1234".into(),
            ..Said::default()
        };
        assert_eq!(from(&said, Some("x"), "").key, "Bot:abcd1234");
        assert_eq!(from(&Said::default(), None, "").key, "Agent:");
    }

    #[test]
    fn header_and_meta() {
        let raw = base64::engine::general_purpose::STANDARD
            .encode(r#"{"client":"copper-cli","session":"s1","project":"p"}"#);
        let mut params = Map::new();
        params.insert(
            "_meta".into(),
            serde_json::json!({"copper/agent": {"session": "s2"}}),
        );
        let said = Said::of_request(&params, Some(&raw));
        assert_eq!(said.session, "s2");
        assert_eq!(said.client, "copper-cli");
        assert_eq!(said.project, "p");
    }

    #[test]
    fn environment_fields() {
        let env = |k: &str| match k {
            "CLAUDE_CODE_SESSION_ID" => Some("abc".to_string()),
            _ => None,
        };
        let f = environment::fields_with(
            "copper-cli",
            "",
            &env,
            Some(std::path::Path::new("/home/u/proj")),
            Some(std::path::Path::new("/home/u")),
        );
        assert_eq!(f["harness"], "Claude Code");
        assert_eq!(f["session"], "abc");
        assert_eq!(f["project"], "proj");
        assert!(!f.contains_key("instance"));
    }
}
