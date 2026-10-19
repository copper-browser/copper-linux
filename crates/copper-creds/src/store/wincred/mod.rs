//! The Windows backend: Credential Manager (`CRED_TYPE_GENERIC`, persisted
//! per user on this machine), the place Windows apps keep passwords.
//!
//! A login is one credential named `Copper/<origin>/<user>` (see
//! [`super::target`]); its user name and a "user on host (Copper)" comment
//! are what Credential Manager shows. `app`, `origin` and `username` are read
//! from the name; `used` and `xdg:schema` ride as credential attributes, so
//! [`super::lookup`] matches the same attributes as on Linux. The secret is
//! the UTF-8 bytes of the password; one over 2560 bytes is sealed in a DPAPI
//! file and the credential carries only a pointer to it ([`vault`]).
//!
//! The calls are local and quick (no prompt, no network), so they run inline
//! in these `async fn`s rather than on a blocking pool.

mod cred;
mod vault;

use zeroize::Zeroizing;

use super::target::{
    NAMESPACE_FILTER, login_target, origin_filter, parse_login, raw_target, tag_filter, tag_prefix,
};
use super::{SCHEMA, StoreError, label, used_text};
use crate::credential::Login;
use cred::{Cred, MAX_BLOB, NewCred};
use vault::Vault;

/// When the login was last used (Unix seconds), as on Linux.
const ATTR_USED: &str = "used";
/// The schema attribute, as on Linux (lets `lookup` match it).
const ATTR_SCHEMA: &str = "xdg:schema";
/// Present when the secret is sealed in the vault: the file's name.
const ATTR_VAULT: &str = "copper:vault";
/// Attribute names Copper reserves for itself.
const RESERVED: &str = "copper:";
/// What a vault-backed credential holds instead of the secret. Only a
/// marker for people browsing Credential Manager; `ATTR_VAULT` is what is
/// trusted.
const POINTER_BLOB: &[u8] = b"(sealed in Copper's vault)";

/// One tag's logins in Credential Manager.
pub(super) struct Store {
    app: String,
    vault: Vault,
}

impl Store {
    /// Checks that this logon session has a credential store (a service or
    /// network logon has none) with one harmless read.
    pub(super) async fn open(tag: &str) -> Result<Self, StoreError> {
        Self::open_with(tag, Vault::standard())
    }

    fn open_with(tag: &str, vault: Vault) -> Result<Self, StoreError> {
        // Never a credential: no origin after the prefix.
        cred::read(&tag_prefix(tag))?;
        Ok(Store {
            app: tag.to_string(),
            vault,
        })
    }

    pub(super) fn tag(&self) -> &str {
        &self.app
    }

    /// The login a credential of this tag stands for.
    fn login(&self, c: &Cred) -> Option<Login> {
        let (tag, host, user) = parse_login(&c.target)?;
        (tag == self.app).then(|| Login {
            host,
            user,
            used: c.attribute(ATTR_USED).and_then(|u| u.parse::<f64>().ok()),
        })
    }

    pub(super) async fn logins(&self, origin: Option<&str>) -> Result<Vec<Login>, StoreError> {
        let filter = match origin {
            Some(o) => origin_filter(&self.app, o),
            None => tag_filter(&self.app),
        };
        Ok(cred::enumerate(&filter)?
            .iter()
            .filter_map(|c| self.login(c))
            .filter(|l| origin.is_none_or(|o| l.host == o))
            .collect())
    }

    pub(super) async fn secret(
        &self,
        host: &str,
        user: &str,
    ) -> Result<Option<String>, StoreError> {
        let Some(c) = cred::read(&login_target(&self.app, host, user))? else {
            return Ok(None);
        };
        let bytes = secret_of(&self.vault, &c)?;
        Ok(std::str::from_utf8(&bytes).ok().map(str::to_string))
    }

