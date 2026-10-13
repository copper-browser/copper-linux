//! Every place a saved sign-in can live, behind one handle the main loop
//! uses: the Secret Service ([`SecretStore`]), Bitwarden (`bw`) and
//! 1Password (`op`). All of it runs on the lane's Tokio runtime
//! ([`super::rt`]); the main loop reads a [`Snapshot`] and awaits answers.
//!
//! Secrets leave here only as the return value of [`Vault::secret`] /
//! [`Vault::totp`], which the caller puts straight into the page's fill
//! script. Nothing here logs one.

use copper_creds::bitwarden::{BwConfig, BwState};
use copper_creds::credential::{
    self, AutofillCard, AutofillIdentity, Credential, CredentialId, Source,
};
use copper_creds::onepassword::{Mode, OnePassword, OpConfig, OpState, Remembered};
use copper_creds::store::{SecretStore, StoreError};
use copper_creds::{Login, bitwarden::Bitwarden};
use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

/// Whether the Secret Service answered.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Keyring {
    #[default]
    Unknown,
    Ready,
    /// No Secret Service (or it never answered): why, in one line.
    Unavailable(String),
    Locked,
}

/// What the UI shows without waiting on anything.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub keyring: Keyring,
    pub bw_state: Option<BwState>,
    /// (logins, identities, cards)
    pub bw_counts: (usize, usize, usize),
    pub bw_stay: bool,
    pub bw_server: Option<String>,
    pub op_state: Option<OpState>,
    pub op_mode: Option<Mode>,
    pub op_account: Option<String>,
    pub op_accounts: Vec<String>,
    pub op_counts: (usize, usize, usize),
    pub op_problem: Option<String>,
    pub op_save_vault: Option<String>,
    pub op_remembered: Remembered,
}

impl Snapshot {
    pub fn bw_unlocked(&self) -> bool {
        matches!(self.bw_state, Some(BwState::Unlocked { .. }))
    }

    pub fn bw_locked(&self) -> bool {
        matches!(self.bw_state, Some(BwState::Locked { .. }))
    }

    pub fn op_unlocked(&self) -> bool {
        self.op_state == Some(OpState::Unlocked)
    }

    pub fn op_locked(&self) -> bool {
        self.op_state == Some(OpState::Locked)
    }

    pub fn keyring_ready(&self) -> bool {
        self.keyring == Keyring::Ready
    }
}

