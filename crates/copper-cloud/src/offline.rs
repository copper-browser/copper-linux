//! Joining Copper Cloud with no browser window open: what `copper cloud
//! pair`, `copper cloud sso` and `copper cloud status` do. A headless
//! Copper (an agent's own browser) is paired before it is started.
//!
//! Everything goes through the files a window would read — `cloud.json`
//! (the link, the account, the token, the sync switches), `cloud-sync.json`
//! (the Personal canvas switch) and `cloud-intelligence.json` — in the
//! [`Store`] given, which is the current `COPPER_WORLD`'s. Toolkit-free and
//! OS-neutral: a Windows shell runs the same functions.
//!
//! The caller makes sure no running Copper owns the profile first: a window
//! keeps its own copy of `cloud.json` and would write over this one.

use crate::client::{CloudClient, wire_id};
use crate::error::{Failure, Result};
use crate::intelligence::{CloudIntelligence, provides};
use crate::link::{JoinCode, parse_join_code};
use crate::sso::{SsoCancel, SsoOutcome};
use crate::sync::Domain;
use copper_core::Store;
use copper_core::cloud::{CloudFile, CloudProvided, CloudSyncFile};
use serde::Serialize;
use std::collections::BTreeSet;
use std::time::Duration;

/// How long the best-effort extras after a sign-in may take (the device
/// rename, the provided keys).
const EXTRAS_TIMEOUT: Duration = Duration::from_secs(20);

/// What syncs once signed in (`--sync`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SyncChoice {
    /// What pairing does in Settings: the switches this device had before,
    /// or every one.
    #[default]
    Default,
    /// Nothing: sync stays off.
    Nothing,
    /// Exactly these.
    These(BTreeSet<Domain>),
}

impl SyncChoice {
    /// `none`, `all`, or a comma list of `spaces`, `settings`, `bookmarks`,
    /// `tabs`, `history`, `canvas`.
    pub fn parse(text: &str) -> std::result::Result<SyncChoice, String> {
        let text = text.trim().to_ascii_lowercase();
        match text.as_str() {
            "none" | "off" | "" => return Ok(SyncChoice::Nothing),
            "all" | "everything" => {
                return Ok(SyncChoice::These(Domain::ALL.into_iter().collect()));
            }
            _ => {}
        }
        let mut set = BTreeSet::new();
        for word in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            let domain = match word {
                "space" | "kept" => Some(Domain::Spaces),
                "bookmark" => Some(Domain::Bookmarks),
                "tab" | "open-tabs" => Some(Domain::Tabs),
                "setting" => Some(Domain::Settings),
                other => Domain::parse(other),
            };
            match domain {
                Some(d) => {
                    set.insert(d);
                }
                None => {
                    return Err(format!(
                        "unknown sync choice: {word} (use none, all, or a list of spaces,settings,bookmarks,tabs,history,canvas)"
                    ));
                }
            }
        }
        Ok(if set.is_empty() {
            SyncChoice::Nothing
        } else {
            SyncChoice::These(set)
        })
    }
}

/// How to join.
#[derive(Clone, Debug, Default)]
pub struct JoinOptions {
    pub sync: SyncChoice,
    /// This device's name, here and on the server.
    pub name: Option<String>,
    /// A fingerprint a person confirmed after an `untrusted` failure.
    pub trust: Option<String>,
    /// The provider to sign in with (`copper cloud sso` on an instance with
    /// more than one).
    pub provider: Option<String>,
}

/// Signed in. Never carries the token or the instance key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Joined {
    pub email: String,
    /// As the server knows it: lowercase.
    pub device_id: String,
    pub device_name: String,
    /// `host:port`.
    pub host: String,
    /// The domains that sync, in the Mac's order; empty: sync is off.
    pub sync: Vec<&'static str>,
    /// Single sign-on only: the instance made the account just now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    /// What the cloud provides keys for (`jev`, `router`), names only.
    pub provided: Vec<&'static str>,
    /// Something that didn't work but didn't stop the sign-in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Pair with a pairing code (`#p=`, or a `copper://cloud/join` deep link to
