//! `browser_sign_in` / `browser_autofill` (Mac `Fork/MCP/SignIn.swift`,
//! `AgentAutofill.swift`). A saved account the user shared with agents is
//! filled into the page in-process; the secret goes from the vault into the
//! page's fill script and never into the result. The agents lane calls these
//! with the tab it resolved; signatures are fixed.

use super::watch::call;
use super::{FormsUi, agent_allowed, autofill_allowed, rt, site_host, site_origin};
use crate::shell::Shell;
use crate::{PageOf, Platform};
use copper_core::TabId;
use copper_creds::credential::{AgentHint, AutofillCard, AutofillIdentity, Credential, Source};
use copper_engine::PageView;
use copper_pagejs::jev::Extras;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// Where an agent's fill goes, fixed when the call starts (Mac
/// `Tab.FormTarget`): the page, its site, and its origin (scheme, host and
/// port). Every read, fill and submit goes there and only while the page is
/// still of that origin ([`copper_pagejs::Call::at`]): a page that navigates
/// during the call gets nothing.
struct Target<V> {
    page: V,
    host: String,
    origin: String,
}

fn target<P: Platform>(
    shell: &Shell<P>,
    tab: &TabId,
    what: &str,
) -> Result<Target<PageOf<P>>, String> {
    let (private, url) = {
        let b = shell.browser();
        let t = b.tab(tab).ok_or("no active tab")?;
        (t.private, t.url.clone())
    };
    if private {
        return Err(format!("{what} is unavailable in private tabs"));
    }
    let page = shell.ensure_page(tab).ok_or("no active tab")?;
    let url = page.url().filter(|u| !u.is_empty()).unwrap_or(url);
    let host = site_host(&url).ok_or("current tab has no host")?;
    let origin = site_origin(&url).ok_or("current tab has no host")?;
    Ok(Target { page, host, origin })
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::Bitwarden => "bitwarden",
        Source::OnePassword => "onepassword",
        Source::Keychain => "keychain",
        Source::BotPass => "botpass",
    }
}

const BW_LOCKED: &str = "bitwarden is locked — unlock it in Settings › Passwords";
const OP_LOCKED: &str = "1password is locked — unlock it in Settings › Passwords";
const BP_LOCKED: &str = "BotPass is locked — run bp unlock";

fn locked_hint(c: &Credential, e: String) -> String {
    let Some(v) = super::vault() else { return e };
    let snap = v.snapshot();
    match c.source {
        Source::Bitwarden if snap.bw_locked() => BW_LOCKED.into(),
        Source::OnePassword if !snap.op_unlocked() => OP_LOCKED.into(),
        Source::BotPass if snap.bp_locked() => BP_LOCKED.into(),
        _ => e,
    }
}

/// The page is still where the call started: else nothing is read from the
/// vault, and nothing is filled. (BotPass, asked per origin, would answer
/// for the new one.)
fn still_there<V: PageView>(page: &V, origin: &str) -> Result<(), String> {
    if page.url().as_deref().and_then(site_origin).as_deref() != Some(origin) {
        return Err(format!(
            "the page left {origin} during the call — nothing was filled; take a fresh browser_snapshot and call again"
        ));
    }
    Ok(())
}

