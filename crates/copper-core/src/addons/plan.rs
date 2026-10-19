//! Applying `/v1/addons` (contract §6.3).
//!
//! [`plan`] compares what the cloud resolved with what is installed, slot by
//! slot:
//!
//! * **Removal** (the slot resolves to nothing, or to another addon):
//!   immediate, the slot falls back to its default.
//! * **Settings-only change** (same sha256): immediate; the platform fires
//!   the `settings` event.
//! * **New addon or new version**: staged (download → verify → stage) and
//!   kept as `pending` until the person presses Install
//!   ([`install_pending`]).
//!
//! An addon for a slot this build doesn't know, for another platform, for a
//! newer bridge, or with a malformed entry is ignored (reported in
//! [`Plan::ignored`] so the caller can log it once).

use super::bundle::{BundleError, is_sha256_hex};
use super::manifest::{valid_id, valid_version};
use super::resolved::{ResolvedAddon, ResolvedSet};
use super::slot::SlotId;
use super::state::{AddonState, AddonStore, Installed};
use super::{BRIDGE_VERSION, Platform};
use crate::date::IsoDate;
use serde_json::Value;
use std::collections::BTreeMap;

/// Why a resolved addon was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreReason {
    /// A slot this build doesn't know.
    UnknownSlot,
    /// Not meant for this platform.
    Platform,
    /// Written for a newer bridge than [`BRIDGE_VERSION`].
    BridgeTooNew,
    /// Bad id, version or sha256.
    Malformed,
    /// The person removed it here and the cloud hasn't changed since.
    RemovedHere,
    /// A second addon for a slot that already has one.
    Duplicate,
}

/// A resolved addon left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ignored {
    pub id: String,
    pub slot: String,
    pub reason: IgnoreReason,
}

/// What to do about one `/v1/addons` answer.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// Installed addons to remove now.
    pub remove: Vec<SlotId>,
    /// Installed addons whose settings change now.
    pub settings: Vec<(SlotId, Value)>,
    /// New addons or versions to download, verify and stage as pending.
    pub stage: Vec<ResolvedAddon>,
    /// Pending updates that no longer apply.
    pub drop_pending: Vec<SlotId>,
    /// Pending updates whose settings changed.
    pub pending_settings: Vec<(SlotId, Value)>,
    pub ignored: Vec<Ignored>,
}

/// Something that changed in the state, for the platform to act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The slot's addon went; show the default (or what is active now).
    Removed { slot: SlotId, id: String },
    /// New settings for the running addon: fire `settings`.
    Settings { slot: SlotId, settings: Value },
    /// A new addon or version now runs in the slot: reload it.
    Installed {
        slot: SlotId,
        id: String,
        version: String,
    },
    /// An update was staged: show "Update available".
    Pending { slot: SlotId },
    /// A staged update went away.
    PendingDropped { slot: SlotId },
    /// The person turned the slot's addon off or on here: reload the slot.
    Toggled {
        slot: SlotId,
        id: String,
        enabled: bool,
    },
}

impl Change {
    /// The slot it is about.
    pub fn slot(&self) -> SlotId {
        match self {
            Change::Removed { slot, .. }
            | Change::Settings { slot, .. }
            | Change::Installed { slot, .. }
            | Change::Pending { slot }
            | Change::PendingDropped { slot }
            | Change::Toggled { slot, .. } => *slot,
        }
    }
}

/// Compare `resolved` with `state` for `platform` (see the module docs).
pub fn plan(state: &AddonState, resolved: &ResolvedSet, platform: Platform) -> Plan {
    let mut out = Plan::default();
    let mut candidates: BTreeMap<SlotId, &ResolvedAddon> = BTreeMap::new();
    for addon in &resolved.addons {
        let ignore = |reason| Ignored {
            id: addon.id.clone(),
            slot: addon.slot.clone(),
            reason,
        };
        let Some(slot) = addon.slot_id() else {
            out.ignored.push(ignore(IgnoreReason::UnknownSlot));
            continue;
        };
        if !valid_id(&addon.id) || !valid_version(&addon.version) || !is_sha256_hex(&addon.sha256) {
            out.ignored.push(ignore(IgnoreReason::Malformed));
            continue;
        }
        if !addon.runs_on(platform) {
            out.ignored.push(ignore(IgnoreReason::Platform));
            continue;
        }
        if addon.bridge > BRIDGE_VERSION {
            out.ignored.push(ignore(IgnoreReason::BridgeTooNew));
            continue;
        }
        if state.removed.get(&addon.id) == Some(&resolved.revision) {
            out.ignored.push(ignore(IgnoreReason::RemovedHere));
            continue;
        }
        if candidates.contains_key(&slot) {
            out.ignored.push(ignore(IgnoreReason::Duplicate));
            continue;
        }
        candidates.insert(slot, addon);
    }
    for &slot in SlotId::ALL {
        let installed = state.installed.get(slot.as_str());
        let pending = state.pending.get(slot.as_str());
        let Some(c) = candidates.get(&slot) else {
            if installed.is_some() {
                out.remove.push(slot);
            }
            if pending.is_some() {
                out.drop_pending.push(slot);
            }
            continue;
        };
        let same = |i: &Installed| i.id == c.id && i.sha256.eq_ignore_ascii_case(&c.sha256);
        if let Some(i) = installed.filter(|i| same(i)) {
            if i.settings != c.settings {
                out.settings.push((slot, c.settings.clone()));
            }
            if pending.is_some() {
                out.drop_pending.push(slot);
            }
            continue;
        }
        if installed.is_some_and(|i| i.id != c.id) {
            out.remove.push(slot);
        }
        match pending {
            Some(p) if same(p) => {
                if p.settings != c.settings {
                    out.pending_settings.push((slot, c.settings.clone()));
                }
            }
            other => {
                if other.is_some() {
                    out.drop_pending.push(slot);
                }
                out.stage.push((*c).clone());
            }
        }
    }
    out
}