/// one): link, sign in, turn sync on as chosen, and fetch what the cloud
/// provides.
pub async fn pair(store: Store, code: &str, opts: &JoinOptions) -> Result<Joined> {
    let client = open(store)?;
    match parse_join_code(code, client.link().as_ref()) {
        Some(JoinCode::Pairing(pairing)) => {
            name_first(&client, opts);
            client.pair(&pairing, opts.trust.as_deref()).await?;
            Ok(finish(&client, opts, None).await)
        }
        Some(JoinCode::Sso(sso)) => Err(Failure::new(
            0,
            "sso_link",
            format!(
                "That's a sign-in link for {} — run `copper cloud sso` with it (it opens a browser), or use a pairing code",
                sso.host()
            ),
        )),
        Some(JoinCode::Link(_)) => Err(Failure::new(
            0,
            "link_code",
            "That's a link code: it connects, but signing in needs an account — use a pairing code (Settings › Cloud › Pair another device on a signed-in Copper), or `copper cloud sso` on an instance with single sign-on",
        )),
        None => Err(Failure::new(
            0,
            "code",
            "That isn't a pairing code — paste the whole copper-cloud://…#p=… text",
        )),
    }
}

/// Sign in with single sign-on: a sign-in link (`#sso=`), a link code
/// (`#k=`, connected first), or none (the instance this Copper is linked to
/// already). `open` opens the system browser.
pub async fn sso(
    store: Store,
    code: Option<&str>,
    opts: &JoinOptions,
    open_browser: impl FnOnce(&str) -> std::result::Result<(), String>,
    cancel: &SsoCancel,
) -> Result<Joined> {
    let client = open(store)?;
    name_first(&client, opts);
    let outcome: SsoOutcome = match code.map(|c| (c, parse_join_code(c, client.link().as_ref()))) {
        Some((_, Some(JoinCode::Sso(link)))) => {
            client
                .sign_in_with_sso(&link, opts.trust.as_deref(), open_browser, cancel)
                .await?
        }
        Some((_, Some(JoinCode::Link(link)))) => {
            client.connect(link, opts.trust.as_deref()).await?;
            let provider = choose_provider(&client, opts).await?;
            client
                .sign_in_with_sso_here(&provider, open_browser, cancel)
                .await?
        }
        Some((_, Some(JoinCode::Pairing(_)))) => {
            return Err(Failure::new(
                0,
                "pairing_code",
                "That's a pairing code — run `copper cloud pair` with it",
            ));
        }
        Some((_, None)) => {
            return Err(Failure::new(
                0,
                "code",
                "That isn't a sign-in link — paste the whole copper-cloud://…#sso=… text",
            ));
        }
        None => {
            if !client.is_linked() {
                return Err(Failure::new(
                    0,
                    "not_linked",
                    "Copper isn't connected to a cloud — give the instance's sign-in link (copper-cloud://…#sso=…)",
                ));
            }
            let provider = choose_provider(&client, opts).await?;
            client
                .sign_in_with_sso_here(&provider, open_browser, cancel)
                .await?
        }
    };
    Ok(finish(&client, opts, Some(outcome.created)).await)
}

async fn choose_provider(client: &CloudClient, opts: &JoinOptions) -> Result<String> {
    let info = client
        .sso_info()
        .await?
        .ok_or_else(|| crate::sso::sso_failure(Failure::new(404, "http_404", ""), &[]))?;
    let offered = info.offered();
    if let Some(wanted) = opts.provider.as_deref() {
        return info.provider(wanted).map(|p| p.id.clone()).ok_or_else(|| {
            Failure::new(
                0,
                "sso_provider",
                format!(
                    "This instance has no sign-in called {wanted} (it has: {})",
                    names(offered)
                ),
            )
        });
    }
    match offered {
        [] => Err(crate::sso::sso_failure(
            Failure::new(404, "sso_disabled", ""),
            &[],
        )),
        [one] => Ok(one.id.clone()),
        many => Err(Failure::new(
            0,
            "sso_provider",
            format!(
                "This instance signs in with more than one provider — choose one with --provider ({})",
                names(many)
            ),
        )),
    }
}

