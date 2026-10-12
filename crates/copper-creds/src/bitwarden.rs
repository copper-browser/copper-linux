//! Bitwarden through its official CLI, `bw` (`Bitwarden.swift`).
//!
//! The unlocked cache holds what autofill needs — logins, cards, identities,
//! notes, hidden fields — in memory only, and is wiped when the vault locks.
//! The session key reaches `bw` through `BW_SESSION`, the master password
//! through `BW_PASSWORD`, API keys through `BW_CLIENTID`/`BW_CLIENTSECRET`;
//! never through an argument (items to save go through stdin). With
//! *stay unlocked* the session key is kept in a 0600 file beside `bw`'s data.
//!
//! Where `bw` is, first match wins: `COPPER_BW_PATH`, `~/.local/bin/bw`,
//! then `$PATH`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::credential::{
    AgentHint, AutofillCard, AutofillIdentity, Credential, CredentialId, Source, VaultField,
};
use crate::process::{self, RunError};
use crate::site::{Uri, matches, uri_host};
use crate::totp;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct BwError(pub String);

fn fail(message: impl Into<String>) -> BwError {
    BwError(message.into())
}

pub type Result<T> = std::result::Result<T, BwError>;

/// What `bw` reports.
#[derive(Debug, Clone, PartialEq)]
pub enum BwState {
    /// No `bw` on this machine.
    Missing,
    Unauthenticated,
    Locked {
        email: Option<String>,
    },
    Unlocked {
        email: Option<String>,
        last_sync: Option<String>,
    },
}

/// A deliberately stripped vault item; its secrets stay in the cache.
#[derive(Debug, Clone, PartialEq)]
pub struct BwItem {
    pub id: String,
    /// 1 login, 2 secure note, 3 card, 4 identity.
    pub kind: i64,
    pub name: String,
    pub folder_id: Option<String>,
    pub username: String,
    pub uris: Vec<Uri>,
    pub has_totp: bool,
    pub has_notes: bool,
    pub fields: Vec<VaultField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BwFolder {
    pub id: String,
    pub name: String,
}

#[derive(Default, Clone)]
struct Secret {
    password: Option<String>,
    totp: Option<String>,
    notes: Option<String>,
    hidden: HashMap<String, String>,
}

/// Where `bw` lives: `override_path` (`COPPER_BW_PATH`, `~` expanded), then
/// `~/.local/bin/bw`, then `$PATH`.
pub fn locate(
    override_path: Option<&str>,
    home: Option<&Path>,
    path_var: Option<&str>,
) -> Option<PathBuf> {
    if let Some(raw) = override_path.map(str::trim).filter(|r| !r.is_empty()) {
        let path = process::expand_tilde(raw, home);
        if process::is_executable(&path) {
            return Some(path);
        }
    }
    if let Some(home) = home {
        let local = home.join(".local/bin/bw");
        if process::is_executable(&local) {
            return Some(local);
        }
    }
    process::in_path("bw", path_var)
}

/// `locate` with this process's environment.
pub fn executable() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    locate(
        std::env::var("COPPER_BW_PATH").ok().as_deref(),
        home.as_deref(),
        std::env::var("PATH").ok().as_deref(),
    )
}

/// How to reach `bw`.
#[derive(Debug, Clone)]
pub struct BwConfig {
    /// `bw` itself; `None` = [`executable`] at each call.
    pub path: Option<PathBuf>,
    /// `BITWARDENCLI_APPDATA_DIR`: Copper's own `bw` data folder, which also
    /// holds the persisted session (`<dir>/session`).
    pub app_data: PathBuf,
    /// Extra environment for every child (tests).
    pub env: Vec<(String, String)>,
}

impl BwConfig {
    /// `<copper data dir>/bitwarden`.
    pub fn in_data_dir(data_dir: &Path) -> Self {
        BwConfig {
            path: None,
            app_data: data_dir.join("bitwarden"),
            env: Vec::new(),
        }
    }
}

