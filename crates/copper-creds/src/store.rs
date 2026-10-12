//! Where passwords live on Linux: the Secret Service (GNOME Keyring,
//! KWallet's bridge, KeePassXC…) through `oo7`, the role the macOS keychain
//! plays in `Vault.swift`.
//!
//! Every item of ours carries `app=Copper` (a test store uses its own
//! value), `origin=<host>` and `username=<user>`; when it was last used rides
//! in `used` (Unix seconds), as the keychain's comment does on macOS. Lists
//! read attributes only; a secret is read for one account at a time.
//! Nothing here logs a secret.

use std::collections::HashMap;

use oo7::{Item, Keyring};

use crate::credential::Login;
use crate::site::registrable;

/// The `app` attribute every item of Copper's carries.
pub const APP: &str = "Copper";
/// What a Secret Service browser shows as the item's schema.
pub const SCHEMA: &str = "com.copperbrowser.Copper.Login";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No Secret Service on the session bus (a bare window manager): passwords
    /// are off, and nothing is written anywhere else.
    #[error("no Secret Service is running: {0}")]
    Unavailable(String),
    #[error("the keyring is locked")]
    Locked,
    #[error("Secret Service: {0}")]
    Service(String),
    #[error("nothing to save without a site and a password")]
    Empty,
}

/// The default collection, or `Unavailable` within ten seconds: a session
/// whose Secret Service has no default collection would otherwise wait on a
/// creation prompt nobody answers.
async fn connect() -> Result<Keyring, StoreError> {
    match tokio::time::timeout(std::time::Duration::from_secs(10), Keyring::new()).await {
        Ok(Ok(k)) => Ok(k),
        Ok(Err(e)) => Err(StoreError::Unavailable(e.to_string())),
        Err(_) => Err(StoreError::Unavailable(
            "the Secret Service did not answer (no default keyring?)".into(),
        )),
    }
}

fn service(e: oo7::Error) -> StoreError {
    StoreError::Service(e.to_string())
}

/// The password store.
pub struct SecretStore {
    keyring: Keyring,
    app: String,
}

impl SecretStore {
    /// The default collection of the Secret Service (in a Flatpak, the
    /// portal's file keyring), unlocked on demand.
    pub async fn open() -> Result<Self, StoreError> {
        Self::open_tagged(APP).await
    }

    /// The same, with `app` set to `tag` — a test run tags its own items so
    /// they never sit among the real ones (macOS: `Copper (world)`).
    pub async fn open_tagged(tag: &str) -> Result<Self, StoreError> {
        let keyring = connect().await?;
        Ok(SecretStore {
            keyring,
            app: tag.to_string(),
        })
    }

    /// The `app` attribute this store reads and writes.
    pub fn tag(&self) -> &str {
        &self.app
    }

    async fn ready(&self) -> Result<(), StoreError> {
        if self.keyring.is_locked().await.map_err(service)? {
            self.keyring
                .unlock()
                .await
                .map_err(|_| StoreError::Locked)?;
        }
        Ok(())
    }