fn names(providers: &[crate::sso::SsoProvider]) -> String {
    if providers.is_empty() {
        return "none".into();
    }
    providers
        .iter()
        .map(|p| p.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn open(store: Store) -> Result<CloudClient> {
    CloudClient::open(store).map_err(|e| Failure::new(0, "store", format!("cloud.json: {e}")))
}

/// `--name` goes with the sign-in itself, so the server knows the device by
/// it from the start.
fn name_first(client: &CloudClient, opts: &JoinOptions) {
    if let Some(name) = opts
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        client.set_device_name(name);
    }
}

/// After any sign-in: the name (when the server chose another, such as a
/// code minted for a named device), the sync switches, the provided keys.
async fn finish(client: &CloudClient, opts: &JoinOptions, created: Option<bool>) -> Joined {
    let mut warnings = vec![];
    if let Some(name) = opts
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        && client.device_name() != name
    {
        match tokio::time::timeout(EXTRAS_TIMEOUT, client.rename_device(name)).await {
            Ok(Ok(())) => {}
            Ok(Err(f)) => warnings.push(format!("the server kept the device's old name: {f}")),
            Err(_) => warnings.push("the server didn't answer the rename in time".into()),
        }
    }
    let domains = apply_sync(client, &opts.sync);
    let keys = CloudIntelligence::new(client.clone());
    let provided = match tokio::time::timeout(EXTRAS_TIMEOUT, keys.fetch()).await {
        Ok(p) => p.as_ref().map(provides).unwrap_or_default(),
        Err(_) => {
            warnings.push("the cloud didn't say in time which keys it provides".into());
            vec![]
        }
    };
    let account = client.account();
    Joined {
        email: account
            .as_ref()
            .map(|a| a.email.clone())
            .unwrap_or_default(),
        device_id: wire_id(
            &account
                .map(|a| a.device_id)
                .unwrap_or_else(|| client.device_id()),
        ),
        device_name: client.device_name(),
        host: client.link().map(|l| l.host()).unwrap_or_default(),
        sync: Domain::ALL
            .into_iter()
            .filter(|d| domains.contains(d))
            .map(Domain::as_str)
            .collect(),
        created,
        provided,
        warnings,
    }
}

/// The switches this profile has now: `cloud.json`'s and the canvas's in
/// `cloud-sync.json`, whether or not sync is on.
pub fn switches(store: &Store, file: &CloudFile) -> BTreeSet<Domain> {
    let p = &file.sync;
    let canvas = store
        .load::<CloudSyncFile>()
        .map(|l| l.value.canvas.unwrap_or(false))
        .unwrap_or(false);
    [
        (p.spaces, Domain::Spaces),
        (p.settings, Domain::Settings),
        (p.bookmarks, Domain::Bookmarks),
        (p.tabs, Domain::Tabs),
        (p.history, Domain::History),
        (canvas, Domain::Canvas),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, d)| d)
    .collect()
}