/// Bitwarden's state and unlocked cache.
pub struct Bitwarden {
    cfg: BwConfig,
    state: BwState,
    session: Option<String>,
    stay_unlocked: bool,
    items: Vec<BwItem>,
    folders: Vec<BwFolder>,
    identities: Vec<AutofillIdentity>,
    cards: Vec<AutofillCard>,
    secrets: HashMap<String, Secret>,
    cli_server: Option<String>,
    cli_version: Option<String>,
    last_activity: Instant,
    cache_version: u64,
}

impl std::fmt::Debug for Bitwarden {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bitwarden")
            .field("state", &self.state)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

const DROPPED: [&str; 4] = [
    "BW_SESSION",
    "BW_PASSWORD",
    "BW_CLIENTID",
    "BW_CLIENTSECRET",
];

impl Bitwarden {
    pub fn new(cfg: BwConfig, stay_unlocked: bool) -> Self {
        let installed = cfg.path.clone().or_else(executable).is_some();
        Bitwarden {
            cfg,
            state: if installed {
                BwState::Unauthenticated
            } else {
                BwState::Missing
            },
            session: None,
            stay_unlocked,
            items: Vec::new(),
            folders: Vec::new(),
            identities: Vec::new(),
            cards: Vec::new(),
            secrets: HashMap::new(),
            cli_server: None,
            cli_version: None,
            last_activity: Instant::now(),
            cache_version: 0,
        }
    }

    fn exe(&self) -> Option<PathBuf> {
        self.cfg.path.clone().or_else(executable)
    }

    pub fn installed(&self) -> bool {
        self.exe().is_some()
    }

    pub fn state(&self) -> &BwState {
        &self.state
    }

    pub fn is_unlocked(&self) -> bool {
        matches!(self.state, BwState::Unlocked { .. })
    }

    /// The server `bw status` reported, if any.
    pub fn cli_server(&self) -> Option<&str> {
        self.cli_server.as_deref()
    }

    /// Bumped whenever the cache changes.
    pub fn cache_version(&self) -> u64 {
        self.cache_version
    }

    fn session_file(&self) -> PathBuf {
        self.cfg.app_data.join("session")
    }

    pub fn stay_unlocked(&self) -> bool {
        self.stay_unlocked
    }

    pub fn set_stay_unlocked(&mut self, on: bool) {
        self.stay_unlocked = on;
        if on {
            self.persist_session();
        } else {
            self.forget_persisted_session();
        }
    }

    fn persist_session(&self) {
        if !self.stay_unlocked {
            return;
        }
        if let Some(key) = &self.session
            && let Err(e) = process::write_private(&self.session_file(), key)
        {
            tracing::warn!("bitwarden: could not keep the session file: {e}");
        }
    }

    fn forget_persisted_session(&self) {
        let _ = std::fs::remove_file(self.session_file());
    }

    /// A session `bw login`/`unlock` returned elsewhere: kept (and persisted).
    pub fn adopt_session(&mut self, key: &str) {
        self.session = Some(key.to_string());
        self.persist_session();
    }

    /// What last launch left: a persisted session, then `bw status`, then the
    /// cache.
    pub async fn start(&mut self) {
        if !self.installed() {
            self.state = BwState::Missing;
            return;
        }
        if self.stay_unlocked
            && let Some(key) = process::read_private(&self.session_file())
        {
            self.session = Some(key);
        }
        self.refresh_status().await;
        self.refresh_cache_if_possible().await;
    }

