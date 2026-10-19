//! `browser_sign_in` / `browser_autofill` (Mac `Fork/MCP/SignIn.swift`,
//! `AgentAutofill.swift`). A saved account the user shared with agents is
//! filled into the page in-process; the secret goes from the vault into the
//! page's fill script and never into the result. The agents lane calls these
//! with the tab it resolved; signatures are fixed.

use super::watch::call;
use super::{FormsUi, agent_allowed, agent_allowed_id, rt, site_host};
use crate::shell::Shell;
use crate::{PageOf, Platform};
use copper_core::TabId;
use copper_creds::credential::{AutofillCard, AutofillIdentity, Credential, Source};
use copper_engine::PageView;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

struct Target<V> {
    page: V,
    host: String,
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
    Ok(Target { page, host })
}

fn source_name(s: Source) -> &'static str {
    match s {
        Source::Bitwarden => "bitwarden",
        Source::OnePassword => "onepassword",
        Source::Keychain => "keychain",
    }
}

const BW_LOCKED: &str = "bitwarden is locked — unlock it in Settings › Passwords";
const OP_LOCKED: &str = "1password is locked — unlock it in Settings › Passwords";

fn locked_hint(c: &Credential, e: String) -> String {
    let Some(v) = super::vault() else { return e };
    let snap = v.snapshot();
    match c.source {
        Source::Bitwarden if snap.bw_locked() => BW_LOCKED.into(),
        Source::OnePassword if !snap.op_unlocked() => OP_LOCKED.into(),
        _ => e,
    }
}

