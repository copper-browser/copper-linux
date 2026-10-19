//! The canvases this device knows (Mac: `Canvases` in `Canvas.swift`):
//! `canvas/canvases.json`. One Personal canvas always (first), local ones
//! made signed out, and the shared ones of each account that signed in here.
//! The server's list (`GET /v1/canvases`) is read at launch, at sign-in,
//! when the event stream says canvases changed, and when Canvas asks.

use copper_cloud::canvas::CanvasInfo;
use copper_core::canvas::{CanvasEntry, PERSONAL_CANVAS_ID};
use copper_core::date::IsoDate;
use std::cell::RefCell;

thread_local! {
    static ALL: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

#[derive(Default)]
pub struct Registry {
    pub all: Vec<CanvasEntry>,
    /// The account the local Personal history was first given to.
    pub personal_account: Option<String>,
    /// The server's list has been read this run.
    pub refreshed: bool,
    pub problem: Option<String>,
}

pub fn is_personal(e: &CanvasEntry) -> bool {
    e.kind == "personal"
}
pub fn is_shared(e: &CanvasEntry) -> bool {
    e.kind == "shared"
}
pub fn is_local(e: &CanvasEntry) -> bool {
    e.kind == "local"
}
/// Can rename / delete / invite rather than only leave.
pub fn is_owner(e: &CanvasEntry) -> bool {
    !is_shared(e) || e.role.as_deref().is_none_or(|r| r == "owner")
}

fn account() -> Option<String> {
    crate::cloud::get()
        .and_then(|c| c.client.account())
        .map(|a| a.user_id.to_ascii_lowercase())
}

/// Signed in on a linked cloud: sharing is possible.
pub fn cloud_ready() -> bool {
    crate::cloud::get().is_some_and(|c| c.client.is_linked() && c.client.is_signed_in())
}

/// The Personal canvas may use its room: chosen in Settings › Cloud.
pub fn personal_syncs() -> bool {
    crate::cloud::get().is_some_and(|c| c.sync.syncs(copper_cloud::Domain::Canvas))
}

/// Run `f` with the registry (loaded on first use).
pub fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    ALL.with(|a| {
        let mut slot = a.borrow_mut();
        if slot.is_none() {
            *slot = Some(load());
        }
        f(slot.as_mut().expect("loaded"))
    })
}

fn load() -> Registry {
    let mut r = Registry::default();
    if let Some(store) = super::store() {
        match store.load::<Vec<CanvasEntry>>() {
            Ok(l) => r.all = l.value,
            Err(e) => tracing::error!(error = %e, "canvases.json"),
        }
        r.personal_account = read_me(&store);
    }
    ensure_personal(&mut r.all);
    r
}

fn me_path(store: &copper_core::Store) -> std::path::PathBuf {
    store.paths().data.join("canvas").join("me.json")
}

fn read_me(store: &copper_core::Store) -> Option<String> {
    let data = std::fs::read(me_path(store)).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&data).ok()?;
    v.get("personalAccount")
        .and_then(|a| a.as_str())
        .map(str::to_string)
}

fn write_me(personal_account: &str) {
    if let Some(store) = super::store() {
        let body = serde_json::json!({"personalAccount": personal_account});
        let _ = copper_core::store::write_atomic(
            &me_path(&store),
            body.to_string().as_bytes(),
            true,
            false,
        );
    }
}

fn ensure_personal(all: &mut Vec<CanvasEntry>) {
    if let Some(i) = all.iter().position(is_personal) {
        all[i].id = PERSONAL_CANVAS_ID.into();
        all[i].name = "Personal".into();
        let mut seen = false;
        all.retain(|e| {
            if !is_personal(e) {
                return true;
            }
            let keep = !seen;
            seen = true;
            keep
        });
        if let Some(i) = all.iter().position(is_personal)
            && i != 0
        {
            let p = all.remove(i);
            all.insert(0, p);
        }
    } else {
        let now = IsoDate::now();
        all.insert(
            0,
            CanvasEntry {
                id: PERSONAL_CANVAS_ID.into(),
                name: "Personal".into(),
                kind: "personal".into(),
                created_at: now,
                updated_at: now,
                ..Default::default()
            },
        );
    }
}

/// Write `canvases.json` (debounced by the saver).
pub fn save() {
    let list = with(|r| r.all.clone());
    super::save_list(list);
}