    /// Run `bw`. `extra` is the password environment for login/unlock.
    async fn run(
        &mut self,
        args: &[&str],
        extra: &[(&str, &str)],
        stdin: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        let exe = self
            .exe()
            .ok_or_else(|| fail("Bitwarden CLI is not installed"))?;
        let mut env: BTreeMap<String, String> = process::scrubbed_env(&DROPPED, &[]);
        env.insert(
            "BITWARDENCLI_APPDATA_DIR".into(),
            self.cfg.app_data.to_string_lossy().into_owned(),
        );
        env.insert("BW_NOINTERACTION".into(), "true".into());
        if let Some(key) = &self.session {
            env.insert("BW_SESSION".into(), key.clone());
        }
        for (k, v) in &self.cfg.env {
            env.insert(k.clone(), v.clone());
        }
        for (k, v) in extra {
            env.insert((*k).into(), (*v).into());
        }
        if self.session.is_some() && !matches!(args.first(), Some(&"status") | Some(&"--version")) {
            self.last_activity = Instant::now();
        }
        {
            use std::os::unix::fs::DirBuilderExt;
            let _ = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.cfg.app_data);
        }
        tracing::debug!(command = args.first().copied().unwrap_or(""), "bw");
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let out = match process::execute(&exe, &args, &env, stdin, timeout).await {
            Ok(out) => out,
            Err(RunError::TimedOut) => return Err(fail("Bitwarden command timed out")),
            Err(RunError::Spawn(e)) => return Err(fail(format!("Couldn't start bw: {e}"))),
        };
        if out.status != 0 {
            let text = String::from_utf8_lossy(&out.stderr);
            let lines: Vec<&str> = text
                .split(['\n', '\r'])
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            // The SDK's Rust log lines come before the sentence meant for
            // people; prefer that sentence.
            let line = lines
                .iter()
                .find(|l| !is_log_line(l))
                .or(lines.first())
                .map(|l| l.to_string());
            return Err(fail(
                line.unwrap_or_else(|| "Bitwarden command failed".into()),
            ));
        }
        Ok(out.stdout)
    }

    /// `bw --version`, kept once read.
    pub async fn version(&mut self) -> Option<String> {
        if let Some(v) = &self.cli_version {
            return Some(v.clone());
        }
        let data = self
            .run(&["--version"], &[], None, Duration::from_secs(10))
            .await
            .ok()?;
        let text = process::trim_newlines(&data).trim().to_string();
        if text.is_empty() || text.chars().count() > 40 || text.contains('\n') {
            return None;
        }
        self.cli_version = Some(text.clone());
        Some(text)
    }

    /// `bw status` → state. An unlocked CLI is only unlocked for Copper when
    /// Copper holds the session key.
    pub async fn refresh_status(&mut self) {
        if !self.installed() {
            self.state = BwState::Missing;
            return;
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            status: String,
            user_email: Option<String>,
            last_sync: Option<String>,
            server_url: Option<String>,
        }
        let read = self
            .run(&["status"], &[], None, Duration::from_secs(10))
            .await;
        let status =
            read.and_then(|d| serde_json::from_slice::<Raw>(&d).map_err(|e| fail(e.to_string())));
        match status {
            Ok(status) => {
                self.cli_server = status.server_url.filter(|s| !s.is_empty());
                match status.status.to_lowercase().as_str() {
                    "unlocked" if self.session.is_some() => {
                        self.state = BwState::Unlocked {
                            email: status.user_email,
                            last_sync: status.last_sync,
                        };
                    }
                    "unlocked" => {
                        self.state = BwState::Locked {
                            email: status.user_email,
                        }
                    }
                    "locked" => {
                        self.session = None;
                        self.clear_cache();
                        self.forget_persisted_session();
                        self.state = BwState::Locked {
                            email: status.user_email,
                        };
                    }
                    _ => {
                        self.session = None;
                        self.clear_cache();
                        self.forget_persisted_session();
                        self.state = BwState::Unauthenticated;
                    }
                }
            }
            Err(_) => {
                if self.session.is_none() && self.state != BwState::Missing {
                    self.state = BwState::Unauthenticated;
                }
            }
        }
    }

    /// `bw config server <url>`.
    pub async fn configure(&mut self, server: &str) -> Result<()> {
        let value = server.trim();
        if value.is_empty() {
            return Err(fail("Bitwarden server URL is empty"));
        }
        self.run(
            &["config", "server", value],
            &[],
            None,
            Duration::from_secs(10),
        )
        .await?;
        Ok(())
    }