pub struct Vault {
    pub bw: Mutex<Bitwarden>,
    pub op: Mutex<OnePassword>,
    store: Mutex<Option<Arc<SecretStore>>>,
    snap: StdMutex<Snapshot>,
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Vault {
    pub fn new(data_dir: &Path, bw_stay: bool, op_stay: bool) -> Arc<Vault> {
        let bw = Bitwarden::new(BwConfig::in_data_dir(data_dir), bw_stay);
        let op = OnePassword::new(OpConfig::in_data_dir(data_dir), op_stay);
        Arc::new(Vault {
            bw: Mutex::new(bw),
            op: Mutex::new(op),
            store: Mutex::new(None),
            snap: StdMutex::new(Snapshot::default()),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snap.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set_keyring(&self, k: Keyring) {
        if let Ok(mut s) = self.snap.lock() {
            s.keyring = k;
        }
    }

    /// The Secret Service, connected once (a failure is retried next time).
    pub async fn store(&self) -> Result<Arc<SecretStore>, StoreError> {
        let mut slot = self.store.lock().await;
        if let Some(s) = slot.as_ref() {
            return Ok(s.clone());
        }
        match SecretStore::open().await {
            Ok(s) => {
                let s = Arc::new(s);
                *slot = Some(s.clone());
                self.set_keyring(Keyring::Ready);
                Ok(s)
            }
            Err(e) => {
                self.set_keyring(match &e {
                    StoreError::Locked => Keyring::Locked,
                    other => Keyring::Unavailable(other.to_string()),
                });
                Err(e)
            }
        }
    }

    /// First look: the keyring, then whatever last launch left of the vaults.
    pub async fn start(&self, remembered: Remembered) {
        let _ = self.store().await;
        self.bw.lock().await.start().await;
        self.op.lock().await.start(remembered).await;
        self.refresh().await;
    }

    /// Re-read the vaults' state into the snapshot.
    pub async fn refresh(&self) {
        let (bw_state, bw_counts, bw_stay, bw_server) = {
            let bw = self.bw.lock().await;
            let (l, i, c, _) = bw.counts();
            (
                bw.state().clone(),
                (l, i, c),
                bw.stay_unlocked(),
                bw.cli_server().map(str::to_string),
            )
        };
        let (op_state, op_mode, op_account, op_accounts, op_counts, op_problem, save, rem) = {
            let op = self.op.lock().await;
            let items = op.items();
            let logins = items.iter().filter(|i| i.is_login()).count();
            (
                op.state.clone(),
                op.mode(),
                op.account().map(|a| {
                    if a.email.is_empty() {
                        a.host()
                    } else {
                        format!("{} · {}", a.email, a.host())
                    }
                }),
                op.accounts()
                    .iter()
                    .map(|a| format!("{} · {}", a.email, a.host()))
                    .collect(),
                (logins, op.identities().len(), op.cards().len()),
                op.problem().map(|p| p.message.clone()),
                op.save_vault().map(|v| v.name.clone()),
                op.remembered(),
            )
        };
        if let Ok(mut s) = self.snap.lock() {
            s.bw_state = Some(bw_state);
            s.bw_counts = bw_counts;
            s.bw_stay = bw_stay;
            s.bw_server = bw_server;
            s.op_state = Some(op_state);
            s.op_mode = op_mode;
            s.op_account = op_account;
            s.op_accounts = op_accounts;
            s.op_counts = op_counts;
            s.op_problem = op_problem;
            s.op_save_vault = save;
            s.op_remembered = rem;
        }
    }

    // MARK: reading

    /// Every source's accounts for `host`, merged (a vault's copy wins).
    pub async fn candidates(&self, host: &str) -> Vec<Credential> {
        let keyring = match self.store().await {
            Ok(s) => s
                .logins_matching(host)
                .await
                .map(|v| v.iter().map(Login::credential).collect())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let bw = self.bw.lock().await.credentials_for(host);
        let op = self.op.lock().await.credentials_for(host);
        credential::combine(keyring, bw, op)
    }

    /// Every account in every source (agent access lists).
    pub async fn all_credentials(&self) -> Vec<Credential> {
        let keyring = match self.store().await {
            Ok(s) => s
                .all()
                .await
                .map(|v| v.iter().map(Login::credential).collect())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let bw = self.bw.lock().await.credentials();
        let op = self.op.lock().await.credentials();
        credential::combine(keyring, bw, op)
    }

    /// The Secret Service's own list (Settings).
    pub async fn logins(&self) -> Result<Vec<Login>, String> {
        let s = self.store().await.map_err(|e| e.to_string())?;
        s.all().await.map_err(|e| e.to_string())
    }

    /// One account's password.
    pub async fn secret(&self, id: &CredentialId) -> Result<String, String> {
        match id {
            CredentialId::Keychain { host, user } => {
                let s = self.store().await.map_err(|e| e.to_string())?;
                s.secret(host, user)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "That password is no longer in the keyring".to_string())
            }
            CredentialId::Bitwarden(i) => self
                .bw
                .lock()
                .await
                .password(i)
                .await
                .map_err(|e| e.to_string()),
            CredentialId::OnePassword(i) => self
                .op
                .lock()
                .await
                .password(i)
                .await
                .map_err(|e| e.to_string()),
        }
    }

    /// One account's current one-time code.
    pub async fn totp(&self, id: &CredentialId) -> Result<String, String> {
        match id {
            CredentialId::Keychain { .. } => Err("no one-time code for this account".into()),
            CredentialId::Bitwarden(i) => self
                .bw
                .lock()
                .await
                .totp(i)
                .await
                .map_err(|e| e.to_string()),
            CredentialId::OnePassword(i) => self
                .op
                .lock()
                .await
                .totp(i)
                .await
                .map_err(|e| e.to_string()),
        }
    }

    /// It was just used: lists put it first.
    pub async fn touch(&self, c: &Credential) {
        if let CredentialId::Keychain { host, user } = &c.id
            && let Ok(s) = self.store().await
        {
            let _ = s.touch(host, user, now()).await;
        }
    }

    pub async fn cards(&self) -> Vec<AutofillCard> {
        let mut v = self.bw.lock().await.cards().to_vec();
        let op = self.op.lock().await;
        if op.is_unlocked() {
            v.extend(op.cards());
        }
        v
    }

    pub async fn identities(&self) -> Vec<AutofillIdentity> {
        let mut v = self.bw.lock().await.identities().to_vec();
        let op = self.op.lock().await;
        if op.is_unlocked() {
            v.extend(op.identities());
        }
        v
    }

    /// Custom fields of the vault items for `host` whose name matches
    /// `label`: `(stable item id, item name, field name, value)`.
    pub async fn fields(&self, host: &str, label: &str) -> Vec<(String, String, String, String)> {
        let norm = |s: &str| -> String {
            s.to_lowercase()
                .chars()
                .filter(|c| !c.is_whitespace() && !matches!(c, '-' | '_' | ':' | '*'))
                .collect()
        };
        let wanted = norm(label);
        if wanted.chars().count() < 3 {
            return Vec::new();
        }
        let fits = |name: &str| {
            let c = norm(name);
            c.chars().count() >= 3 && (c == wanted || c.contains(&wanted) || wanted.contains(&c))
        };
        let mut out = Vec::new();
        {
            let bw = self.bw.lock().await;
            for item in bw.items_matching(host) {
                for f in &item.fields {
                    if !f.name.is_empty() && fits(&f.name) {
                        let value = bw.field_value(&item.id, &f.name).unwrap_or_default();
                        out.push((
                            format!("bw:{}", item.id),
                            item.name.clone(),
                            f.name.clone(),
                            value,
                        ));
                    }
                }
            }
        }
        {
            let op = self.op.lock().await;
            for item in op.items_matching(host) {
                for f in &item.fields {
                    if !f.name.is_empty() && fits(&f.name) {
                        let value = op
                            .field_value(&item.id, &f.name)
                            .unwrap_or_else(|| f.value.clone());
                        out.push((
                            format!("op:{}", item.id),
                            item.title.clone(),
                            f.name.clone(),
                            value,
                        ));
                    }
                }
            }
        }
        out
    }

    // MARK: saving

    /// The users already kept for `host` where a save would go.
    pub async fn known_users(&self, target: Source, host: &str) -> Vec<String> {
        match target {
            Source::Keychain => match self.store().await {
                Ok(s) => s
                    .logins_for(host)
                    .await
                    .map(|v| v.into_iter().map(|l| l.user).collect())
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            },
            Source::Bitwarden => self
                .bw
                .lock()
                .await
                .items_matching(host)
                .iter()
                .map(|i| i.username.clone())
                .collect(),
            Source::OnePassword => self
                .op
                .lock()
                .await
                .items_matching(host)
                .iter()
                .map(|i| i.username.clone())
                .collect(),
        }
    }

    /// The kept password for one account in `target` (`Err` = unreadable,
    /// the shape [`copper_creds::offer::after_compare`] takes).
    #[allow(clippy::result_unit_err)]
    pub async fn kept(&self, target: Source, host: &str, user: &str) -> Result<Option<String>, ()> {
        match target {
            Source::Keychain => {
                let s = self.store().await.map_err(|_| ())?;
                s.secret(host, user).await.map_err(|_| ())
            }
            Source::Bitwarden => {
                let id = self
                    .bw
                    .lock()
                    .await
                    .items_matching(host)
                    .iter()
                    .find(|i| i.username == user)
                    .map(|i| i.id.clone());
                match id {
                    Some(id) => self
                        .bw
                        .lock()
                        .await
                        .password(&id)
                        .await
                        .map(Some)
                        .map_err(|_| ()),
                    None => Ok(None),
                }
            }
            Source::OnePassword => {
                let id = self
                    .op
                    .lock()
                    .await
                    .items_matching(host)
                    .iter()
                    .find(|i| i.username == user)
                    .map(|i| i.id.clone());
                match id {
                    Some(id) => self
                        .op
                        .lock()
                        .await
                        .password(&id)
                        .await
                        .map(Some)
                        .map_err(|_| ()),
                    None => Ok(None),
                }
            }
        }
    }

    /// Keep a sign-in where `target` says; the same account is updated.
    pub async fn save(
        &self,
        target: Source,
        host: &str,
        user: &str,
        password: &str,
    ) -> Result<(), String> {
        match target {
            Source::Keychain => {
                let s = self.store().await.map_err(|e| e.to_string())?;
                s.save(host, user, password, Some(now()))
                    .await
                    .map_err(|e| e.to_string())
            }
            Source::Bitwarden => {
                let mut bw = self.bw.lock().await;
                let id = bw
                    .items_matching(host)
                    .iter()
                    .find(|i| i.username == user)
                    .map(|i| i.id.clone());
                let r = match id {
                    Some(id) => bw.update(&id, password).await,
                    None => bw.create(host, user, password).await.map(|_| ()),
                };
                drop(bw);
                self.refresh().await;
                r.map_err(|e| e.to_string())
            }
            Source::OnePassword => {
                let mut op = self.op.lock().await;
                let id = op
                    .items_matching(host)
                    .iter()
                    .find(|i| i.username == user)
                    .map(|i| i.id.clone());
                let r = match id {
                    Some(id) => op.update(&id, password).await,
                    None => op.create(host, user, password).await.map(|_| ()),
                };
                drop(op);
                self.refresh().await;
                r.map_err(|e| e.to_string())
            }
        }
    }

    /// Imported passwords, kept as they were last used.
    pub async fn save_imported(
        &self,
        logins: Vec<(String, String, String, Option<f64>)>,
    ) -> Result<usize, String> {
        let s = self.store().await.map_err(|e| e.to_string())?;
        let mut n = 0;
        for (host, user, password, used) in logins {
            if s.save(&host, &user, &password, used).await.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    pub async fn forget(&self, host: &str, user: &str) -> Result<(), String> {
        let s = self.store().await.map_err(|e| e.to_string())?;
        s.forget(host, user).await.map_err(|e| e.to_string())
    }
}