impl Plan {
    /// Nothing to change (ignored entries don't count).
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty()
            && self.settings.is_empty()
            && self.stage.is_empty()
            && self.drop_pending.is_empty()
            && self.pending_settings.is_empty()
    }

    /// Apply the immediate parts (removals, settings, dropped pending
    /// updates) to `state`, and forget local removals made at another
    /// `revision`. Staging is the caller's (it needs the network); so is
    /// saving and setting `state.revision` once everything applied.
    pub fn apply_immediate(&self, state: &mut AddonState, revision: &str) -> Vec<Change> {
        let mut changes = Vec::new();
        for &slot in &self.remove {
            if let Some(old) = state.uninstall(slot) {
                changes.push(Change::Removed { slot, id: old.id });
            }
        }
        for (slot, settings) in &self.settings {
            if let Some(i) = state.installed.get_mut(slot.as_str()) {
                i.settings = settings.clone();
                changes.push(Change::Settings {
                    slot: *slot,
                    settings: settings.clone(),
                });
            }
        }
        for &slot in &self.drop_pending {
            if state.pending.remove(slot.as_str()).is_some() {
                changes.push(Change::PendingDropped { slot });
            }
        }
        for (slot, settings) in &self.pending_settings {
            if let Some(p) = state.pending.get_mut(slot.as_str()) {
                p.settings = settings.clone();
            }
        }
        state.removed.retain(|_, r| r == revision);
        changes
    }
}

/// [`AddonState::active_for`]: what renders in `slot` (the developer folder,
/// else the installed addon unless turned off here).
pub fn active_for(slot: SlotId, state: &AddonState) -> Option<Installed> {
    state.active_for(slot).cloned()
}

/// What [`install_pending`] did.
#[derive(Debug, Default)]
pub struct InstallOutcome {
    pub changes: Vec<Change>,
    /// Slots whose staged bundle couldn't be installed; their pending entry
    /// is dropped and `state.revision` cleared so the next check stages
    /// again.
    pub failed: Vec<(SlotId, BundleError)>,
}