    /// Personal API key login (no two-step prompt); leaves the account
    /// signed in but locked — follow with [`Bitwarden::unlock`].
    pub async fn login_api_key(&mut self, client_id: &str, client_secret: &str) -> Result<()> {
        let (id, secret) = (client_id.trim(), client_secret.trim());
        if id.is_empty() || secret.is_empty() {
            return Err(fail(
                "Bitwarden API key client_id and client_secret are both required",
            ));
        }
        self.run(
            &["login", "--apikey", "--raw"],
            &[("BW_CLIENTID", id), ("BW_CLIENTSECRET", secret)],
            None,
            Duration::from_secs(30),
        )
        .await?;
        self.refresh_status().await;
        Ok(())
    }

    /// Email + master password sign-in, with a two-step code when the
    /// account has one (`--method`, `--code`). Returns with the vault open.
    /// (The interactive form that waits for an emailed code is the app's.)
    pub async fn login(
        &mut self,
        email: &str,
        password: &str,
        method: Option<u8>,
        code: Option<&str>,
    ) -> Result<()> {
        if email.trim().is_empty() || password.is_empty() {
            return Err(fail("Email and master password are required"));
        }
        let method_text = method.map(|m| m.to_string());
        let mut args = vec![
            "login",
            email.trim(),
            "--passwordenv",
            "BW_PASSWORD",
            "--raw",
        ];
        let code = code.map(str::trim).filter(|c| !c.is_empty());
        if let Some(code) = code {
            args.extend([
                "--method",
                method_text.as_deref().unwrap_or("0"),
                "--code",
                code,
            ]);
        }
        let data = self
            .run(
                &args,
                &[("BW_PASSWORD", password)],
                None,
                Duration::from_secs(60),
            )
            .await
            .map_err(|e| fail(login_failure(&e.0)))?;
        let key = process::trim_newlines(&data).trim().to_string();
        if key.is_empty() {
            return Err(fail("Bitwarden did not return a session"));
        }
        self.session = Some(key);
        self.persist_session();
        self.refresh_status().await;
        self.refresh_cache_if_possible().await;
        Ok(())
    }

    /// `bw unlock` with the master password through `BW_PASSWORD`.
    pub async fn unlock(&mut self, password: &str) -> Result<()> {
        if password.is_empty() {
            return Err(fail("Bitwarden master password is required"));
        }
        let data = self
            .run(
                &["unlock", "--passwordenv", "BW_PASSWORD", "--raw"],
                &[("BW_PASSWORD", password)],
                None,
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| fail(unlock_message(&e.0)))?;
        let key = process::trim_newlines(&data).trim().to_string();
        if key.is_empty() {
            return Err(fail("Bitwarden did not return a session"));
        }
        self.session = Some(key);
        self.persist_session();
        self.refresh_status().await;
        self.refresh_cache_if_possible().await;
        Ok(())
    }

    fn email(&self) -> Option<String> {
        match &self.state {
            BwState::Locked { email } | BwState::Unlocked { email, .. } => email.clone(),
            _ => None,
        }
    }

    pub async fn lock(&mut self) {
        let missing = self.state == BwState::Missing;
        let _ = self
            .run(&["lock"], &[], None, Duration::from_secs(10))
            .await;
        self.session = None;
        self.clear_cache();
        self.forget_persisted_session();
        self.state = if missing {
            BwState::Missing
        } else {
            BwState::Locked {
                email: self.email(),
            }
        };
    }

    pub async fn logout(&mut self) {
        let missing = self.state == BwState::Missing;
        let _ = self
            .run(&["logout"], &[], None, Duration::from_secs(10))
            .await;
        self.session = None;
        self.clear_cache();
        self.forget_persisted_session();
        self.state = if missing {
            BwState::Missing
        } else {
            BwState::Unauthenticated
        };
    }

    /// Lock after `minutes` without use (0 = never, the default).
    pub async fn check_idle(&mut self, minutes: f64) {
        if self.session.is_none() || minutes <= 0.0 {
            return;
        }
        if self.last_activity.elapsed() >= Duration::from_secs_f64(minutes * 60.0) {
            self.lock().await;
        }
    }

