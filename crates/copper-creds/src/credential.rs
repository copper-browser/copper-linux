//! The source-neutral vocabulary (`Credential.swift`, `Autofill.swift`) and
//! the picker's ordering and duplicate policy (`CredentialStore.swift`).

use std::cmp::Ordering;
use std::fmt;

use crate::site::{bare, normalized};

/// A saved sign-in's stable id, independent of where its secret lives. The
/// string form is the Mac's (`kc:host\u{1}user`, `bw:<id>`, `op:<id>`), used
/// in settings and the agent allow-list; on Linux `kc` is the Secret Service.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CredentialId {
    Keychain { host: String, user: String },
    Bitwarden(String),
    OnePassword(String),
}

impl CredentialId {
    pub fn parse(text: &str) -> Option<Self> {
        if let Some(body) = text.strip_prefix("kc:") {
            let (host, user) = body.split_once('\u{1}')?;
            if host.is_empty() || user.is_empty() {
                return None;
            }
            return Some(Self::Keychain {
                host: host.into(),
                user: user.into(),
            });
        }
        if let Some(id) = text.strip_prefix("bw:") {
            return (!id.is_empty()).then(|| Self::Bitwarden(id.into()));
        }
        if let Some(id) = text.strip_prefix("op:") {
            return (!id.is_empty()).then(|| Self::OnePassword(id.into()));
        }
        None
    }

    pub fn source(&self) -> Source {
        match self {
            Self::Keychain { .. } => Source::Keychain,
            Self::Bitwarden(_) => Source::Bitwarden,
            Self::OnePassword(_) => Source::OnePassword,
        }
    }
}

impl fmt::Display for CredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keychain { host, user } => write!(f, "kc:{host}\u{1}{user}"),
            Self::Bitwarden(id) => write!(f, "bw:{id}"),
            Self::OnePassword(id) => write!(f, "op:{id}"),
        }
    }
}

impl serde::Serialize for CredentialId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for CredentialId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom("Invalid credential id"))
    }
}

/// Where a credential's secret lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// The Secret Service here (macOS: the keychain).
    Keychain,
    Bitwarden,
    OnePassword,
}

impl Source {
    /// How a person reads it.
    pub fn title(self) -> &'static str {
        match self {
            Source::Keychain => "keychain",
            Source::Bitwarden => "Bitwarden",
            Source::OnePassword => "1Password",
        }
    }
}

/// Whether an agent may use it (an `Agents` folder/vault, a `copper-agent`
/// tag, or a `copper-agent: deny` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AgentHint {
    #[default]
    None,
    Allow,
    Deny,
}

/// One saved sign-in as the picker and agents see it. Never holds a secret.
#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    pub id: CredentialId,
    pub source: Source,
    pub host: String,
    pub user: String,
    pub sites: Vec<String>,
    pub name: String,
    pub has_totp: bool,
    /// Last used, Unix seconds.
    pub used: Option<f64>,
    pub folder: Option<String>,
    pub agent_hint: AgentHint,
}

/// A Secret Service login: the item's attributes, never its secret.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    pub host: String,
    pub user: String,
    /// Last used for a sign-in, Unix seconds.
    pub used: Option<f64>,
}

impl Login {
    pub fn credential(&self) -> Credential {
        Credential {
            id: CredentialId::Keychain {
                host: self.host.clone(),
                user: self.user.clone(),
            },
            source: Source::Keychain,
            host: self.host.clone(),
            user: self.user.clone(),
            sites: vec![self.host.clone()],
            name: self.user.clone(),
            has_totp: false,
            used: self.used,
            folder: None,
            agent_hint: AgentHint::None,
        }
    }
}

/// An identity from a vault, for form autofill (`AutofillIdentity`).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutofillIdentity {
    pub id: String,
    pub name: String,
    pub title: String,
    pub first_name: String,
    pub middle_name: String,
    pub last_name: String,
    pub username: String,
    pub company: String,
    pub email: String,
    pub phone: String,
    pub address1: String,
    pub address2: String,
    pub address3: String,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub country: String,
    pub ssn: String,
    pub passport_number: String,
    pub license_number: String,
}