/// Set the sync switches as a window's "Turn on sync" would, without
/// syncing: the window that opens this profile next syncs at once (a first
/// sync merges; nothing on either side is thrown away). Returns what syncs.
pub fn apply_sync(client: &CloudClient, choice: &SyncChoice) -> BTreeSet<Domain> {
    let store = client.store().clone();
    let file = CloudFile {
        sync: client.sync_prefs(),
        ..Default::default()
    };
    let domains: BTreeSet<Domain> = match choice {
        SyncChoice::Nothing => BTreeSet::new(),
        SyncChoice::These(set) => set.clone(),
        SyncChoice::Default => {
            let before = switches(&store, &file);
            if before.is_empty() {
                Domain::ALL.into_iter().collect()
            } else {
                before
            }
        }
    };
    let on = !domains.is_empty();
    client.update_sync_prefs(|p| {
        p.spaces = domains.contains(&Domain::Spaces);
        p.settings = domains.contains(&Domain::Settings);
        p.bookmarks = domains.contains(&Domain::Bookmarks);
        p.tabs = domains.contains(&Domain::Tabs);
        p.history = domains.contains(&Domain::History);
        p.on = on;
    });
    let canvas = domains.contains(&Domain::Canvas);
    let mut saved = store
        .load::<CloudSyncFile>()
        .map(|l| l.value)
        .unwrap_or_default();
    if saved.canvas.unwrap_or(false) != canvas {
        saved.canvas = Some(canvas);
        if let Err(e) = store.save(&saved) {
            tracing::error!(error = %e, "cloud-sync.json not written");
        }
    }
    domains
}

/// This profile's Copper Cloud, read from the files alone (nothing is sent,
/// nothing is written). Never carries the token or a key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    pub linked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The instance is pinned by a certificate fingerprint (else the public
    /// authorities vouch for it, or it is a loopback development server).
    pub pinned: bool,
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// As the server knows it: lowercase.
    pub device_id: String,
    pub device_name: String,
    pub sync: SyncState,
    /// The keys this account's cloud provides, as cached: names only.
    pub provided: Vec<&'static str>,
}

/// The sync switches, and whether sync is on at all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SyncState {
    /// "Turn on sync" was pressed (or `copper cloud pair` did it).
    pub on: bool,
    pub spaces: bool,
    pub settings: bool,
    pub bookmarks: bool,
    pub tabs: bool,
    pub history: bool,
    pub canvas: bool,
}

impl SyncState {
    fn of(on: bool, domains: &BTreeSet<Domain>) -> SyncState {
        SyncState {
            on,
            spaces: domains.contains(&Domain::Spaces),
            settings: domains.contains(&Domain::Settings),
            bookmarks: domains.contains(&Domain::Bookmarks),
            tabs: domains.contains(&Domain::Tabs),
            history: domains.contains(&Domain::History),
            canvas: domains.contains(&Domain::Canvas),
        }
    }

    /// The switches that are on, in the Mac's order.
    pub fn domains(&self) -> Vec<&'static str> {
        [
            (self.spaces, Domain::Spaces),
            (self.settings, Domain::Settings),
            (self.bookmarks, Domain::Bookmarks),
            (self.tabs, Domain::Tabs),
            (self.history, Domain::History),
            (self.canvas, Domain::Canvas),
        ]
        .into_iter()
        .filter(|(v, _)| *v)
        .map(|(_, d)| d.as_str())
        .collect()
    }
}