    pub async fn sync(&mut self) -> Result<()> {
        self.require_unlocked()?;
        self.run(&["sync"], &[], None, Duration::from_secs(30))
            .await?;
        self.refresh_status().await;
        self.refresh_cache().await
    }

    fn require_unlocked(&self) -> Result<()> {
        if self.session.is_none() || !self.is_unlocked() {
            return Err(fail("Bitwarden is locked"));
        }
        Ok(())
    }

    fn clear_cache(&mut self) {
        self.items.clear();
        self.folders.clear();
        self.identities.clear();
        self.cards.clear();
        self.secrets.clear();
        self.cache_version += 1;
    }

    async fn refresh_cache(&mut self) -> Result<()> {
        self.items().await?;
        self.folders().await?;
        Ok(())
    }

    /// Re-read the cache when there is a session; failures are quiet (a
    /// successful unlock stays useful offline).
    pub async fn refresh_cache_if_possible(&mut self) {
        if self.session.is_some() && self.is_unlocked() {
            let _ = self.refresh_cache().await;
        }
    }

    /// `bw list items`, stripped into the cache.
    pub async fn items(&mut self) -> Result<Vec<BwItem>> {
        self.require_unlocked()?;
        let data = self
            .run(&["list", "items"], &[], None, Duration::from_secs(30))
            .await?;
        let rows: Vec<Value> = serde_json::from_slice(&data)
            .map_err(|_| fail("Bitwarden returned an unreadable list"))?;
        let mut fresh = HashMap::new();
        let mut identities = Vec::new();
        let mut cards = Vec::new();
        let mut items = Vec::new();
        for row in &rows {
            if let Some(item) = parse_item(row, &mut fresh, &mut identities, &mut cards) {
                items.push(item);
            }
        }
        self.items = items.clone();
        self.identities = identities;
        self.cards = cards;
        self.secrets = fresh;
        self.cache_version += 1;
        Ok(items)
    }

    /// `bw list folders`.
    pub async fn folders(&mut self) -> Result<Vec<BwFolder>> {
        self.require_unlocked()?;
        let data = self
            .run(&["list", "folders"], &[], None, Duration::from_secs(30))
            .await?;
        let rows: Vec<Value> = serde_json::from_slice(&data)
            .map_err(|_| fail("Bitwarden returned unreadable folders"))?;
        self.folders = rows
            .iter()
            .filter_map(|r| {
                let id = r.get("id")?.as_str().filter(|s| !s.is_empty())?;
                Some(BwFolder {
                    id: id.into(),
                    name: r.get("name").and_then(Value::as_str).unwrap_or("").into(),
                })
            })
            .collect();
        Ok(self.folders.clone())
    }

    pub fn cached_items(&self) -> &[BwItem] {
        &self.items
    }

    pub fn cached_folders(&self) -> &[BwFolder] {
        &self.folders
    }

    pub fn identities(&self) -> &[AutofillIdentity] {
        &self.identities
    }

    pub fn cards(&self) -> &[AutofillCard] {
        &self.cards
    }