impl AutofillIdentity {
    /// First, middle and last name, the empty ones left out.
    pub fn full_name(&self) -> String {
        [&self.first_name, &self.middle_name, &self.last_name]
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The line under its name in the list (`AutofillIdentity.summary`):
    /// the address, else the email, the phone, the name.
    pub fn summary(&self) -> String {
        fn joined(parts: &[&str], by: &str) -> String {
            parts
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(by)
        }
        let street = joined(&[&self.address1, &self.address2, &self.address3], ", ");
        let locality = joined(&[&self.city, &self.state, &self.postal_code], " ");
        let place = joined(&[&locality, &self.country], ", ");
        let address = joined(&[&street, &place], ", ");
        let name = self.full_name();
        [address, self.email.clone(), self.phone.clone(), name]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_else(|| self.name.clone())
    }
}

/// A payment card from a vault (`AutofillCard`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AutofillCard {
    pub id: String,
    pub name: String,
    pub cardholder_name: String,
    pub brand: String,
    pub exp_month: String,
    pub exp_year: String,
    pub number: String,
    pub code: String,
}

impl fmt::Debug for AutofillCard {
    // Never the number or the code.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutofillCard")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("brand", &self.brand)
            .field("last4", &self.last4())
            .finish_non_exhaustive()
    }
}

impl AutofillCard {
    pub fn last4(&self) -> String {
        let digits: Vec<char> = self.number.chars().filter(char::is_ascii_digit).collect();
        if digits.len() < 4 {
            return String::new();
        }
        digits[digits.len() - 4..].iter().collect()
    }