    pub(super) async fn save(
        &self,
        host: &str,
        user: &str,
        password: &str,
        used: Option<f64>,
    ) -> Result<(), StoreError> {
        let target = login_target(&self.app, host, user);
        // Keep the old `used` when nothing new is said.
        let used = match used {
            Some(u) => Some(used_text(u)),
            None => cred::read(&target)?.and_then(|c| c.attribute(ATTR_USED).map(str::to_string)),
        };
        let mut attributes = vec![(ATTR_SCHEMA.to_string(), SCHEMA.as_bytes().to_vec())];
        if let Some(u) = used {
            attributes.push((ATTR_USED.to_string(), u.into_bytes()));
        }
        let secret = Zeroizing::new(password.as_bytes().to_vec());
        put(
            &self.vault,
            &target,
            &label(host, user),
            user,
            attributes,
            &secret,
        )
    }

    pub(super) async fn touch(&self, host: &str, user: &str, now: f64) -> Result<(), StoreError> {
        let target = login_target(&self.app, host, user);
        let Some(c) = cred::read(&target)? else {
            return Ok(());
        };
        let mut attributes: Vec<(String, Vec<u8>)> = c
            .attributes
            .iter()
            .filter(|(k, _)| k != ATTR_USED)
            .cloned()
            .collect();
        attributes.push((ATTR_USED.to_string(), used_text(now).into_bytes()));
        // The blob (secret or vault pointer) stays exactly as it is.
        cred::write(&NewCred {
            target: &target,
            comment: &label(host, user),
            user_name: user,
            attributes: &attributes,
            blob: None,
        })
    }

    pub(super) async fn forget(&self, host: &str, user: &str) -> Result<(), StoreError> {
        let target = login_target(&self.app, host, user);
        cred::delete(&target)?;
        self.vault.remove(&target);
        Ok(())
    }

    pub(super) async fn forget_origin(&self, host: &str) -> Result<usize, StoreError> {
        let mut n = 0;
        for c in cred::enumerate(&origin_filter(&self.app, host))? {
            if self.login(&c).is_some_and(|l| l.host == host) {
                if cred::delete(&c.target)? {
                    n += 1;
                }
                self.vault.remove(&c.target);
            }
        }
        Ok(n)
    }
}

/// Write one credential: the secret inline when it fits, else sealed in the
/// vault with a pointer inline. A vault file left from a bigger secret
/// before goes once the credential no longer points at it.
fn put(
    vault: &Vault,
    target: &str,
    comment: &str,
    user_name: &str,
    mut attributes: Vec<(String, Vec<u8>)>,
    secret: &[u8],
) -> Result<(), StoreError> {
    attributes.retain(|(k, _)| k != ATTR_VAULT);
    if secret.len() <= MAX_BLOB {
        cred::write(&NewCred {
            target,
            comment,
            user_name,
            attributes: &attributes,
            blob: Some(secret),
        })?;
        vault.remove(target);
        return Ok(());
    }
    let file = vault.seal(target, secret)?;
    attributes.push((ATTR_VAULT.to_string(), file.into_bytes()));
    cred::write(&NewCred {
        target,
        comment,
        user_name,
        attributes: &attributes,
        blob: Some(POINTER_BLOB),
    })
}

/// A credential's secret: its blob, or what its vault file holds.
fn secret_of(vault: &Vault, c: &Cred) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    match c.attribute(ATTR_VAULT) {
        Some(file) => vault.open(&c.target, file),
        None => Ok(c.blob.clone()),
    }
}

/// Every attribute a credential answers to: the ones stored on it, plus
/// `app`/`origin`/`username` read from a login's name.
fn attributes_of(c: &Cred) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = c
        .attributes
        .iter()
        .filter(|(k, _)| !k.starts_with(RESERVED))
        .filter_map(|(k, v)| Some((k.clone(), String::from_utf8(v.clone()).ok()?)))
        .collect();
    if let Some((tag, origin, user)) = parse_login(&c.target) {
        out.push(("app".into(), tag));
        out.push(("origin".into(), origin));
        out.push(("username".into(), user));
    }
    out
}

fn matches(c: &Cred, wanted: &[(&str, &str)]) -> bool {
    let have = attributes_of(c);
    wanted
        .iter()
        .all(|(k, v)| have.iter().any(|(hk, hv)| hk == k && hv == v))
}