/// Send the form the password box is in — or, for a code, the code box's.
async fn submit<V: PageView>(page: &V, origin: &str, code: bool) -> bool {
    if code {
        return call(page, &copper_pagejs::forms::submit_otp().at(origin))
            .await
            .unwrap_or(false);
    }
    call(page, &copper_pagejs::forms::submit().at(origin))
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// The shared logins an agent may fill on `origin`: shared, and saved for
/// that exact address ([`copper_creds::origin::owns`]). BotPass's grants
/// already answer per origin: what it offers here is this origin's.
fn owned(c: &Credential, origin: &str) -> bool {
    c.source == Source::BotPass || copper_creds::origin::owns(c, origin)
}

/// Why a shared login wasn't used: it is saved for another address.
fn saved_elsewhere(host: &str, origin: &str, logins: &[&Credential]) -> String {
    let mut saved: Vec<&str> = logins
        .iter()
        .flat_map(|c| c.sites.iter().map(String::as_str))
        .collect();
    saved.sort_unstable();
    saved.dedup();
    saved.truncate(4);
    format!(
        "the shared login for {host} is saved for {}, not {origin} — agents fill a login only on the exact address it was saved for. Save it for this address (add the address to the item) to use it here",
        saved.join(", ")
    )
}

/// Secret redaction: the page's word on an agent's sign-in
/// (`forms/sign_in.js`) as the agent's error, or `None` when the password
/// went in. None says the password; each says nothing of it was left.
pub fn sign_in_refusal(word: &str, account: &str) -> Option<String> {
    Some(match word {
        "ok" => return None,
        "not-password" => "refused: the box the password would go into is not a password box, so nothing was filled. Go to the step of the sign-in that shows the password box, take a fresh browser_snapshot and call again".into(),
        "hidden-password" => "refused: the page's password box is not shown yet (the sign-in asks for the account first), so nothing was filled. Enter the account and continue to the password step, take a fresh browser_snapshot and call again".into(),
        "other-account" => format!("refused: the account box on the page holds a different account than {account}, so nothing was filled. Put {account} in the account box (or pass account: for the one on the page) and call again"),
        "echoed" => "refused: the page copied the password out of its password box, so Copper emptied every copy and the box itself. Take a fresh browser_snapshot, check the account box holds the right account, then call again".into(),
        _ => "sign-in fields not found on the page".into(),
    })
}

/// The kinds of value an agent never reads back: a card's number and code,
/// a password, a code, a social security number.
pub const SECRET_KINDS: [&str; 5] = ["password", "otp", "cardNumber", "cardCode", "ssn"];

/// Remember the secret ones of `values` for `tab` before they are filled.
pub fn remember_secret_values(tab: &TabId, values: &BTreeMap<String, String>) {
    for (kind, value) in values {
        if SECRET_KINDS.contains(&kind.as_str()) {
            copper_mcp::secrets::remember(tab, value);
        }
    }
}

pub async fn sign_in<U: FormsUi>(
    shell: Shell<U::Platform>,
    tab: TabId,
    args: Map<String, Value>,
) -> Result<Value, String> {
    let Target { page, host, origin } = target(&shell, &tab, "sign-in")?;
    let what = args
        .get("what")
        .and_then(Value::as_str)
        .unwrap_or("password")
        .to_lowercase();
    if what != "password" && what != "otp" {
        return Err("what must be password or otp".into());
    }
    let vault = super::vault().ok_or("passwords are not available in this build")?;
    let v = vault.clone();
    let h = host.clone();
    let o = origin.clone();
    let all = rt::run(async move { v.candidates(&h, Some(&o)).await })
        .await
        .unwrap_or_default();
    let wider: Vec<&Credential> = all.iter().filter(|c| agent_allowed(&shell, c)).collect();
    // Only a login saved for this page's exact address (scheme, host and port).
    let permitted: Vec<Credential> = wider
        .iter()
        .filter(|c| owned(c, &origin))
        .map(|c| (*c).clone())
        .collect();
    if permitted.is_empty() {
        if !wider.is_empty() {
            return Err(saved_elsewhere(&host, &origin, &wider));
        }
        if !all.is_empty() {
            return Err(format!(
                "credential for {host} exists but is not shared with agents — enable it in Settings › Passwords › Agent access"
            ));
        }
        let snap = vault.snapshot();
        if snap.bw_locked() {
            return Err(BW_LOCKED.into());
        }
        if snap.op_locked() {
            return Err(OP_LOCKED.into());
        }
        if snap.bp_locked() {
            return Err(BP_LOCKED.into());
        }
        return Err(format!("no saved credential for {host}"));
    }
    let account = args
        .get("account")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let chosen = match account {
        Some(a) => {
            let found = permitted
                .iter()
                .find(|c| c.user == a)
                .or_else(|| permitted.iter().find(|c| c.user.eq_ignore_ascii_case(a)));
            let named: Vec<&Credential> = wider
                .iter()
                .filter(|c| c.user.eq_ignore_ascii_case(a))
                .copied()
                .collect();
            if found.is_none() && !named.is_empty() {
                return Err(saved_elsewhere(&host, &origin, &named));
            }
            if found.is_none() && all.iter().any(|c| c.user.eq_ignore_ascii_case(a)) {
                return Err(format!(
                    "account {a} for {host} is not shared with agents — enable it in Settings › Passwords › Agent access"
                ));
            }
            Some(found.cloned().ok_or_else(|| {
                format!(
                    "no shared account named {a} for {host}; saved: {}",
                    permitted
                        .iter()
                        .map(|c| c.user.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?)
        }
        None if permitted.len() == 1 => Some(permitted[0].clone()),
        None => None,
    };
    let Some(credential) = chosen else {
        return Ok(json!({
            "candidates": permitted.iter().map(|c| c.user.clone()).collect::<Vec<_>>(),
            "host": host,
        }));
    };
    let wants_submit = args.get("submit").and_then(Value::as_bool).unwrap_or(true);

    if what == "password" {
        // The field first, the secret second: a page with nothing to fill
        // never causes a read from the vault.
        let has = call(&page, &copper_pagejs::forms::has_password().at(&origin))
            .await
            .unwrap_or(false);
        if !has {
            return Err("sign-in fields not found on the page".into());
        }
        still_there(&page, &origin)?;
        let v = vault.clone();
        let id = credential.id.clone();
        let o = origin.clone();
        let secret = rt::run(async move { v.secret_at(&id, Some(&o)).await })
            .await
            .unwrap_or_else(|| Err("the vault did not answer".into()))
            .map_err(|e| locked_hint(&credential, e))?;
        // Secret redaction: remembered before the fill (an echo the page makes
        // the moment it lands is scrubbed), and put only into a real, shown
        // password box beside this account, only while it stays there.
        copper_mcp::secrets::remember(&tab, &secret);
        let word = call(
            &page,
            &copper_pagejs::forms::sign_in(&credential.user, &secret).at(&origin),
        )
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "no-fields".into());
        drop(secret);
        if let Some(refusal) = sign_in_refusal(&word, &credential.user) {
            return Err(refusal);
        }
        let submitted = wants_submit && submit(&page, &origin, false).await;
        let v = vault.clone();
        let c = credential.clone();
        rt::run(async move { v.touch(&c).await }).await;
        if let Some(f) = super::get::<U>() {
            f.signed_in_with(&tab, &credential);
        }
        return Ok(json!({
            "filled": true,
            "account": credential.user,
            "host": host,
            "submitted": submitted,
            "source": source_name(credential.source),
        }));
    }

    if !credential.has_totp {
        return Err("no one-time code for this account".into());
    }
    let has = call(&page, &copper_pagejs::forms::has_otp().at(&origin))
        .await
        .unwrap_or(false);
    if !has {
        return Err("no one-time-code field on the page".into());
    }
    still_there(&page, &origin)?;
    let v = vault.clone();
    let id = credential.id.clone();
    let o = origin.clone();
    let code = rt::run(async move { v.totp_at(&id, Some(&o)).await })
        .await
        .unwrap_or_else(|| Err("the vault did not answer".into()))
        .map_err(|e| locked_hint(&credential, e))?;
    copper_mcp::secrets::remember(&tab, &code);
    let filled = call(&page, &copper_pagejs::forms::fill_otp(&code).at(&origin)).await;
    drop(code);
    if filled.ok().flatten() != Some(true) {
        return Err("no one-time-code field on the page".into());
    }
    // A code page has no password box: its own form is sent.
    let submitted = wants_submit && submit(&page, &origin, true).await;
    Ok(json!({
        "filled": true,
        "account": credential.user,
        "what": "otp",
        "submitted": submitted,
    }))
}

/// The agent-only controls Jev may offer on `tab` (`Ultrafast.availability`):
/// SIGN_IN when the page has a password field and an account for its site
/// is shared with agents; AUTOFILL_CARD / AUTOFILL_IDENTITY when it has
/// such fields and a card / identity is shared. None in a private tab. The
/// vault is asked only about what the page has fields for.
pub async fn jev_extras<P: Platform>(shell: Shell<P>, tab: TabId) -> Extras {
    let (Ok(Target { page, host, origin }), Some(vault)) =
        (target(&shell, &tab, "Jev"), super::vault())
    else {
        return Extras::default();
    };
    let has = |c: copper_pagejs::Call<bool>| {
        let page = page.clone();
        let c = c.at(&origin);
        async move { call(&page, &c).await.unwrap_or(false) }
    };
    let mut out = Extras::default();
    if has(copper_pagejs::forms::has_password()).await {
        let v = vault.clone();
        let o = origin.clone();
        let all = rt::run(async move { v.candidates(&host, Some(&o)).await })
            .await
            .unwrap_or_default();
        out.sign_in = all
            .iter()
            .any(|c| agent_allowed(&shell, c) && owned(c, &origin));
    }
    if has(copper_pagejs::forms::has_fields("card")).await {
        let v = vault.clone();
        let cards = rt::run(async move { hinted(&v, v.cards().await, |c| &c.id).await })
            .await
            .unwrap_or_default();
        out.autofill_card = cards
            .iter()
            .any(|(c, hint)| autofill_allowed(&shell, &stable(&c.id), *hint));
    }
    if has(copper_pagejs::forms::has_fields("identity")).await {
        let v = vault.clone();
        let ids = rt::run(async move { hinted(&v, v.identities().await, |i| &i.id).await })
            .await
            .unwrap_or_default();
        out.autofill_identity = ids
            .iter()
            .any(|(i, hint)| autofill_allowed(&shell, &stable(&i.id), *hint));
    }
    out
}

/// Each card or identity with its agent hint (its Agents folder, tag or
/// deny field), which shares it as a login's does.
async fn hinted<T>(
    v: &super::vault::Vault,
    items: Vec<T>,
    id: impl Fn(&T) -> &String,
) -> Vec<(T, AgentHint)> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let hint = v.agent_hint(id(&item)).await;
        out.push((item, hint));
    }
    out
}

/// `bw:<id>` / `op:<id>`: the id a card, identity or field is shared under.
fn stable(id: &str) -> String {
    if id.starts_with("op:") {
        id.to_string()
    } else {
        format!("bw:{id}")
    }
}

fn from_vault(id: &str) -> &'static str {
    if id.starts_with("op:") {
        "onepassword"
    } else {
        "bitwarden"
    }
}

fn add(map: &mut BTreeMap<String, String>, k: &str, v: &str) {
    if !v.is_empty() {
        map.insert(k.into(), v.into());
    }
}

pub fn card_values(c: &AutofillCard) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    add(&mut m, "cardNumber", &c.number);
    add(&mut m, "cardName", &c.cardholder_name);
    add(&mut m, "cardExpMonth", &c.exp_month);
    add(&mut m, "cardExpYear", &c.exp_year);
    if !c.exp_month.is_empty() && !c.exp_year.is_empty() {
        let month = if c.exp_month.chars().count() == 1 {
            format!("0{}", c.exp_month)
        } else {
            c.exp_month.clone()
        };
        let year: String = if c.exp_year.chars().count() > 2 {
            c.exp_year
                .chars()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        } else {
            c.exp_year.clone()
        };
        m.insert("cardExp".into(), format!("{month}/{year}"));
    }
    add(&mut m, "cardCode", &c.code);
    add(&mut m, "cardBrand", &c.brand);
    m
}