async fn submit<V: PageView>(page: &V) -> bool {
    call(page, &copper_pagejs::forms::submit())
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

pub async fn sign_in<U: FormsUi>(
    shell: Shell<U::Platform>,
    tab: TabId,
    args: Map<String, Value>,
) -> Result<Value, String> {
    let Target { page, host } = target(&shell, &tab, "sign-in")?;
    let vault = super::vault().ok_or("passwords are not available in this build")?;
    let v = vault.clone();
    let h = host.clone();
    let all = rt::run(async move { v.candidates(&h).await })
        .await
        .unwrap_or_default();
    let permitted: Vec<Credential> = all
        .iter()
        .filter(|c| agent_allowed(&shell, c))
        .cloned()
        .collect();
    if permitted.is_empty() {
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
        return Err(format!("no saved credential for {host}"));
    }
    let account = args
        .get("account")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty());
    let chosen = match account {
        Some(a) => Some(
            permitted
                .iter()
                .find(|c| c.user == a)
                .or_else(|| permitted.iter().find(|c| c.user.eq_ignore_ascii_case(a)))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no shared account named {a} for {host}; saved: {}",
                        permitted
                            .iter()
                            .map(|c| c.user.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?,
        ),
        None if permitted.len() == 1 => Some(permitted[0].clone()),
        None => None,
    };
    let Some(credential) = chosen else {
        return Ok(json!({
            "candidates": permitted.iter().map(|c| c.user.clone()).collect::<Vec<_>>(),
            "host": host,
        }));
    };
    let what = args
        .get("what")
        .and_then(Value::as_str)
        .unwrap_or("password")
        .to_lowercase();
    if what != "password" && what != "otp" {
        return Err("what must be password or otp".into());
    }
    let wants_submit = args.get("submit").and_then(Value::as_bool).unwrap_or(true);

    if what == "password" {
        // The field first, the secret second: a page with nothing to fill
        // never causes a read from the vault.
        let has = call(&page, &copper_pagejs::forms::has_password())
            .await
            .unwrap_or(false);
        if !has {
            return Err("sign-in fields not found on the page".into());
        }
        let v = vault.clone();
        let id = credential.id.clone();
        let secret = rt::run(async move { v.secret(&id).await })
            .await
            .unwrap_or_else(|| Err("the vault did not answer".into()))
            .map_err(|e| locked_hint(&credential, e))?;
        let filled = call(
            &page,
            &copper_pagejs::forms::fill_login(&credential.user, &secret),
        )
        .await;
        drop(secret);
        if filled.ok().flatten() != Some(true) {
            return Err("sign-in fields not found on the page".into());
        }
        let submitted = if wants_submit {
            submit(&page).await
        } else {
            false
        };
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
    let has = call(&page, &copper_pagejs::forms::has_otp())
        .await
        .unwrap_or(false);
    if !has {
        return Err("no one-time-code field on the page".into());
    }
    let v = vault.clone();
    let id = credential.id.clone();
    let code = rt::run(async move { v.totp(&id).await })
        .await
        .unwrap_or_else(|| Err("the vault did not answer".into()))
        .map_err(|e| locked_hint(&credential, e))?;
    let filled = call(&page, &copper_pagejs::forms::fill_otp(&code)).await;
    drop(code);
    if filled.ok().flatten() != Some(true) {
        return Err("no one-time-code field on the page".into());
    }
    let submitted = if wants_submit {
        submit(&page).await
    } else {
        false
    };
    Ok(json!({
        "filled": true,
        "account": credential.user,
        "what": "otp",
        "submitted": submitted,
    }))
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
    let last4 = c.last4();
    let tail = if last4.is_empty() {
        String::new()
    } else {
        format!("•••• {last4}")
    };
    [c.brand.as_str(), tail.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

fn full_name(i: &AutofillIdentity) -> String {
    [&i.first_name, &i.middle_name, &i.last_name]
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
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

pub async fn autofill<P: Platform>(
    shell: Shell<P>,
    tab: TabId,
    args: Map<String, Value>,
) -> Result<Value, String> {
    let Target { page, host } = target(&shell, &tab, "autofill")?;
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
            let all = rt::run(async move { v.cards().await })
                .await
                .unwrap_or_default();
            let permitted: Vec<AutofillCard> = all
                .iter()
                .filter(|c| agent_allowed_id(&shell, &stable(&c.id)))
                .cloned()
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
            if !call(&page, &copper_pagejs::forms::has_fields("card"))
                .await
                .unwrap_or(false)
            {
                return Err("no card fields on the page".into());
            }
            let filled = call(
                &page,
                &copper_pagejs::forms::fill_values(&card_values(&card)),
            )
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
            let submitted = if should_submit {
                submit(&page).await
            } else {
                false
            };
            let mut out = json!({
                "filled": filled, "kind": kind, "name": card.name, "submitted": submitted,
                "source": from_vault(&card.id),
            });
            if filled == 0 {
                out["note"] = "the card fields already hold values; nothing overwritten".into();
            }
            Ok(out)
        }
        "identity" => {
            let v = vault.clone();
            let all = rt::run(async move { v.identities().await })
                .await
                .unwrap_or_default();
            let permitted: Vec<AutofillIdentity> = all
                .iter()
                .filter(|i| agent_allowed_id(&shell, &stable(&i.id)))
                .cloned()
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
            if !call(&page, &copper_pagejs::forms::has_fields("identity"))
                .await
                .unwrap_or(false)
            {
                return Err("no identity fields on the page".into());
            }
            let filled = call(
                &page,
                &copper_pagejs::forms::fill_values(&identity_values(&who)),
            )
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
            let submitted = if should_submit {
                submit(&page).await
            } else {
                false
            };
            let mut out = json!({
                "filled": filled, "kind": kind, "name": who.name, "submitted": submitted,
                "source": from_vault(&who.id),
            });
            if filled == 0 {
                out["note"] = "the identity fields already hold values; nothing overwritten".into();
            }
            Ok(out)
        }
        _ => {
            let name = name.ok_or("name is required for field")?;
            let v = vault.clone();
            let (h, n) = (host.clone(), name.clone());
            let all = rt::run(async move { v.fields(&h, &n).await })
                .await
                .unwrap_or_default();
            let Some((_, item, field, value)) = all
                .iter()
                .find(|(id, ..)| agent_allowed_id(&shell, id))
                .cloned()
            else {
                return Err(if all.is_empty() {
                    format!("no saved field named {name}")
                } else {
                    "field exists but is not shared with agents — enable it in Settings › Passwords › Agent access".into()
                });
            };
            let filled = call(&page, &copper_pagejs::forms::fill_field(&name, &value))
                .await
                .ok()
                .flatten()
                .unwrap_or(false);
            if !filled {
                let focused = call(&page, &copper_pagejs::forms::fill_focused(&value))
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false);
                if !focused {
                    return Err(format!("no field named {name} on the page"));
                }
            }
            Ok(json!({ "filled": 1, "kind": "field", "name": field, "item": item }))
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