/// [`Status`] of the profile in `store`.
pub fn status(store: &Store) -> Status {
    let file = store
        .load_opt::<CloudFile>()
        .ok()
        .and_then(|l| l.value)
        .unwrap_or_default();
    let link = file.link.as_ref().and_then(crate::link::Link::from_saved);
    let account = file.token.as_ref().and(file.account.clone());
    let on = switches(store, &file);
    let provided = match (&link, &account) {
        (Some(link), Some(account)) => {
            let owner = format!(
                "{}|{}",
                link.endpoint.base(),
                account.user_id.to_ascii_lowercase()
            );
            store
                .load_opt::<CloudProvided>()
                .ok()
                .and_then(|l| l.value)
                .filter(|p| p.owner.as_deref() == Some(owner.as_str()))
                .map(|p| provides(&p))
                .unwrap_or_default()
        }
        _ => vec![],
    };
    Status {
        linked: link.is_some(),
        host: link.as_ref().map(|l| l.host()),
        pinned: link.as_ref().is_some_and(|l| l.fingerprint.is_some()),
        signed_in: account.is_some() && link.is_some(),
        email: account.as_ref().map(|a| a.email.clone()),
        display_name: account.as_ref().map(|a| a.display_name.clone()),
        device_id: wire_id(
            &account
                .as_ref()
                .map(|a| a.device_id.clone())
                .unwrap_or_else(|| file.device_id.clone()),
        ),
        device_name: file.device_name.clone(),
        sync: SyncState::of(file.sync.on, &on),
        provided,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::Paths;

    fn store(dir: &std::path::Path) -> Store {
        Store::new(Paths::new(dir.join("data"), dir.join("config")))
    }

    #[test]
    fn sync_choices() {
        assert_eq!(SyncChoice::parse("none"), Ok(SyncChoice::Nothing));
        assert_eq!(
            SyncChoice::parse("ALL"),
            Ok(SyncChoice::These(Domain::ALL.into_iter().collect()))
        );
        assert_eq!(
            SyncChoice::parse("tabs, bookmarks,spaces,settings,history"),
            Ok(SyncChoice::These(
                [
                    Domain::Tabs,
                    Domain::Bookmarks,
                    Domain::Spaces,
                    Domain::Settings,
                    Domain::History
                ]
                .into_iter()
                .collect()
            ))
        );
        assert!(
            SyncChoice::parse("tabs,passwords")
                .unwrap_err()
                .contains("passwords")
        );
    }

    #[tokio::test]
    async fn switches_are_written_where_a_window_reads_them() {
        let tmp = tempfile::tempdir().unwrap();
        let client = CloudClient::open(store(tmp.path())).unwrap();
        // Default on a fresh profile: everything.
        let on = apply_sync(&client, &SyncChoice::Default);
        assert_eq!(on.len(), Domain::ALL.len());
        let s = status(&store(tmp.path()));
        assert!(s.sync.on);
        assert_eq!(s.sync.domains().len(), Domain::ALL.len());
        // Exactly the ones asked for; the canvas lives in cloud-sync.json.
        let tabs = SyncChoice::parse("tabs").unwrap();
        assert_eq!(
            apply_sync(&client, &tabs),
            [Domain::Tabs].into_iter().collect()
        );
        let s = status(&store(tmp.path()));
        assert_eq!(
            s.sync,
            SyncState {
                on: true,
                tabs: true,
                ..Default::default()
            }
        );
        let saved: CloudSyncFile = store(tmp.path()).load().unwrap().value;
        assert_eq!(saved.canvas, Some(false));
        // Default again: the switches from before, not everything.
        assert_eq!(
            apply_sync(&client, &SyncChoice::Default),
            [Domain::Tabs].into_iter().collect()
        );
        // None: off.
        assert!(apply_sync(&client, &SyncChoice::Nothing).is_empty());
        let s = status(&store(tmp.path()));
        assert!(!s.sync.on);
        assert!(!s.linked && !s.signed_in);
        assert_eq!(s.device_id, client.device_id().to_ascii_lowercase());
    }

    #[tokio::test]
    async fn codes_that_dont_pair_say_what_to_do() {
        let tmp = tempfile::tempdir().unwrap();
        let fp = "a".repeat(64);
        let opts = JoinOptions::default();
        let err = pair(
            store(tmp.path()),
            &format!("copper-cloud://h:1/#sso=google&fp={fp}"),
            &opts,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "sso_link");
        assert!(err.message.contains("copper cloud sso"));
        let err = pair(
            store(tmp.path()),
            &format!("copper-cloud://h:1/#k=ck_abcdefghijklmnop&fp={fp}"),
            &opts,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "link_code");
        let err = pair(store(tmp.path()), "hello", &opts).await.unwrap_err();
        assert_eq!(err.code, "code");
        let err = sso(
            store(tmp.path()),
            None,
            &opts,
            |_: &str| Ok(()),
            &SsoCancel::new(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "not_linked");
    }
}