fn card_label(c: &AutofillCard) -> String {
    c.label()
}

fn full_name(i: &AutofillIdentity) -> String {
    i.full_name()
}

pub fn identity_values(i: &AutofillIdentity) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    add(&mut m, "fullName", &full_name(i));
    add(&mut m, "firstName", &i.first_name);
    add(&mut m, "middleName", &i.middle_name);
    add(&mut m, "lastName", &i.last_name);
    add(&mut m, "username", &i.username);
    add(&mut m, "company", &i.company);
    add(&mut m, "email", &i.email);
    add(&mut m, "phone", &i.phone);
    add(&mut m, "address1", &i.address1);
    add(&mut m, "address2", &i.address2);
    add(&mut m, "address3", &i.address3);
    add(&mut m, "city", &i.city);
    add(&mut m, "state", &i.state);
    add(&mut m, "postalCode", &i.postal_code);
    add(&mut m, "country", &i.country);
    add(&mut m, "ssn", &i.ssn);
    add(&mut m, "passportNumber", &i.passport_number);
    add(&mut m, "licenseNumber", &i.license_number);
    m
}

/// For a refusal: the page left the form's origin during the call.
fn gone<V: PageView>(page: &V, origin: &str) -> String {
    if page.url().as_deref().and_then(site_origin).as_deref() == Some(origin) {
        return String::new();
    }
    format!(
        " (the form's document at {origin} changed during the call — nothing was filled; take a fresh browser_snapshot and call again)"
    )
}