/// Install the pending updates for `slots` (all when `None`): re-verify the
/// staged bytes, unpack into `bundles/<id>/<sha256>/`, make them current
/// (the old one is kept in `previous` until the next launch), save
/// `state.json`, then delete the staged file and collect garbage. The
/// platform reloads the slot's view for each [`Change::Installed`].
pub fn install_pending(
    store: &AddonStore,
    state: &mut AddonState,
    slots: Option<&[SlotId]>,
) -> Result<InstallOutcome, crate::store::StoreError> {
    let mut out = InstallOutcome::default();
    let chosen: Vec<SlotId> = SlotId::ALL
        .iter()
        .copied()
        .filter(|s| slots.is_none_or(|list| list.contains(s)))
        .filter(|s| state.pending.contains_key(s.as_str()))
        .collect();
    for slot in chosen {
        let Some(pending) = state.pending.get(slot.as_str()).cloned() else {
            continue;
        };
        let unpacked = store
            .read_staged(&pending)
            .and_then(|(_, bundle)| store.unpack(&bundle).map(|dir| (dir, bundle)));
        match unpacked {
            Ok((dir, bundle)) => {
                state.uninstall(slot);
                state.pending.remove(slot.as_str());
                state.installed.insert(
                    slot.as_str().to_string(),
                    Installed {
                        dir,
                        installed_at: IsoDate::now(),
                        manifest: Some(bundle.manifest),
                        ..pending.clone()
                    },
                );
                out.changes.push(Change::Installed {
                    slot,
                    id: pending.id.clone(),
                    version: pending.version.clone(),
                });
            }
            Err(e) => {
                tracing::warn!(slot = %slot, error = %e, "staged addon not installed");
                state.pending.remove(slot.as_str());
                state.revision = None;
                out.changes.push(Change::PendingDropped { slot });
                out.failed.push((slot, e));
            }
        }
    }
    if !out.changes.is_empty() {
        store.save(state)?;
        store.gc(state);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::testkit;
    use serde_json::json;

    fn resolved(id: &str, sha: &str, settings: Value) -> ResolvedAddon {
        ResolvedAddon {
            id: id.into(),
            slot: "agent-sidebar".into(),
            name: "Example".into(),
            version: "1.0.0".into(),
            sha256: sha.into(),
            entry: "index.html".into(),
            bridge: 1,
            settings,
            ..Default::default()
        }
    }

    fn installed(id: &str, sha: &str, settings: Value) -> Installed {
        Installed {
            id: id.into(),
            version: "1.0.0".into(),
            sha256: sha.into(),
            dir: format!("bundles/{id}/{sha}"),
            settings,
            slot: "agent-sidebar".into(),
            ..Default::default()
        }
    }

    fn set(addons: Vec<ResolvedAddon>) -> ResolvedSet {
        ResolvedSet {
            revision: "r_2".into(),
            addons,
            ..Default::default()
        }
    }

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn fresh_install_is_staged() {
        let p = plan(
            &AddonState::default(),
            &set(vec![resolved("x.y", A, json!({}))]),
            Platform::Linux,
        );
        assert_eq!(p.stage.len(), 1);
        assert!(p.remove.is_empty() && p.settings.is_empty());
    }

    #[test]
    fn removal_is_immediate() {
        let mut s = AddonState::default();
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({})));
        s.pending
            .insert("agent-sidebar".into(), installed("x.y", B, json!({})));
        let p = plan(&s, &set(vec![]), Platform::Linux);
        assert_eq!(p.remove, vec![SlotId::AgentSidebar]);
        assert_eq!(p.drop_pending, vec![SlotId::AgentSidebar]);
        let changes = p.apply_immediate(&mut s, "r_2");
        assert_eq!(changes.len(), 2);
        assert!(s.installed.is_empty() && s.pending.is_empty());
        assert!(s.previous.contains_key("agent-sidebar"));
        assert!(active_for(SlotId::AgentSidebar, &s).is_none());
    }

    #[test]
    fn another_addon_replaces_now_and_stages() {
        let mut s = AddonState::default();
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({})));
        let p = plan(
            &s,
            &set(vec![resolved("z.w", B, json!({}))]),
            Platform::Linux,
        );
        assert_eq!(p.remove, vec![SlotId::AgentSidebar]);
        assert_eq!(p.stage[0].id, "z.w");
    }

    #[test]
    fn settings_only_is_immediate() {
        let mut s = AddonState::default();
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({"a": 1})));
        let p = plan(
            &s,
            &set(vec![resolved("x.y", A, json!({"a": 2}))]),
            Platform::Linux,
        );
        assert!(p.stage.is_empty() && p.remove.is_empty());
        assert_eq!(p.settings, vec![(SlotId::AgentSidebar, json!({"a": 2}))]);
        let changes = p.apply_immediate(&mut s, "r_2");
        assert!(matches!(changes[0], Change::Settings { .. }));
        assert_eq!(s.installed["agent-sidebar"].settings, json!({"a": 2}));
    }

    #[test]
    fn same_sha_is_a_no_op() {
        let mut s = AddonState::default();
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({"a": 1})));
        let p = plan(
            &s,
            &set(vec![resolved("x.y", A, json!({"a": 1}))]),
            Platform::Linux,
        );
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn new_version_replaces_an_older_pending() {
        let mut s = AddonState::default();
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({})));
        s.pending
            .insert("agent-sidebar".into(), installed("x.y", B, json!({})));
        let c = "c".repeat(64);
        let p = plan(
            &s,
            &set(vec![resolved("x.y", &c, json!({}))]),
            Platform::Linux,
        );
        assert_eq!(p.drop_pending, vec![SlotId::AgentSidebar]);
        assert_eq!(p.stage[0].sha256, c);
        assert!(p.remove.is_empty(), "the installed one keeps running");
        // The pending one again: nothing to stage.
        let p = plan(
            &s,
            &set(vec![resolved("x.y", B, json!({"k": 1}))]),
            Platform::Linux,
        );
        assert!(p.stage.is_empty());
        assert_eq!(p.pending_settings.len(), 1);
    }

    #[test]
    fn disabled_locally_shows_the_default() {
        let mut s = AddonState::default();
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({})));
        assert!(active_for(SlotId::AgentSidebar, &s).is_some());
        s.set_disabled("x.y", true);
        assert!(active_for(SlotId::AgentSidebar, &s).is_none());
        // Still installed, so the cloud's same answer changes nothing.
        let p = plan(
            &s,
            &set(vec![resolved("x.y", A, json!({}))]),
            Platform::Linux,
        );
        assert!(p.is_empty());
    }

    #[test]
    fn filters() {
        let mut mac_only = resolved("x.y", A, json!({}));
        mac_only.platforms = Some(vec!["mac".into()]);
        let mut unknown = resolved("u.v", A, json!({}));
        unknown.slot = "left-rail".into();
        let mut newer = resolved("n.w", A, json!({}));
        newer.bridge = 2;
        let mut bad = resolved("bad", A, json!({}));
        bad.sha256 = "zz".into();
        let p = plan(
            &AddonState::default(),
            &set(vec![mac_only, unknown, newer, bad]),
            Platform::Linux,
        );
        assert!(p.stage.is_empty());
        let reasons: Vec<_> = p.ignored.iter().map(|i| i.reason).collect();
        assert_eq!(
            reasons,
            vec![
                IgnoreReason::Platform,
                IgnoreReason::UnknownSlot,
                IgnoreReason::BridgeTooNew,
                IgnoreReason::Malformed
            ]
        );
        let two = plan(
            &AddonState::default(),
            &set(vec![
                resolved("x.y", A, json!({})),
                resolved("z.w", B, json!({})),
            ]),
            Platform::Linux,
        );
        assert_eq!(two.stage.len(), 1);
        assert_eq!(two.ignored[0].reason, IgnoreReason::Duplicate);
    }

    #[test]
    fn removed_here_until_the_cloud_changes() {
        let mut s = AddonState {
            revision: Some("r_2".into()),
            ..Default::default()
        };
        s.installed
            .insert("agent-sidebar".into(), installed("x.y", A, json!({})));
        s.remove_locally("x.y");
        let p = plan(
            &s,
            &set(vec![resolved("x.y", A, json!({}))]),
            Platform::Linux,
        );
        assert!(p.stage.is_empty());
        assert_eq!(p.ignored[0].reason, IgnoreReason::RemovedHere);
        let mut newer = set(vec![resolved("x.y", B, json!({}))]);
        newer.revision = "r_3".into();
        let p = plan(&s, &newer, Platform::Linux);
        assert_eq!(p.stage.len(), 1);
        p.apply_immediate(&mut s, "r_3");
        assert!(s.removed.is_empty());
    }

    #[test]
    fn install_pending_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AddonStore::at(tmp.path().join("addons"));
        let manifest = testkit::manifest_json("example.sidebar", "1.0.0");
        let bytes = testkit::served_bundle(&manifest, &[("index.html", b"<p>1</p>".as_slice())]);
        let r = testkit::resolved_for(&manifest, &bytes);
        let mut state = AddonState::default();
        let p = plan(&state, &set(vec![r.clone()]), Platform::Linux);
        assert_eq!(p.stage.len(), 1);
        let bundle = crate::addons::bundle::verify_bundle(&bytes, &r.sha256, &r).unwrap();
        let entry = store.stage(&bytes, &bundle, &r).unwrap();
        state.set_pending(SlotId::AgentSidebar, entry);
        assert!(active_for(SlotId::AgentSidebar, &state).is_none());
        let out = install_pending(&store, &mut state, None).unwrap();
        assert!(out.failed.is_empty());
        assert!(matches!(out.changes[0], Change::Installed { .. }));
        assert!(state.pending.is_empty());
        let active = store.active(&state, SlotId::AgentSidebar).unwrap();
        assert_eq!(
            std::fs::read(active.dir.join("index.html")).unwrap(),
            b"<p>1</p>"
        );
        assert!(!store.staging_path(&r.sha256).exists(), "staged file gone");
        assert_eq!(store.load_saved().installed.len(), 1, "saved");

        // A tampered staged file is refused and the pending entry dropped.
        let manifest2 = testkit::manifest_json("example.sidebar", "1.1.0");
        let bytes2 = testkit::served_bundle(&manifest2, &[("index.html", b"<p>2</p>".as_slice())]);
        let r2 = testkit::resolved_for(&manifest2, &bytes2);
        let bundle2 = crate::addons::bundle::verify_bundle(&bytes2, &r2.sha256, &r2).unwrap();
        let entry2 = store.stage(&bytes2, &bundle2, &r2).unwrap();
        state.set_pending(SlotId::AgentSidebar, entry2);
        std::fs::write(store.staging_path(&r2.sha256), b"{}").unwrap();
        state.revision = Some("r_2".into());
        let out = install_pending(&store, &mut state, None).unwrap();
        assert_eq!(out.failed.len(), 1);
        assert!(state.pending.is_empty());
        assert_eq!(state.revision, None);
        assert_eq!(state.installed["agent-sidebar"].version, "1.0.0");
    }
}