pub fn entry(id: &str) -> Option<CanvasEntry> {
    with(|r| r.all.iter().find(|e| e.id == id).cloned())
}

pub fn personal() -> CanvasEntry {
    with(|r| {
        ensure_personal(&mut r.all);
        r.all[0].clone()
    })
}

/// What the lists show: Personal, the local ones and — signed in — the
/// shared ones of the account signed in now.
pub fn visible() -> Vec<CanvasEntry> {
    let me = account();
    with(|r| {
        r.all
            .iter()
            .filter(|e| !is_shared(e) || (me.is_some() && e.account == me))
            .cloned()
            .collect()
    })
}

/// By id, by remote id, or by name (any case) — what an agent passes.
pub fn find(key: &str) -> Option<CanvasEntry> {
    let wanted = key.trim();
    if let Some(e) = with(|r| {
        r.all
            .iter()
            .find(|e| e.id == wanted || e.remote_id.as_deref() == Some(wanted))
            .cloned()
    }) {
        return Some(e);
    }
    if wanted.eq_ignore_ascii_case("personal") {
        return Some(personal());
    }
    let lower = wanted.to_lowercase();
    visible()
        .into_iter()
        .find(|e| e.name.to_lowercase() == lower)
}

/// Who the user is on a canvas: the account, or this device's own persona.
pub fn identity() -> (String, String, String) {
    if let Some(a) = crate::cloud::get().and_then(|c| c.client.account()) {
        let id = a.user_id.to_ascii_lowercase();
        let name = if a.display_name.is_empty() {
            a.email.clone()
        } else {
            a.display_name.clone()
        };
        let color = super::stable_color(&id);
        return (id, name, color);
    }
    let device = crate::cloud::get()
        .map(|c| c.client.device_id().to_ascii_lowercase())
        .unwrap_or_else(|| "this-device".into());
    let id = format!("local:{device}");
    let name = super::user_name();
    let color = super::stable_color(&id);
    (id, name, color)
}

/// Where a canvas's history is kept (`canvas/<key>/`): Personal's moves to
/// `personal-<account>` once a second account signs in here.
pub fn store_key(e: &CanvasEntry) -> String {
    if !is_personal(e) {
        return e.id.clone();
    }
    let bound = with(|r| r.personal_account.clone());
    match (account(), bound) {
        (Some(a), Some(b)) if a != b => format!("personal-{a}"),
        _ => e.id.clone(),
    }
}

/// The room a canvas relays to, when it has one and the cloud is up — only
/// for the account signed in now; Personal only while its sync is chosen.
pub fn room_for(e: &CanvasEntry) -> Option<String> {
    if !cloud_ready() {
        return None;
    }
    let remote = e.remote_id.clone()?;
    if (is_shared(e) || is_personal(e)) && (account().is_none() || e.account != account()) {
        return None;
    }
    if is_personal(e) && !personal_syncs() {
        return None;
    }
    Some(remote)
}

/// The first time the local Personal history goes to an account.
pub fn bind_personal() {
    let Some(a) = account() else { return };
    let bound = with(|r| {
        if r.personal_account.is_none() {
            r.personal_account = Some(a.clone());
            true
        } else {
            false
        }
    });
    if bound {
        write_me(&a);
    }
}

pub fn clean(name: &str) -> String {
    let t: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(120).collect();
    if t.is_empty() {
        "Untitled canvas".into()
    } else {
        t
    }
}