    /// (logins, identities, cards, notes)
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        (
            self.items.iter().filter(|i| i.kind == 1).count(),
            self.identities.len(),
            self.cards.len(),
            self.items
                .iter()
                .filter(|i| i.kind == 2 && i.has_notes)
                .count(),
        )
    }

    /// One login's password: from the cache, else `bw get password`.
    pub async fn password(&mut self, id: &str) -> Result<String> {
        self.require_unlocked()?;
        if let Some(kept) = self.secrets.get(id).and_then(|s| s.password.clone()) {
            return Ok(kept);
        }
        let data = self
            .run(&["get", "password", id], &[], None, Duration::from_secs(15))
            .await?;
        Ok(process::trim_newlines(&data))
    }

    /// The current code: computed here from an ordinary seed, else `bw`'s.
    pub async fn totp(&mut self, id: &str) -> Result<String> {
        self.require_unlocked()?;
        if let Some(code) = self
            .secrets
            .get(id)
            .and_then(|s| s.totp.as_deref())
            .and_then(totp::code_now)
        {
            return Ok(code);
        }
        let data = self
            .run(&["get", "totp", id], &[], None, Duration::from_secs(15))
            .await?;
        Ok(process::trim_newlines(&data))
    }

    /// A custom field's value (a hidden one from the in-memory cache).
    pub fn field_value(&self, item_id: &str, name: &str) -> Option<String> {
        let item = self.items.iter().find(|i| i.id == item_id)?;
        let field = item.fields.iter().find(|f| f.name == name)?;
        if field.hidden {
            return Some(
                self.secrets
                    .get(item_id)
                    .and_then(|s| s.hidden.get(name).cloned())
                    .unwrap_or_default(),
            );
        }
        Some(field.value.clone())
    }

    /// A secure note's text.
    pub fn notes(&self, item_id: &str) -> Option<String> {
        self.secrets.get(item_id)?.notes.clone()
    }

    /// `bw encode` with the item on stdin, then `bw <verb> item [id]` with the
    /// encoded item on stdin too — never on argv.
    async fn encode(&mut self, object: &Map<String, Value>) -> Result<String> {
        let json = serde_json::to_vec(object).map_err(|e| fail(e.to_string()))?;
        let encoded = self
            .run(&["encode"], &[], Some(&json), Duration::from_secs(15))
            .await?;
        let value = process::trim_newlines(&encoded);
        if value.is_empty() {
            return Err(fail("Bitwarden did not encode the item"));
        }
        Ok(value)
    }

    /// The account exists with another password: change it there.
    pub async fn update(&mut self, id: &str, password: &str) -> Result<()> {
        self.require_unlocked()?;
        let current = self
            .run(&["get", "item", id], &[], None, Duration::from_secs(15))
            .await?;
        let mut object: Map<String, Value> = serde_json::from_slice(&current)
            .map_err(|_| fail("Bitwarden returned an invalid item"))?;
        let mut login = object
            .get("login")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        login.insert("password".into(), Value::String(password.into()));
        object.insert("login".into(), Value::Object(login));
        let value = self.encode(&object).await?;
        self.run(
            &["edit", "item", id],
            &[],
            Some(value.as_bytes()),
            Duration::from_secs(30),
        )
        .await?;
        self.refresh_cache_if_possible().await;
        Ok(())
    }

    /// A new login for `host`; returns its id.
    pub async fn create(&mut self, host: &str, user: &str, password: &str) -> Result<String> {
        self.require_unlocked()?;
        let template = self
            .run(
                &["get", "template", "item"],
                &[],
                None,
                Duration::from_secs(15),
            )
            .await?;
        let mut object: Map<String, Value> = serde_json::from_slice(&template)
            .map_err(|_| fail("Bitwarden returned an invalid item template"))?;
        object.insert("type".into(), 1.into());
        object.insert("name".into(), host.into());
        object.insert(
            "login".into(),
            serde_json::json!({
                "username": user,
                "password": password,
                "totp": null,
                "uris": [{"uri": format!("https://{host}"), "match": null}],
            }),
        );
        let value = self.encode(&object).await?;
        let created = self
            .run(
                &["create", "item"],
                &[],
                Some(value.as_bytes()),
                Duration::from_secs(30),
            )
            .await?;
        let made: Value = serde_json::from_slice(&created)
            .map_err(|_| fail("Bitwarden did not return the new item id"))?;
        let id = made
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| fail("Bitwarden did not return the new item id"))?
            .to_string();
        self.refresh_cache_if_possible().await;
        Ok(id)
    }

    /// A fresh 20-character password.
    pub async fn generate(&mut self) -> Result<String> {
        let data = self
            .run(
                &["generate", "-ulns", "--length", "20"],
                &[],
                None,
                Duration::from_secs(15),
            )
            .await?;
        Ok(process::trim_newlines(&data))
    }

    /// Logins whose websites match `host` (empty while locked).
    pub fn items_matching(&self, host: &str) -> Vec<&BwItem> {
        if !self.is_unlocked() {
            return Vec::new();
        }
        self.items
            .iter()
            .filter(|i| i.kind == 1 && i.uris.iter().any(|u| matches(u, host)))
            .collect()
    }

    /// The picker's rows for `host`.
    pub fn credentials_for(&self, host: &str) -> Vec<Credential> {
        let folders = self.folder_names();
        self.items_matching(host)
            .into_iter()
            .map(|i| credential(i, &folders))
            .collect()
    }

    /// Every login as a picker row (empty while locked).
    pub fn credentials(&self) -> Vec<Credential> {
        if !self.is_unlocked() {
            return Vec::new();
        }
        let folders = self.folder_names();
        self.items
            .iter()
            .filter(|i| i.kind == 1)
            .map(|i| credential(i, &folders))
            .collect()
    }

    fn folder_names(&self) -> HashMap<String, String> {
        self.folders
            .iter()
            .map(|f| (f.id.clone(), f.name.clone()))
            .collect()
    }
}

