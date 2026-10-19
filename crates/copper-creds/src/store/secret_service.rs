//! The Linux backend: the Secret Service through `oo7`. Items carry
//! `app`, `origin`, `username`, `xdg:schema` and (when known) `used`.

use std::collections::HashMap;

use oo7::{Item, Keyring};

use super::{SCHEMA, StoreError, label, used_text};
use crate::credential::Login;

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

/// One tag's view of the default collection.
pub(super) struct Store {
    keyring: Keyring,
    app: String,
}

impl Store {
    pub(super) async fn open(tag: &str) -> Result<Self, StoreError> {
        let keyring = connect().await?;
        Ok(Store {
            keyring,
            app: tag.to_string(),
        })
    }

    pub(super) fn tag(&self) -> &str {
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

    /// Logins for `origin`, or every login of this tag.
    pub(super) async fn logins(&self, origin: Option<&str>) -> Result<Vec<Login>, StoreError> {
        let pairs: Vec<(&str, &str)> = origin.map(|o| ("origin", o)).into_iter().collect();
        let mut out = Vec::new();
        for item in self.items(&pairs).await? {
            if let Some(login) = Self::login(&item).await {
                out.push(login);
            }
        }
        Ok(out)
    }

    async fn one(&self, host: &str, user: &str) -> Result<Option<Item>, StoreError> {
        Ok(self
            .items(&[("origin", host), ("username", user)])
            .await?
            .into_iter()
            .next())
    }

    pub(super) async fn secret(
        &self,
        host: &str,
        user: &str,
    ) -> Result<Option<String>, StoreError> {
        let Some(item) = self.one(host, user).await? else {
            return Ok(None);
        };
        if item.is_locked().await.map_err(service)? {
            item.unlock().await.map_err(|_| StoreError::Locked)?;
        }
        let secret = item.secret().await.map_err(service)?;
        Ok(String::from_utf8(secret.as_bytes().to_vec()).ok())
    }

    pub(super) async fn save(
        &self,
        host: &str,
        user: &str,
        password: &str,
        used: Option<f64>,
    ) -> Result<(), StoreError> {
        let used_attr = used.map(used_text);
        let mut pairs = vec![("origin", host), ("username", user), ("xdg:schema", SCHEMA)];
        if let Some(u) = &used_attr {
            pairs.push(("used", u.as_str()));
        }
        let attrs = self.attrs(&pairs);
        if let Some(item) = self.one(host, user).await? {
            if used_attr.is_none() {
                // Keep the old `used` when nothing new is said.
                if let Some(old) = Self::login(&item).await.and_then(|l| l.used) {
                    let old = used_text(old);
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

    pub(super) async fn touch(&self, host: &str, user: &str, now: f64) -> Result<(), StoreError> {
        let Some(item) = self.one(host, user).await? else {
            return Ok(());
        };
        let mut attrs = item.attributes().await.map_err(service)?;
        attrs.insert("used".into(), used_text(now));
        item.set_attributes(&attrs).await.map_err(service)
    }

    pub(super) async fn forget(&self, host: &str, user: &str) -> Result<(), StoreError> {
        for item in self.items(&[("origin", host), ("username", user)]).await? {
            item.delete().await.map_err(service)?;
        }
        Ok(())
    }

    pub(super) async fn forget_origin(&self, host: &str) -> Result<usize, StoreError> {
        let items = self.items(&[("origin", host)]).await?;
        let n = items.len();
        for item in items {
            item.delete().await.map_err(service)?;
        }
        Ok(n)
    }
}

pub(super) async fn lookup(attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, StoreError> {
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

pub(super) async fn store_raw(
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