    fn attrs<'a>(&'a self, pairs: &[(&'a str, &'a str)]) -> HashMap<&'a str, &'a str> {
        let mut map: HashMap<&str, &str> = pairs.iter().copied().collect();
        map.insert("app", self.app.as_str());
        map
    }

    async fn items(&self, pairs: &[(&str, &str)]) -> Result<Vec<Item>, StoreError> {
        self.ready().await?;
        self.keyring
            .search_items(&self.attrs(pairs))
            .await
            .map_err(service)
    }

    async fn login(item: &Item) -> Option<Login> {
        let attrs = item.attributes().await.ok()?;
        let host = attrs.get("origin")?.clone();
        let user = attrs.get("username").cloned().unwrap_or_default();
        let used = attrs.get("used").and_then(|u| u.parse::<f64>().ok());
        Some(Login { host, user, used })
    }

    async fn logins(&self, pairs: &[(&str, &str)]) -> Result<Vec<Login>, StoreError> {
        let mut out = Vec::new();
        for item in self.items(pairs).await? {
            if let Some(login) = Self::login(&item).await {
                out.push(login);
            }
        }
        Ok(out)
    }

    /// What is kept for exactly this origin.
    pub async fn logins_for(&self, host: &str) -> Result<Vec<Login>, StoreError> {
        self.logins(&[("origin", host)]).await
    }

    /// The site matched as a site: this host, then anything sharing its
    /// registrable domain; most recently used first.
    pub async fn logins_matching(&self, host: &str) -> Result<Vec<Login>, StoreError> {
        let domain = registrable(host);
        let mut exact = self.logins_for(host).await?;
        let wider = self
            .all()
            .await?
            .into_iter()
            .filter(|l| l.host != host && registrable(&l.host) == domain);
        exact.extend(wider);
        exact.sort_by(|a, b| {
            b.used
                .unwrap_or(f64::MIN)
                .partial_cmp(&a.used.unwrap_or(f64::MIN))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(exact)
    }

    /// Everything this store holds, by host then user.
    pub async fn all(&self) -> Result<Vec<Login>, StoreError> {
        let mut all = self.logins(&[]).await?;
        all.sort_by(|a, b| a.host.cmp(&b.host).then(a.user.cmp(&b.user)));
        Ok(all)
    }

    async fn one(&self, host: &str, user: &str) -> Result<Option<Item>, StoreError> {
        Ok(self
            .items(&[("origin", host), ("username", user)])
            .await?
            .into_iter()
            .next())
    }

    /// One account's password, read for that account only.
    pub async fn secret(&self, host: &str, user: &str) -> Result<Option<String>, StoreError> {
        let Some(item) = self.one(host, user).await? else {
            return Ok(None);
        };
        if item.is_locked().await.map_err(service)? {
            item.unlock().await.map_err(|_| StoreError::Locked)?;
        }
        let secret = item.secret().await.map_err(service)?;
        Ok(String::from_utf8(secret.as_bytes().to_vec()).ok())
    }

    /// Keep a password: the same origin + user is updated in place, anything
    /// else is a new item. `used` (Unix seconds) is kept when given.
    pub async fn save(
        &self,
        host: &str,
        user: &str,
        password: &str,
        used: Option<f64>,
    ) -> Result<(), StoreError> {
        if host.is_empty() || password.is_empty() {
            return Err(StoreError::Empty);
        }
        let used_text = used.map(|u| format!("{u}"));
        let mut pairs = vec![("origin", host), ("username", user), ("xdg:schema", SCHEMA)];
        if let Some(u) = &used_text {
            pairs.push(("used", u.as_str()));
        }
        let attrs = self.attrs(&pairs);
        if let Some(item) = self.one(host, user).await? {
            if used_text.is_none() {
                // Keep the old `used` when nothing new is said.
                if let Some(old) = Self::login(&item).await.and_then(|l| l.used) {
                    let old = format!("{old}");
                    let mut keep = attrs.clone();
                    keep.insert("used", old.as_str());
                    item.set_attributes(&keep).await.map_err(service)?;
                    item.set_secret(password).await.map_err(service)?;
                    return Ok(());
                }
            }
            item.set_attributes(&attrs).await.map_err(service)?;
            item.set_secret(password).await.map_err(service)?;
            return Ok(());
        }
        self.ready().await?;
        self.keyring
            .create_item(&label(host, user), &attrs, password, true)
            .await
            .map_err(service)
    }

    /// It was just used to sign in: lists put it first from now on. Only the
    /// attribute moves; the secret is neither read nor rewritten.
    pub async fn touch(&self, host: &str, user: &str, now: f64) -> Result<(), StoreError> {
        let Some(item) = self.one(host, user).await? else {
            return Ok(());
        };
        let mut attrs = item.attributes().await.map_err(service)?;
        attrs.insert("used".into(), format!("{now}"));
        item.set_attributes(&attrs).await.map_err(service)
    }

    /// Forget one account.
    pub async fn forget(&self, host: &str, user: &str) -> Result<(), StoreError> {
        for item in self.items(&[("origin", host), ("username", user)]).await? {
            item.delete().await.map_err(service)?;
        }
        Ok(())
    }

    /// Forget everything kept for one origin.
    pub async fn forget_origin(&self, host: &str) -> Result<usize, StoreError> {
        let items = self.items(&[("origin", host)]).await?;
        let n = items.len();
        for item in items {
            item.delete().await.map_err(service)?;
        }
        Ok(n)
    }
}

fn label(host: &str, user: &str) -> String {
    if user.is_empty() {
        format!("{host} (Copper)")
    } else {
        format!("{user} on {host} (Copper)")
    }
}

/// Any Secret Service item's secret by its attributes, for reading another
/// application's own key (Chrome's "Chrome Safe Storage"). `None` when there
/// is no such item.
pub async fn lookup(attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, StoreError> {
    let keyring = connect().await?;
    if keyring.is_locked().await.map_err(service)? {
        keyring.unlock().await.map_err(|_| StoreError::Locked)?;
    }
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    let items = keyring.search_items(&attrs).await.map_err(service)?;
    let Some(item) = items.into_iter().next() else {
        return Ok(None);
    };
    if item.is_locked().await.map_err(service)? {
        item.unlock().await.map_err(|_| StoreError::Locked)?;
    }
    Ok(Some(
        item.secret().await.map_err(service)?.as_bytes().to_vec(),
    ))
}

/// Put an arbitrary item in the default collection (tests and tools that
/// stand in for another application's key).
pub async fn store_raw(
    label: &str,
    attributes: &[(&str, &str)],
    secret: &[u8],
) -> Result<(), StoreError> {
    let keyring = connect().await?;
    if keyring.is_locked().await.map_err(service)? {
        keyring.unlock().await.map_err(|_| StoreError::Locked)?;
    }
    let attrs: HashMap<&str, &str> = attributes.iter().copied().collect();
    keyring
        .create_item(label, &attrs, secret.to_vec(), true)
        .await
        .map_err(service)
}