    /// "Visa •••• 1111" (`AutofillCard.label`): the brand and the last
    /// four, whichever there are.
    pub fn label(&self) -> String {
        let last4 = self.last4();
        let tail = if last4.is_empty() {
            String::new()
        } else {
            format!("•••• {last4}")
        };
        [self.brand.as_str(), tail.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A custom field on a vault item; a hidden one carries no value here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VaultField {
    pub name: String,
    pub value: String,
    pub hidden: bool,
}

/// Case-insensitive comparison, the way `localizedCaseInsensitiveCompare`
/// orders plain names.
fn caseless(a: &str, b: &str) -> Ordering {
    a.to_lowercase().cmp(&b.to_lowercase())
}

/// One row per host + user: a vault's copy wins over the keychain's (it
/// carries a TOTP key, custom fields, the agent policy); the first vault's
/// over the second.
pub fn deduplicated(values: Vec<Credential>) -> Vec<Credential> {
    let mut out: Vec<Credential> = Vec::with_capacity(values.len());
    let mut at = std::collections::HashMap::new();
    for value in values {
        let key = format!(
            "{}\u{1}{}",
            normalized(&value.host),
            value.user.to_lowercase()
        );
        match at.get(&key) {
            None => {
                at.insert(key, out.len());
                out.push(value);
            }
            Some(&i) => {
                if value.source != Source::Keychain && out[i].source == Source::Keychain {
                    out[i] = value;
                }
            }
        }
    }
    out
}

/// Most recently used first, then by name, then by user.
pub fn sorted(mut values: Vec<Credential>) -> Vec<Credential> {
    values.sort_by(|a, b| {
        let (l, r) = (a.used.unwrap_or(f64::MIN), b.used.unwrap_or(f64::MIN));
        r.partial_cmp(&l)
            .unwrap_or(Ordering::Equal)
            .then_with(|| caseless(&a.name, &b.name))
            .then_with(|| caseless(&a.user, &b.user))
    });
    values
}

/// Every source's rows for one site, merged the Mac's way.
pub fn combine(
    keychain: Vec<Credential>,
    bitwarden: Vec<Credential>,
    one_password: Vec<Credential>,
) -> Vec<Credential> {
    let mut all = keychain;
    all.extend(bitwarden);
    all.extend(one_password);
    sorted(deduplicated(all))
}

/// The accounts for a site, best first: the one the page already names
/// (`hint`), then those kept for this exact host, then the order given.
pub fn ranked(all: Vec<Credential>, host: &str, hint: &str) -> Vec<Credential> {
    let wanted = hint.trim().to_lowercase();
    let exact = bare(host);
    let rank = |c: &Credential| {
        let mut score = 0;
        if !wanted.is_empty() && c.user.to_lowercase() == wanted {
            score -= 100;
        }
        if bare(&c.host) == exact {
            score -= 10;
        }
        score
    };
    let mut rows: Vec<(usize, Credential)> = all.into_iter().enumerate().collect();
    rows.sort_by(|(ia, a), (ib, b)| rank(a).cmp(&rank(b)).then(ia.cmp(ib)));
    rows.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(source: Source, host: &str, user: &str, used: Option<f64>) -> Credential {
        let id = match source {
            Source::Keychain => CredentialId::Keychain {
                host: host.into(),
                user: user.into(),
            },
            Source::Bitwarden => CredentialId::Bitwarden(format!("{host}-{user}")),
            Source::OnePassword => CredentialId::OnePassword(format!("{host}-{user}")),
        };
        Credential {
            id,
            source,
            host: host.into(),
            user: user.into(),
            sites: vec![],
            name: user.into(),
            has_totp: false,
            used,
            folder: None,
            agent_hint: AgentHint::None,
        }
    }

    #[test]
    fn ids_round_trip() {
        for s in ["kc:example.com\u{1}ann", "bw:abc", "op:xyz"] {
            assert_eq!(CredentialId::parse(s).unwrap().to_string(), s);
        }
        assert!(CredentialId::parse("kc:example.com").is_none());
        assert!(CredentialId::parse("bw:").is_none());
        assert!(CredentialId::parse("zz:1").is_none());
        let json = serde_json::to_string(&CredentialId::Bitwarden("1".into())).unwrap();
        assert_eq!(json, "\"bw:1\"");
    }

    #[test]
    fn vault_copy_wins_and_order() {
        let rows = combine(
            vec![
                cred(Source::Keychain, "example.com", "ann", Some(5.0)),
                cred(Source::Keychain, "example.com", "bob", None),
            ],
            vec![cred(Source::Bitwarden, "Example.com", "ANN", None)],
            vec![cred(Source::OnePassword, "example.com", "ann", None)],
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, Source::Bitwarden);
        let ranked = ranked(rows, "www.example.com", "bob");
        assert_eq!(ranked[0].user, "bob");
    }

    #[test]
    fn identity_summary_prefers_the_address() {
        let mut i = AutofillIdentity {
            name: "Home".into(),
            first_name: "Ana".into(),
            last_name: "Lima".into(),
            email: "ana@example.com".into(),
            ..Default::default()
        };
        assert_eq!(i.full_name(), "Ana Lima");
        assert_eq!(i.summary(), "ana@example.com");
        i.address1 = "1 Main St".into();
        i.city = "Springfield".into();
        i.state = "IL".into();
        i.postal_code = "62701".into();
        i.country = "US".into();
        assert_eq!(i.summary(), "1 Main St, Springfield IL 62701, US");
        let bare = AutofillIdentity {
            name: "Work".into(),
            ..Default::default()
        };
        assert_eq!(bare.summary(), "Work");
    }

    #[test]
    fn card_label_is_brand_and_last_four() {
        let card = AutofillCard {
            brand: "Visa".into(),
            number: "4111 1111 1111 1234".into(),
            ..Default::default()
        };
        assert_eq!(card.label(), "Visa •••• 1234");
        let no_brand = AutofillCard {
            number: "4111111111115678".into(),
            ..Default::default()
        };
        assert_eq!(no_brand.label(), "•••• 5678");
        assert_eq!(AutofillCard::default().label(), "");
        assert!(!format!("{card:?}").contains("4111"));
    }
}