/// A canvas on this device only.
pub fn create_local(name: &str) -> CanvasEntry {
    let now = IsoDate::now();
    let e = CanvasEntry {
        id: copper_core::ids::new_uuid().to_ascii_lowercase(),
        name: clean(name),
        kind: "local".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    with(|r| r.all.push(e.clone()));
    save();
    e
}

/// The page saw a change: the list sorts recent first (at most every 30 s).
pub fn touched(id: &str) {
    let changed = with(|r| {
        let Some(e) = r.all.iter_mut().find(|e| e.id == id) else {
            return false;
        };
        let now = IsoDate::now();
        if now.unix() - e.updated_at.unix() <= 30 {
            return false;
        }
        e.updated_at = now;
        true
    });
    if changed {
        save();
    }
}

fn date(text: &Option<String>) -> Option<IsoDate> {
    text.as_deref().and_then(IsoDate::parse)
}

fn owner_name(info: &CanvasInfo) -> Option<String> {
    info.owner.as_ref().and_then(|p| {
        p.display_name
            .clone()
            .filter(|n| !n.is_empty())
            .or_else(|| p.email.clone())
    })
}

/// One row from the server, added or updated. Returns its entry.
pub fn upsert(info: &CanvasInfo, account: &str) -> Option<CanvasEntry> {
    if info.id.is_empty() {
        return None;
    }
    let updated = date(&info.updated_at).unwrap_or_else(IsoDate::now);
    let out = with(|r| {
        if let Some(e) = r.all.iter_mut().find(|e| e.id == info.id) {
            e.name = if info.name.is_empty() {
                e.name.clone()
            } else {
                info.name.clone()
            };
            e.kind = "shared".into();
            e.remote_id = Some(info.id.clone());
            e.role = Some(info.role.clone()).filter(|r| !r.is_empty());
            e.owner = owner_name(info);
            e.members = info.member_count;
            e.account = Some(account.to_string());
            if updated.unix() > e.updated_at.unix() {
                e.updated_at = updated;
            }
            e.clone()
        } else {
            let e = CanvasEntry {
                id: info.id.clone(),
                name: if info.name.is_empty() {
                    "Untitled canvas".into()
                } else {
                    info.name.clone()
                },
                kind: "shared".into(),
                remote_id: Some(info.id.clone()),
                created_at: date(&info.created_at).unwrap_or(updated),
                updated_at: updated,
                role: Some(info.role.clone()).filter(|r| !r.is_empty()),
                owner: owner_name(info),
                members: info.member_count,
                account: Some(account.to_string()),
                ..Default::default()
            };
            r.all.push(e.clone());
            e
        }
    });
    save();
    Some(out)
}

/// What a read of the server's list changed.
#[derive(Default, Debug)]
pub struct Adopted {
    pub renamed: Vec<String>,
    pub gone: Vec<String>,
    pub seen: Vec<String>,
}

/// Take the server's list for `account`.
pub fn adopt(rows: &[CanvasInfo], account: &str) -> Adopted {
    let mut out = Adopted::default();
    with(|r| {
        for row in rows {
            if row.id.is_empty() {
                continue;
            }
            if row.kind == "personal" {
                if let Some(p) = r.all.iter_mut().find(|e| is_personal(e)) {
                    p.remote_id = Some(row.id.clone());
                    p.account = Some(account.to_string());
                }
                continue;
            }
            out.seen.push(row.id.clone());
            if let Some(e) = r.all.iter().find(|e| e.id == row.id)
                && !row.name.is_empty()
                && e.name != row.name
            {
                out.renamed.push(row.id.clone());
            }
        }
    });
    for row in rows
        .iter()
        .filter(|r| r.kind != "personal" && !r.id.is_empty())
    {
        upsert(row, account);
    }
    with(|r| {
        let gone: Vec<String> = r
            .all
            .iter()
            .filter(|e| {
                is_shared(e) && e.account.as_deref() == Some(account) && !out.seen.contains(&e.id)
            })
            .map(|e| e.id.clone())
            .collect();
        r.all.retain(|e| !gone.contains(&e.id));
        out.gone = gone;
        r.refreshed = true;
        r.problem = None;
    });
    save();
    out
}

/// Gone from this device (deleted, left): the entry goes.
pub fn forget(id: &str) {
    with(|r| r.all.retain(|e| e.id != id));
    save();
}

pub fn rename_local(id: &str, name: &str) {
    with(|r| {
        if let Some(e) = r.all.iter_mut().find(|e| e.id == id) {
            e.name = clean(name);
            e.updated_at = IsoDate::now();
        }
    });
    save();
}

pub fn set_problem(text: Option<String>) {
    with(|r| r.problem = text);
}

pub fn refreshed() -> bool {
    with(|r| r.refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn personal_is_first_and_single() {
        let now = IsoDate::now();
        let mk = |id: &str, kind: &str| CanvasEntry {
            id: id.into(),
            name: id.into(),
            kind: kind.into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        let mut all = vec![mk("a", "local"), mk("p1", "personal"), mk("p2", "personal")];
        ensure_personal(&mut all);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, "personal");
        assert_eq!(all[0].name, "Personal");
        let mut none = vec![mk("a", "local")];
        ensure_personal(&mut none);
        assert_eq!(none[0].id, "personal");
        assert_eq!(clean("  a   b "), "a b");
        assert_eq!(clean(""), "Untitled canvas");
    }
}