/// The first of Copper's credentials (any tag, logins and raw items) that
/// answers to every attribute in `attributes`. Other programs' credentials
/// are never read: Credential Manager has no attribute search, and their
/// names mean nothing here.
pub(super) async fn lookup(attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, StoreError> {
    lookup_in(&Vault::standard(), attributes)
}

fn lookup_in(vault: &Vault, attributes: &[(&str, &str)]) -> Result<Option<Vec<u8>>, StoreError> {
    let found = cred::enumerate(NAMESPACE_FILTER)?
        .into_iter()
        .find(|c| matches(c, attributes));
    match found {
        Some(c) => Ok(Some(secret_of(vault, &c)?.to_vec())),
        None => Ok(None),
    }
}

/// An arbitrary item, named after its attributes (the same attributes
/// replace it, as `create_item(…, replace)` does on Linux). Attribute names
/// starting `copper:` are Copper's own and refused; each attribute must fit
/// Credential Manager (name ≤ 256 characters, value ≤ 256 bytes, ≤ 63 of
/// them).
pub(super) async fn store_raw(
    label: &str,
    attributes: &[(&str, &str)],
    secret: &[u8],
) -> Result<(), StoreError> {
    store_raw_in(&Vault::standard(), label, attributes, secret)
}

fn store_raw_in(
    vault: &Vault,
    label: &str,
    attributes: &[(&str, &str)],
    secret: &[u8],
) -> Result<(), StoreError> {
    if attributes.iter().any(|(k, _)| k.starts_with(RESERVED)) {
        return Err(StoreError::Service(format!(
            "attribute names starting {RESERVED:?} are Copper's own"
        )));
    }
    let canonical: std::collections::BTreeMap<&str, &str> = attributes.iter().copied().collect();
    // One slot stays free for a vault pointer.
    if canonical.len() >= cred::MAX_ATTRIBUTES {
        return Err(StoreError::Service(
            "too many attributes for Credential Manager".into(),
        ));
    }
    let stored: Vec<(String, Vec<u8>)> = canonical
        .iter()
        .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
        .collect();
    put(vault, &raw_target(attributes), label, "", stored, secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("copper-wincred-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A store with its own tag and vault folder, emptied first.
    fn store(name: &str) -> (Store, std::path::PathBuf) {
        let dir = scratch(name);
        let tag = format!("Copper (test {name} {})", std::process::id());
        let s = Store::open_with(&tag, Vault::at(dir.clone())).expect("Credential Manager");
        for c in cred::enumerate(&tag_filter(&tag)).unwrap() {
            cred::delete(&c.target).unwrap();
        }
        (s, dir)
    }

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn logins_round_trip_through_credential_manager() {
        let (s, dir) = store("crud");
        block(async {
            s.save("example.com", "Ann", "pw-A", Some(100.0))
                .await
                .unwrap();
            s.save("example.com", "ann", "pw-a", None).await.unwrap();
            s.save("example.com", "", "pw-none", None).await.unwrap();
            s.save("other.example.com", "x/y*z", "pw-x", None)
                .await
                .unwrap();
            // Case-distinct users stay distinct.
            assert_eq!(
                s.secret("example.com", "Ann").await.unwrap().as_deref(),
                Some("pw-A")
            );
            assert_eq!(
                s.secret("example.com", "ann").await.unwrap().as_deref(),
                Some("pw-a")
            );
            assert_eq!(
                s.secret("example.com", "").await.unwrap().as_deref(),
                Some("pw-none")
            );
            assert_eq!(s.secret("example.com", "zed").await.unwrap(), None);
            let mut users: Vec<String> = s
                .logins(Some("example.com"))
                .await
                .unwrap()
                .into_iter()
                .map(|l| l.user)
                .collect();
            users.sort();
            assert_eq!(users, ["", "Ann", "ann"]);
            assert_eq!(s.logins(None).await.unwrap().len(), 4);
            // `used` survives a save without one and moves on touch.
            s.save("example.com", "Ann", "pw-A2", None).await.unwrap();
            let ann = |l: &Vec<Login>| l.iter().find(|l| l.user == "Ann").unwrap().used;
            assert_eq!(
                ann(&s.logins(Some("example.com")).await.unwrap()),
                Some(100.0)
            );
            s.touch("example.com", "Ann", 300.0).await.unwrap();
            assert_eq!(
                ann(&s.logins(Some("example.com")).await.unwrap()),
                Some(300.0)
            );
            assert_eq!(
                s.secret("example.com", "Ann").await.unwrap().as_deref(),
                Some("pw-A2")
            );
            // Visible to lookup by the Linux attribute names.
            let got = lookup_in(
                &s.vault,
                &[
                    ("app", s.tag()),
                    ("origin", "other.example.com"),
                    ("username", "x/y*z"),
                ],
            )
            .unwrap();
            assert_eq!(got.as_deref(), Some(b"pw-x".as_slice()));
            s.forget("example.com", "ann").await.unwrap();
            assert_eq!(s.secret("example.com", "ann").await.unwrap(), None);
            assert_eq!(s.forget_origin("example.com").await.unwrap(), 2);
            assert_eq!(s.forget_origin("other.example.com").await.unwrap(), 1);
            assert!(s.logins(None).await.unwrap().is_empty());
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn big_secrets_go_to_the_vault_and_back() {
        let (s, dir) = store("big");
        let target = login_target(s.tag(), "big.example", "ann");
        let big: String = "0123456789abcdef".repeat(400); // 6400 bytes
        block(async {
            s.save("big.example", "ann", &big, Some(5.0)).await.unwrap();
            assert!(s.vault.holds(&target));
            let c = cred::read(&target).unwrap().unwrap();
            assert_eq!(c.blob.as_slice(), POINTER_BLOB);
            assert!(c.attribute(ATTR_VAULT).is_some());
            assert_eq!(
                s.secret("big.example", "ann").await.unwrap().as_deref(),
                Some(big.as_str())
            );
            // Touch keeps the pointer and the sealed secret.
            s.touch("big.example", "ann", 6.0).await.unwrap();
            assert_eq!(
                s.secret("big.example", "ann").await.unwrap().as_deref(),
                Some(big.as_str())
            );
            // Exactly the limit stays inline.
            let edge = "e".repeat(MAX_BLOB);
            s.save("big.example", "ann", &edge, None).await.unwrap();
            assert!(!s.vault.holds(&target), "a shrunk secret leaves no file");
            assert_eq!(cred::read(&target).unwrap().unwrap().blob.len(), MAX_BLOB);
            assert_eq!(
                s.secret("big.example", "ann").await.unwrap().as_deref(),
                Some(edge.as_str())
            );
            // Big again, then forget: credential and file both go.
            s.save("big.example", "ann", &big, None).await.unwrap();
            assert!(s.vault.holds(&target));
            s.forget("big.example", "ann").await.unwrap();
            assert!(!s.vault.holds(&target));
            assert!(cred::read(&target).unwrap().is_none());
        });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn raw_items_by_attribute() {
        let dir = scratch("raw");
        let v = Vault::at(dir.clone());
        let marker = format!("copper-test-{}", std::process::id());
        let attrs = [("application", marker.as_str()), ("xdg:schema", "x")];
        let big = vec![7u8; MAX_BLOB + 1];
        store_raw_in(&v, "Test key", &attrs, b"key-1").unwrap();
        store_raw_in(&v, "Test key", &attrs, b"key-2").unwrap();
        assert_eq!(
            lookup_in(&v, &[("application", &marker)])
                .unwrap()
                .as_deref(),
            Some(b"key-2".as_slice())
        );
        store_raw_in(&v, "Test key", &attrs, &big).unwrap();
        assert_eq!(
            lookup_in(&v, &[("application", &marker)])
                .unwrap()
                .as_deref(),
            Some(big.as_slice())
        );
        assert_eq!(
            lookup_in(&v, &[("application", "copper-nope")]).unwrap(),
            None
        );
        assert!(store_raw_in(&v, "x", &[("copper:vault", "x")], b"x").is_err());
        let target = raw_target(&attrs);
        assert!(cred::delete(&target).unwrap());
        v.remove(&target);
        let _ = std::fs::remove_dir_all(dir);
    }
}
