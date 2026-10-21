//! BotPass as one of Copper's password sources ([`BotPass`]), the way the
//! forms lane's `Vault` uses Bitwarden and 1Password.
//!
//! Nothing is cached in bulk: the accounts for a page are listed when a
//! sign-in box is clicked (or an agent asks), and a secret is fetched only
//! when one is filled, for the page's origin at that moment. BotPass checks
//! its grants on both calls, so a phishing origin is offered nothing and
//! gets nothing. `botpass.json` is re-read when it changes, so
//! `copper botpass pair` (or `bp connect copper`) takes effect in a running
//! Copper without a restart.

use crate::client::{Account, Server, What};
use crate::error::Error;
use crate::pairing::{self, Pairing};
use copper_creds::credential::{AgentHint, Credential, CredentialId, Source};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// What Copper last learned from BotPass.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BpState {
    /// No `botpass.json`.
    #[default]
    Unpaired,
    /// Paired; nothing asked yet.
    Paired,
    /// The last call worked.
    Ready,
    /// The server is sealed (`bp unlock`).
    Locked,
    /// The token was refused (the device was removed in BotPass).
    Revoked,
    /// The last call got no answer, or the certificate didn't match.
    Unreachable(String),
}

/// What the UI shows without waiting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BpSnapshot {
    pub state: BpState,
    pub url: Option<String>,
    pub fingerprint: Option<String>,
    pub device_name: Option<String>,
    pub bot: Option<String>,
}

impl BpSnapshot {
    pub fn paired(&self) -> bool {
        self.state != BpState::Unpaired
    }

    pub fn locked(&self) -> bool {
        self.state == BpState::Locked
    }
}

struct Loaded {
    stamp: Option<SystemTime>,
    pairing: Option<(Pairing, Server)>,
}

/// BotPass, for one Copper data folder.
pub struct BotPass {
    data_dir: PathBuf,
    loaded: Mutex<Option<Loaded>>,
    state: Mutex<BpState>,
}

impl std::fmt::Debug for BotPass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BotPass")
            .field("data_dir", &self.data_dir)
            .field("state", &self.state())
            .finish()
    }
}

fn stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The account a fill names, as a picker row / agent candidate.
pub fn credential(origin_host: &str, a: &Account) -> Credential {
    let user = if a.username.is_empty() {
        a.title.clone()
    } else {
        a.username.clone()
    };
    Credential {
        id: CredentialId::BotPass(a.id.clone()),
        source: Source::BotPass,
        host: origin_host.to_string(),
        user,
        sites: a.sites.clone(),
        name: if a.title.is_empty() {
            a.username.clone()
        } else {
            a.title.clone()
        },
        has_totp: a.has_totp,
        used: None,
        folder: (!a.vault.is_empty()).then(|| a.vault.clone()),
        // BotPass's grants already decided this device may use it.
        agent_hint: AgentHint::Allow,
    }
}

impl BotPass {
    pub fn new(data_dir: &Path) -> BotPass {
        BotPass {
            data_dir: data_dir.to_path_buf(),
            loaded: Mutex::new(None),
            state: Mutex::new(BpState::Unpaired),
        }
    }

    /// `<data>/botpass.json`.
    pub fn file(&self) -> PathBuf {
        pairing::path(&self.data_dir)
    }

    /// The pairing and its server, re-read when the file changed.
    fn current(&self) -> Result<Option<(Pairing, Server)>, Error> {
        let path = self.file();
        let now = stamp(&path);
        let mut slot = self.loaded.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = slot.as_ref()
            && l.stamp == now
        {
            return Ok(l.pairing.clone());
        }
        let pairing = match pairing::load(&self.data_dir)? {
            Some(p) => {
                let server = Server::for_pairing(&p)?;
                Some((p, server))
            }
            None => None,
        };
        self.set_state(if pairing.is_some() {
            BpState::Paired
        } else {
            BpState::Unpaired
        });
        *slot = Some(Loaded {
            stamp: now,
            pairing: pairing.clone(),
        });
        Ok(pairing)
    }

    /// The pairing in use, if any.
    pub fn pairing(&self) -> Option<Pairing> {
        self.current().ok().flatten().map(|(p, _)| p)
    }

    pub fn state(&self) -> BpState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_state(&self, s: BpState) {
        if let Ok(mut st) = self.state.lock() {
            *st = s;
        }
    }

    /// Note how a call went; the error passes through.
    fn noted<T>(&self, r: Result<T, Error>) -> Result<T, Error> {
        match &r {
            Ok(_) => self.set_state(BpState::Ready),
            Err(Error::Locked) => self.set_state(BpState::Locked),
            Err(Error::Unpaired) => self.set_state(BpState::Revoked),
            Err(
                e @ (Error::Unreachable(_) | Error::Fingerprint { .. } | Error::Untrusted { .. }),
            ) => self.set_state(BpState::Unreachable(e.to_string())),
            Err(_) => {}
        }
        r
    }

    pub fn snapshot(&self) -> BpSnapshot {
        let p = self.pairing();
        BpSnapshot {
            state: self.state(),
            url: p.as_ref().map(|p| p.url.clone()),
            fingerprint: p.as_ref().and_then(|p| p.fingerprint.clone()),
            device_name: p.as_ref().map(|p| p.device_name.clone()),
            bot: p.and_then(|p| p.bot),
        }
    }

