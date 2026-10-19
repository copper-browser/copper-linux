//! Where passwords live: the platform's own secret store, the role the macOS
//! keychain plays in `Vault.swift`.
//!
//! - Linux: the Secret Service (GNOME Keyring, KWallet's bridge,
//!   KeePassXC…) through `oo7` ([`secret_service`]).
//! - Windows: Credential Manager, with DPAPI-sealed files for secrets too big
//!   for it ([`wincred`]).
//!
//! Every login of ours carries `app=Copper` (a test store uses its own value),
//! `origin=<host>` and `username=<user>`; when it was last used rides in
//! `used` (Unix seconds), as the keychain's comment does on macOS. Lists read
//! attributes only; a secret is read for one account at a time. Nothing here
//! logs a secret.
//!
//! [`SecretStore`] is the same API on every platform: the backend supplies
//! the per-item operations, this module the site matching and ordering.

#[cfg(target_os = "linux")]
mod secret_service;
#[cfg(target_os = "linux")]
use secret_service as backend;

#[cfg(windows)]
mod wincred;
#[cfg(windows)]
use wincred as backend;

#[cfg(not(any(target_os = "linux", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", windows)))]
use unsupported as backend;

/// What Credential Manager target names look like and how they read back.
/// Platform-free so it is tested everywhere; only Windows uses it.
#[cfg(any(windows, test))]
mod target;

use crate::credential::Login;
use crate::site::registrable;

/// The `app` attribute every item of Copper's carries.
pub const APP: &str = "Copper";
/// What a Secret Service browser shows as the item's schema.
pub const SCHEMA: &str = "com.copperbrowser.Copper.Login";

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No secret store to talk to — no Secret Service on the session bus (a
    /// bare window manager), or no Credential Manager for this logon (a
    /// service or network logon): passwords are off, and nothing is written
    /// anywhere else.
    #[cfg_attr(not(windows), error("no Secret Service is running: {0}"))]
    #[cfg_attr(windows, error("Credential Manager is unavailable: {0}"))]
    Unavailable(String),
    /// The keyring would not unlock (never on Windows: Credential Manager
    /// is open whenever the user is signed in).
    #[error("the keyring is locked")]
    Locked,
    #[cfg_attr(not(windows), error("Secret Service: {0}"))]
    #[cfg_attr(windows, error("Credential Manager: {0}"))]
    Service(String),
    #[error("nothing to save without a site and a password")]
    Empty,
}

/// The password store.
pub struct SecretStore {
    inner: backend::Store,
}

impl SecretStore {
    /// The default collection of the Secret Service (in a Flatpak, the
    /// portal's file keyring), unlocked on demand; on Windows, the user's
    /// Credential Manager.
    pub async fn open() -> Result<Self, StoreError> {
        Self::open_tagged(APP).await
    }

    /// The same, with `app` set to `tag` — a test run tags its own items so
    /// they never sit among the real ones (macOS: `Copper (world)`).
    pub async fn open_tagged(tag: &str) -> Result<Self, StoreError> {
        Ok(SecretStore {
            inner: backend::Store::open(tag).await?,
        })
    }

    /// The `app` attribute this store reads and writes.
    pub fn tag(&self) -> &str {
        self.inner.tag()
    }

    /// What is kept for exactly this origin.
    pub async fn logins_for(&self, host: &str) -> Result<Vec<Login>, StoreError> {
        self.inner.logins(Some(host)).await
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
        let mut all = self.inner.logins(None).await?;
        all.sort_by(|a, b| a.host.cmp(&b.host).then(a.user.cmp(&b.user)));
        Ok(all)
    }

    /// One account's password, read for that account only.
    pub async fn secret(&self, host: &str, user: &str) -> Result<Option<String>, StoreError> {
        self.inner.secret(host, user).await
    }

    /// Keep a password: the same origin + user is updated in place, anything
    /// else is a new item. `used` (Unix seconds) is kept when given; when
    /// not, an existing item keeps its old `used`.
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
        self.inner.save(host, user, password, used).await
    }

    /// It was just used to sign in: lists put it first from now on. Only the
    /// attribute moves; the secret is neither read nor rewritten.
    pub async fn touch(&self, host: &str, user: &str, now: f64) -> Result<(), StoreError> {
        self.inner.touch(host, user, now).await
    }

    /// Forget one account.
    pub async fn forget(&self, host: &str, user: &str) -> Result<(), StoreError> {
        self.inner.forget(host, user).await
    }

    /// Forget everything kept for one origin.
    pub async fn forget_origin(&self, host: &str) -> Result<usize, StoreError> {
        self.inner.forget_origin(host).await
    }
}

/// What a store browser (Seahorse, Credential Manager) shows for a login.
pub(crate) fn label(host: &str, user: &str) -> String {
    if user.is_empty() {
        format!("{host} (Copper)")
    } else {
        format!("{user} on {host} (Copper)")
    }
}

/// `used` as it is written into an attribute (shortest round-trip form).
#[cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
pub(crate) fn used_text(used: f64) -> String {
    format!("{used}")
}

/// Any stored item's secret by its attributes, for reading another
/// application's own key (Chrome's "Chrome Safe Storage" on Linux). `None`
/// when there is no such item. On Windows only items Copper wrote are
/// searched (Credential Manager has no attribute search of its own).
pub async fn lookup(attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, StoreError> {
    backend::lookup(attributes).await
}

/// Put an arbitrary item in the store (tests and tools that stand in for
/// another application's key). An item with exactly these attributes is
/// replaced.
pub async fn store_raw(
    label: &str,
    attributes: &[(&str, &str)],
    secret: &[u8],
) -> Result<(), StoreError> {
    backend::store_raw(label, attributes, secret).await
}