fn is_log_line(line: &str) -> bool {
    let Some((level, _)) = line.split_once(' ') else {
        return false;
    };
    ["ERROR", "WARN", "INFO", "DEBUG", "TRACE"].contains(&level) && line.contains("::")
}

/// What `bw unlock` says about a wrong master password, in words.
pub fn unlock_message(raw: &str) -> String {
    let said = raw.to_lowercase();
    if said.contains("decryption operation failed")
        || said.contains("invalid master password")
        || said.contains("cryptography error")
    {
        return "Wrong master password — try again".into();
    }
    raw.into()
}

fn login_failure(raw: &str) -> String {
    let said = raw.to_lowercase();
    if said.contains("username or password is incorrect") || said.contains("invalid_grant") {
        return "Wrong email or master password".into();
    }
    if said.contains("code is required") || said.contains("two-step") {
        return "This account needs a two-step code".into();
    }
    raw.into()
}

/// A JSON value as text, the way `decodeLossyString` reads it.
fn lossy(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn parse_item(
    row: &Value,
    secrets: &mut HashMap<String, Secret>,
    identities: &mut Vec<AutofillIdentity>,
    cards: &mut Vec<AutofillCard>,
) -> Option<BwItem> {
    let kind = row.get("type")?.as_i64()?;
    let id = row
        .get("id")?
        .as_str()
        .filter(|s| !s.is_empty())?
        .to_string();
    let name = row
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let login = row.get("login").and_then(Value::as_object);
    let text = |obj: Option<&Map<String, Value>>, key: &str| {
        obj.and_then(|o| o.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let mut secret = Secret {
        password: text(login, "password"),
        totp: text(login, "totp"),
        notes: row
            .get("notes")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        hidden: HashMap::new(),
    };
    let uris = login
        .and_then(|l| l.get("uris"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|u| {
                    let uri = u.get("uri")?.as_str().filter(|s| !s.is_empty())?;
                    Some(Uri {
                        uri: uri.into(),
                        match_: u.get("match").and_then(Value::as_i64),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut fields = Vec::new();
    for raw in row
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(fname) = raw
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let value = raw.get("value");
        match raw.get("type").and_then(Value::as_i64).unwrap_or(0) {
            0 => {
                if value.is_none_or(Value::is_null) {
                    continue;
                }
                fields.push(VaultField {
                    name: fname.into(),
                    value: lossy(value),
                    hidden: false,
                });
            }
            1 => {
                secret.hidden.insert(fname.into(), lossy(value));
                fields.push(VaultField {
                    name: fname.into(),
                    value: String::new(),
                    hidden: true,
                });
            }
            2 => {
                let on = match value {
                    Some(Value::Bool(b)) => *b,
                    other => lossy(other).eq_ignore_ascii_case("true"),
                };
                fields.push(VaultField {
                    name: fname.into(),
                    value: on.to_string(),
                    hidden: false,
                });
            }
            // Linked fields point at another item; not values.
            _ => {}
        }
    }
    match kind {
        3 => {
            if let Some(card) = row.get("card").and_then(Value::as_object) {
                let g = |k: &str| lossy(card.get(k));
                cards.push(AutofillCard {
                    id: id.clone(),
                    name: name.clone(),
                    cardholder_name: g("cardholderName"),
                    brand: g("brand"),
                    exp_month: g("expMonth"),
                    exp_year: g("expYear"),
                    number: g("number"),
                    code: g("code"),
                });
            }
        }
        4 => {
            if let Some(who) = row.get("identity").and_then(Value::as_object) {
                let g = |k: &str| lossy(who.get(k));
                identities.push(AutofillIdentity {
                    id: id.clone(),
                    name: name.clone(),
                    title: g("title"),
                    first_name: g("firstName"),
                    middle_name: g("middleName"),
                    last_name: g("lastName"),
                    username: g("username"),
                    company: g("company"),
                    email: g("email"),
                    phone: g("phone"),
                    address1: g("address1"),
                    address2: g("address2"),
                    address3: g("address3"),
                    city: g("city"),
                    state: g("state"),
                    postal_code: g("postalCode"),
                    country: g("country"),
                    ssn: g("ssn"),
                    passport_number: g("passportNumber"),
                    license_number: g("licenseNumber"),
                });
            }
        }
        _ => {}
    }
    let has_totp = secret.totp.is_some();
    let has_notes = secret.notes.is_some();
    if secret.password.is_some()
        || secret.totp.is_some()
        || secret.notes.is_some()
        || !secret.hidden.is_empty()
    {
        secrets.insert(id.clone(), secret);
    }
    Some(BwItem {
        id,
        kind,
        name,
        folder_id: row
            .get("folderId")
            .and_then(Value::as_str)
            .map(str::to_string),
        username: text(login, "username").unwrap_or_default(),
        uris,
        has_totp,
        has_notes,
        fields,
    })
}

fn credential(item: &BwItem, folders: &HashMap<String, String>) -> Credential {
    let folder = item
        .folder_id
        .as_ref()
        .and_then(|f| folders.get(f))
        .cloned();
    let denied = item.fields.iter().any(|f| {
        f.name.trim().eq_ignore_ascii_case("copper-agent")
            && f.value.trim().eq_ignore_ascii_case("deny")
    });
    let agent_hint = if denied {
        AgentHint::Deny
    } else if folder
        .as_deref()
        .is_some_and(|f| f.trim().eq_ignore_ascii_case("agents"))
    {
        AgentHint::Allow
    } else {
        AgentHint::None
    };
    let host = item
        .uris
        .first()
        .and_then(|u| uri_host(&u.uri))
        .unwrap_or_else(|| item.name.clone());
    Credential {
        id: CredentialId::Bitwarden(item.id.clone()),
        source: Source::Bitwarden,
        host,
        user: item.username.clone(),
        sites: item.uris.iter().map(|u| u.uri.clone()).collect(),
        name: item.name.clone(),
        has_totp: item.has_totp,
        used: None,
        folder,
        agent_hint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_order() {
        let dir = std::env::temp_dir().join(format!("bw-locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        let bin = dir.join("bin");
        let custom = dir.join("custom/bw");
        for p in [home.join(".local/bin/bw"), bin.join("bw"), custom.clone()] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "#!/bin/sh\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path_var = bin.to_string_lossy().into_owned();
        let c = custom.to_string_lossy().into_owned();
        assert_eq!(
            locate(Some(&c), Some(&home), Some(&path_var)),
            Some(custom.clone())
        );
        assert_eq!(
            locate(None, Some(&home), Some(&path_var)),
            Some(home.join(".local/bin/bw"))
        );
        // A COPPER_BW_PATH that isn't executable falls through.
        assert_eq!(
            locate(
                Some("/nope/bw"),
                Some(&dir.join("elsewhere")),
                Some(&path_var)
            ),
            Some(bin.join("bw"))
        );
        assert_eq!(locate(None, None, None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn messages() {
        assert_eq!(
            unlock_message("Cryptography error, The decryption operation failed"),
            "Wrong master password — try again"
        );
        assert!(is_log_line("ERROR bitwarden_crypto::x: y"));
        assert!(!is_log_line("Invalid master password."));
    }
}