/// A card's or identity's values into the form, pinned to its origin; the
/// answer says how many boxes took one, never a value.
async fn fill_group<V: PageView>(
    tab: &TabId,
    page: &V,
    origin: &str,
    group: &str,
    values: BTreeMap<String, String>,
    should_submit: bool,
) -> Result<(u32, bool), String> {
    if !call(page, &copper_pagejs::forms::has_fields(group).at(origin))
        .await
        .unwrap_or(false)
    {
        return Err(format!(
            "no {group} fields on the page{}",
            gone(page, origin)
        ));
    }
    remember_secret_values(tab, &values);
    let filled = call(page, &copper_pagejs::forms::fill_values(&values).at(origin))
        .await
        .ok()
        .flatten();
    drop(values);
    let filled = match (filled, gone(page, origin)) {
        (Some(n), _) => n,
        (None, gone) if !gone.is_empty() => {
            return Err(format!("no {group} fields on the page{gone}"));
        }
        (None, _) => 0,
    };
    let submitted = should_submit && submit(page, origin, false).await;
    Ok((filled, submitted))
}

pub async fn autofill<P: Platform>(
    shell: Shell<P>,
    tab: TabId,
    args: Map<String, Value>,
) -> Result<Value, String> {
    // The form's page and origin, fixed here, once: every read, fill and
    // submit below goes there and only while the page is still of it.
    let Target { page, host, origin } = target(&shell, &tab, "autofill")?;
    let vault = super::vault().ok_or("autofill is not available in this build")?;
    let snap = vault.snapshot();
    if !snap.bw_unlocked() && !snap.op_unlocked() {
        let bw_absent = !matches!(
            snap.bw_state,
            Some(copper_creds::bitwarden::BwState::Locked { .. })
                | Some(copper_creds::bitwarden::BwState::Unlocked { .. })
        );
        if snap.op_locked() && bw_absent {
            return Err(OP_LOCKED.into());
        }
        return Err(BW_LOCKED.into());
    }
    let kind = args
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("kind is required (card, identity, or field)")?
        .trim()
        .to_lowercase();
    if !["card", "identity", "field"].contains(&kind.as_str()) {
        return Err("kind must be card, identity, or field".into());
    }
    let should_submit = args.get("submit").and_then(Value::as_bool).unwrap_or(false);
    let first = args.get("first").and_then(Value::as_bool).unwrap_or(false);
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string);

    match kind.as_str() {
        "card" => {
            let v = vault.clone();
            let all = rt::run(async move { hinted(&v, v.cards().await, |c| &c.id).await })
                .await
                .unwrap_or_default();
            let permitted: Vec<AutofillCard> = all
                .iter()
                .filter(|(c, hint)| autofill_allowed(&shell, &stable(&c.id), *hint))
                .map(|(c, _)| c.clone())
                .collect();
            if permitted.is_empty() {
                return Err(if all.is_empty() {
                    "no saved card".into()
                } else {
                    "card exists but is not shared with agents — enable it in Settings › Passwords › Agent access".into()
                });
            }
            let matches: Vec<AutofillCard> = match &name {
                Some(n) => {
                    let m: Vec<AutofillCard> = permitted
                        .iter()
                        .filter(|c| {
                            [
                                c.name.clone(),
                                card_label(c),
                                c.last4(),
                                c.cardholder_name.clone(),
                            ]
                            .iter()
                            .any(|x| x.eq_ignore_ascii_case(n))
                        })
                        .cloned()
                        .collect();
                    if m.is_empty() {
                        return Err(format!(
                            "no shared card named {n}; saved: {}",
                            permitted
                                .iter()
                                .map(|c| c.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    m
                }
                None => permitted,
            };
            if !first && matches.len() != 1 {
                return Ok(
                    json!({ "candidates": matches.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), "kind": kind }),
                );
            }
            let card = matches.into_iter().next().ok_or("no saved card")?;
            let (filled, submitted) = fill_group(
                &tab,
                &page,
                &origin,
                "card",
                card_values(&card),
                should_submit,
            )
            .await?;
            let mut out = json!({
                "filled": filled, "kind": kind, "name": card.name, "submitted": submitted,
                "origin": origin, "source": from_vault(&card.id),
            });
            if filled == 0 {
                out["note"] = "the card fields already hold values; nothing overwritten".into();
            }
            Ok(out)
        }
        "identity" => {
            let v = vault.clone();
            let all = rt::run(async move { hinted(&v, v.identities().await, |i| &i.id).await })
                .await
                .unwrap_or_default();
            let permitted: Vec<AutofillIdentity> = all
                .iter()
                .filter(|(i, hint)| autofill_allowed(&shell, &stable(&i.id), *hint))
                .map(|(i, _)| i.clone())
                .collect();
            if permitted.is_empty() {
                return Err(if all.is_empty() {
                    "no saved identity".into()
                } else {
                    "identity exists but is not shared with agents — enable it in Settings › Passwords › Agent access".into()
                });
            }
            let matches: Vec<AutofillIdentity> = match &name {
                Some(n) => {
                    let m: Vec<AutofillIdentity> = permitted
                        .iter()
                        .filter(|i| {
                            [i.name.clone(), full_name(i), i.email.clone()]
                                .iter()
                                .any(|x| x.eq_ignore_ascii_case(n))
                        })
                        .cloned()
                        .collect();
                    if m.is_empty() {
                        return Err(format!(
                            "no shared identity named {n}; saved: {}",
                            permitted
                                .iter()
                                .map(|i| i.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    m
                }
                None => permitted,
            };
            if !first && matches.len() != 1 {
                return Ok(
                    json!({ "candidates": matches.iter().map(|i| i.name.clone()).collect::<Vec<_>>(), "kind": kind }),
                );
            }
            let who = matches.into_iter().next().ok_or("no saved identity")?;
            let (filled, submitted) = fill_group(
                &tab,
                &page,
                &origin,
                "identity",
                identity_values(&who),
                should_submit,
            )
            .await?;
            let mut out = json!({
                "filled": filled, "kind": kind, "name": who.name, "submitted": submitted,
                "origin": origin, "source": from_vault(&who.id),
            });
            if filled == 0 {
                out["note"] = "the identity fields already hold values; nothing overwritten".into();
            }
            Ok(out)
        }
        _ => {
            let name = name.ok_or("name is required for field")?;
            let v = vault.clone();
            let (h, n, o) = (host.clone(), name.clone(), origin.clone());
            let all = rt::run(async move { v.fields(&h, &n, &o).await })
                .await
                .unwrap_or_default();
            // A field belongs to a login, as a password does: only one saved
            // for this page's exact address (scheme, host and port).
            let owned: Vec<&super::vault::FieldHit> = all.iter().filter(|f| f.owned).collect();
            let Some(hit) = owned
                .iter()
                .find(|f| autofill_allowed(&shell, &f.id, f.hint))
            else {
                return Err(if !owned.is_empty() {
                    "field exists but is not shared with agents — enable it in Settings › Passwords › Agent access".into()
                } else if !all.is_empty() {
                    format!(
                        "the field {name} is saved with a login for another address than {origin} — agents fill it only on the exact address (scheme, host and port) the login was saved for. Add this address to the item to use it here"
                    )
                } else {
                    format!("no saved field named {name}")
                });
            };
            // A vault field may be hidden (a PIN, a key) and the vault does not
            // say here: every one an agent fills is treated as a secret.
            copper_mcp::secrets::remember(&tab, &hit.value);
            let filled = call(
                &page,
                &copper_pagejs::forms::fill_field_secret(&name, &hit.value).at(&origin),
            )
            .await
            .ok()
            .flatten()
            .unwrap_or(false);
            if !filled {
                let focused = call(
                    &page,
                    &copper_pagejs::forms::fill_focused_secret(&hit.value).at(&origin),
                )
                .await
                .ok()
                .flatten()
                .unwrap_or(false);
                if !focused {
                    return Err(format!(
                        "no field named {name} on the page{}",
                        gone(&page, &origin)
                    ));
                }
            }
            Ok(json!({
                "filled": 1, "kind": "field", "name": hit.name, "item": hit.item, "origin": origin,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_and_identity_values() {
        let c = AutofillCard {
            id: "i".into(),
            name: "Visa".into(),
            cardholder_name: "Ann".into(),
            brand: "Visa".into(),
            exp_month: "3".into(),
            exp_year: "2030".into(),
            number: "4111 1111 1111 1111".into(),
            code: "321".into(),
        };
        let v = card_values(&c);
        assert_eq!(v["cardExp"], "03/30");
        assert_eq!(card_label(&c), "Visa •••• 1111");
        let i = AutofillIdentity {
            first_name: "Ann".into(),
            last_name: "Example".into(),
            ..Default::default()
        };
        assert_eq!(identity_values(&i)["fullName"], "Ann Example");
        assert_eq!(stable("op:x"), "op:x");
        assert_eq!(stable("x"), "bw:x");
    }
}