    /// Look at the server (`/v1/status`, no token): locked or not.
    pub async fn refresh(&self) {
        let Ok(Some((_, server))) = self.current() else {
            return;
        };
        match server.status().await {
            Ok(s) if s.locked => self.set_state(BpState::Locked),
            Ok(_) => {
                if matches!(
                    self.state(),
                    BpState::Locked | BpState::Unreachable(_) | BpState::Paired
                ) {
                    self.set_state(BpState::Ready)
                }
            }
            Err(e) => {
                let _ = self.noted::<()>(Err(e));
            }
        }
    }

    /// The accounts BotPass offers on `origin` (`https://host[:port]`).
    /// Unpaired: none, and no error.
    pub async fn credentials_for(&self, origin: &str) -> Result<Vec<Credential>, Error> {
        let Some((p, server)) = self.current()? else {
            return Ok(Vec::new());
        };
        let host = url::Url::parse(origin)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .ok_or_else(|| Error::Usage(format!("not a page origin: {origin}")))?;
        let accounts = self.noted(server.accounts(&p.token, origin).await)?;
        Ok(accounts.iter().map(|a| credential(&host, a)).collect())
    }

    /// One account's password, for the page at `origin`, fetched now.
    pub async fn password(&self, id: &str, origin: &str) -> Result<String, Error> {
        let (p, server) = self.current()?.ok_or(Error::Unpaired)?;
        let filled = self.noted(server.fill(&p.token, id, origin, What::Password).await)?;
        filled
            .password
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::NotFound("that account has no password".into()))
    }

    /// One account's current one-time code, for the page at `origin`.
    pub async fn totp(&self, id: &str, origin: &str) -> Result<String, Error> {
        let (p, server) = self.current()?.ok_or(Error::Unpaired)?;
        let filled = self.noted(server.fill(&p.token, id, origin, What::Otp).await)?;
        filled
            .code
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::NotFound("no one-time code for this account".into()))
    }

    /// Pair with a code (Settings › Passwords), keeping `botpass.json`.
    pub async fn pair(
        &self,
        url: &str,
        fingerprint: Option<&str>,
        code: &str,
        name: &str,
    ) -> Result<Pairing, Error> {
        let server = Server::new(url, fingerprint)?;
        let p = server.pair(code, name).await?;
        pairing::save(&self.data_dir, &p)?;
        self.forget_loaded();
        self.set_state(BpState::Ready);
        Ok(p)
    }

    /// Forget the pairing here (BotPass keeps the device until it is
    /// removed there).
    pub fn unpair(&self) -> Result<bool, Error> {
        let had = pairing::remove(&self.data_dir)?;
        self.forget_loaded();
        self.set_state(BpState::Unpaired);
        Ok(had)
    }

    fn forget_loaded(&self) {
        if let Ok(mut l) = self.loaded.lock() {
            *l = None;
        }
    }
}

/// What this Copper calls itself when it pairs: `Copper on <host>`
/// (with the `COPPER_WORLD`, if any).
pub fn default_device_name() -> String {
    let host = host_name();
    match std::env::var("COPPER_WORLD") {
        Ok(w) if !w.trim().is_empty() => format!("Copper on {host} ({})", w.trim()),
        _ => format!("Copper on {host}"),
    }
}

#[cfg(not(windows))]
fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Linux".to_string())
}

#[cfg(windows)]
fn host_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_become_allowed_credentials_without_secrets() {
        let a = Account {
            id: "it1".into(),
            title: "GitHub".into(),
            username: "ann".into(),
            sites: vec!["https://github.com".into()],
            has_totp: true,
            vault: "Agents".into(),
        };
        let c = credential("github.com", &a);
        assert_eq!(c.id, CredentialId::BotPass("it1".into()));
        assert_eq!(c.id.to_string(), "bp:it1");
        assert_eq!(c.source, Source::BotPass);
        assert_eq!(c.user, "ann");
        assert_eq!(c.name, "GitHub");
        assert!(c.has_totp);
        assert_eq!(c.agent_hint, AgentHint::Allow);
        assert_eq!(c.folder.as_deref(), Some("Agents"));
        let nameless = credential(
            "x.com",
            &Account {
                id: "i2".into(),
                title: "X".into(),
                ..Default::default()
            },
        );
        assert_eq!(nameless.user, "X");
    }

    /// The forms lane runs these on another thread (`rt::run` needs `Send`).
    #[test]
    fn its_futures_are_send() {
        fn send<T: Send>(_: &T) {}
        let bp = BotPass::new(Path::new("/nonexistent"));
        send(&bp.pair("https://h", None, "c", "n"));
        send(&bp.credentials_for("https://h"));
        send(&bp.password("i", "https://h"));
        send(&bp.totp("i", "https://h"));
        send(&bp.refresh());
    }

    #[test]
    fn unpaired_offers_nothing_and_fills_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let bp = BotPass::new(dir.path());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert!(
            rt.block_on(bp.credentials_for("https://example.com"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            rt.block_on(bp.password("x", "https://example.com")),
            Err(Error::Unpaired)
        );
        assert!(!bp.snapshot().paired());
    }
}
