//! A device can only overwrite what it changed (Mac: `CloudSync.Intents`).
//! Pure: no browser, no network.
//!
//! Each device builds its spaces document from what it has now. One whose
//! state merely *lacks* a filing (a copied session, folders made on another
//! computer, a drift) would otherwise win the three-way merge against an
//! unchanged server and unfile those tabs everywhere (the 2026-10-28
//! incident: 36 Exowatt tabs). So this device records *intents* — the
//! changes made here to filing and kept tabs ([`record`]) — and for anything
//! the base already has, only an intent lets this device's copy through
//! ([`outgoing`]). A publish, or an apply, that would unfile or remove many
//! kept tabs at once is held and asked about ([`Loss`]).

use super::adapter::LiveKept;
use super::merge;
use copper_core::cloud::{KeptTab, SpacesDoc, SpacesIntents};
use std::collections::{HashMap, HashSet};

/// More unfiled plus removed kept tabs than this in one publish or one apply
/// is held for the user to confirm.
// shortcut: one fixed count for every profile; make it a share of the kept tabs if 10 proves wrong for big ones.
pub const HOLD_OVER: usize = 10;

/// What changed here between two looks at the kept tabs and the folders
/// (`before`, `after`), as intents. `key` names a tab's entry in the
/// document (the engine's key for it this run, else the one it carries).
pub fn record(
    intents: &mut SpacesIntents,
    before: &[LiveKept],
    after: &[LiveKept],
    key: impl Fn(&LiveKept) -> Option<String>,
    folders_before: &[String],
    folders_after: &[String],
) -> bool {
    let start = intents.clone();
    let now: HashMap<&str, &LiveKept> = after.iter().map(|t| (t.tab_id.as_str(), t)).collect();
    let was: HashSet<&str> = before.iter().map(|t| t.tab_id.as_str()).collect();
    let mut next = || {
        intents.seq += 1;
        intents.seq
    };
    for old in before {
        let Some(k) = key(old) else { continue };
        match now.get(old.tab_id.as_str()) {
            // Unsaved, unpinned for good, the cross: gone.
            None => {
                let n = next();
                intents.removed.insert(k, n);
            }
            // To another space or section: a new entry; this one is gone.
            Some(new)
                if !new.space.eq_ignore_ascii_case(&old.space) || new.pinned != old.pinned =>
            {
                let n = next();
                intents.removed.insert(k, n);
            }
            Some(new) if !same_folder(new.group.as_deref(), old.group.as_deref()) => {
                let n = next();
                intents.filed.insert(k, n);
            }
            Some(_) => {}
        }
    }
    // Kept again (Reopen Closed Tab brings its key back): as it is now.
    for new in after.iter().filter(|t| !was.contains(t.tab_id.as_str())) {
        if let Some(k) = key(new) {
            let n = next();
            intents.filed.insert(k, n);
        }
    }
    let kept: HashSet<String> = folders_after
        .iter()
        .map(|f| f.to_ascii_uppercase())
        .collect();
    for f in folders_before {
        let id = f.to_ascii_uppercase();
        if !kept.contains(&id) {
            let n = next();
            intents.dissolved.insert(id, n);
        }
    }
    *intents != start
}

fn same_folder(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

/// This device's document as it may go out: where the base has an entry,
/// this device's copy only where an intent says it changed it here.
/// * a kept tab's folder: this device's only with a filing intent;
/// * a kept tab the base has and this device lacks: dropped only with a
///   removal intent (else the base's stays, and applying it reopens it here);
/// * a folder the base has and this device lacks: dropped only when it was
///   deleted here.
///
/// New entries go out as they are. With no base (a first sync) nothing is
/// held back: that merge is a union anyway.
pub fn outgoing(
    base: Option<&SpacesDoc>,
    mut local: SpacesDoc,
    intents: &SpacesIntents,
) -> SpacesDoc {
    let Some(base) = base else { return local };
    let base_by: HashMap<&str, &KeptTab> = base.tabs.iter().map(|t| (t.key.as_str(), t)).collect();
    for t in &mut local.tabs {
        if let Some(was) = base_by.get(t.key.as_str())
            && !intents.filed.contains_key(&t.key)
        {
            t.group = was.group.clone();
        }
    }
    let mine: HashSet<String> = local.tabs.iter().map(|t| t.key.clone()).collect();
    let spaces: HashSet<String> = local
        .spaces
        .iter()
        .map(|s| s.id.to_ascii_uppercase())
        .collect();
    let pinned: HashSet<String> = local
        .tabs
        .iter()
        .filter(|t| t.pinned)
        .map(|t| merge::site(&t.url))
        .collect();
    let back: Vec<KeptTab> = base
        .tabs
        .iter()
        .filter(|was| {
            !mine.contains(&was.key)
                && !intents.removed.contains_key(&was.key)
                // A space deleted here takes its tabs with it.
                && (spaces.is_empty() || spaces.contains(&was.space.to_ascii_uppercase()))
                // One pin per site: a pin for a site pinned here under another key is that pin.
                && !(was.pinned && pinned.contains(&merge::site(&was.url)))
        })
        .cloned()
        .collect();
    local.tabs.extend(back);
    local.tabs.sort_by(|a, b| a.key.cmp(&b.key));
    if let (Some(folders), Some(was)) = (local.groups.as_mut(), base.groups.as_ref()) {
        let have: HashSet<String> = folders.iter().map(|f| f.id.to_ascii_uppercase()).collect();
        for (i, f) in was.iter().enumerate() {
            let id = f.id.to_ascii_uppercase();
            if !have.contains(&id) && !intents.dissolved.contains_key(&id) {
                folders.insert(i.min(folders.len()), f.clone());
            }
        }
    }
    local
}

/// How much of `before`'s filing and kept tabs `after` takes away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Loss {
    /// Kept tabs in a folder in `before` and in none in `after`.
    pub unfiled: usize,
    /// Kept tabs in `before` and not in `after`.
    pub removed: usize,
    /// Folders in `before`, holding tabs there, and not in `after`.
    pub folders: usize,
}

impl Loss {
    pub fn between(before: &SpacesDoc, after: &SpacesDoc) -> Loss {
        let after_by: HashMap<&str, &KeptTab> =
            after.tabs.iter().map(|t| (t.key.as_str(), t)).collect();
        // A pin keyed afresh (the first space changed) is still pinned.
        let pinned: HashSet<String> = after
            .tabs
            .iter()
            .filter(|t| t.pinned)
            .map(|t| merge::site(&t.url))
            .collect();
        let mut loss = Loss::default();
        for t in &before.tabs {
            match after_by.get(t.key.as_str()) {
                None if t.pinned && pinned.contains(&merge::site(&t.url)) => {}
                None => loss.removed += 1,
                Some(now) if t.group.is_some() && now.group.is_none() => loss.unfiled += 1,
                Some(_) => {}
            }
        }
        if let (Some(was), Some(now)) = (&before.groups, &after.groups) {
            let now: HashSet<String> = now.iter().map(|f| f.id.to_ascii_uppercase()).collect();
            loss.folders = was
                .iter()
                .filter(|f| {
                    !now.contains(&f.id.to_ascii_uppercase())
                        && before.tabs.iter().any(|t| {
                            t.group
                                .as_deref()
                                .is_some_and(|g| g.eq_ignore_ascii_case(&f.id))
                        })
                })
                .count();
        }
        loss
    }

    /// Too much to send without asking: many tabs, or a folder still holding some.
    pub fn holds_out(&self) -> bool {
        self.unfiled + self.removed > HOLD_OVER || self.folders > 0
    }

    /// Too much to take without asking.
    pub fn holds_in(&self) -> bool {
        self.unfiled + self.removed > HOLD_OVER
    }

    /// `unfiled=N removed=M folders-=K`, for the log.
    pub fn line(&self) -> String {
        format!(
            "unfiled={} removed={} folders-={}",
            self.unfiled, self.removed, self.folders
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::cloud::CloudFolder;
    use copper_core::session::Space;

    const SPACE: &str = "AAAAAAAA-0000-0000-0000-000000000001";
    const APPS: &str = "F0000000-0000-0000-0000-00000000000A";

    fn folder(id: &str, name: &str) -> CloudFolder {
        CloudFolder {
            id: id.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    fn kept(n: usize, group: Option<&str>) -> KeptTab {
        KeptTab {
            key: format!(
                "{}|saved|site{n}.test|https://site{n}.test/",
                SPACE.to_lowercase()
            ),
            space: SPACE.into(),
            url: format!("https://site{n}.test/"),
            title: format!("Site {n}"),
            pinned: false,
            pin: None,
            group: group.map(str::to_string),
            extra: Default::default(),
        }
    }

    fn doc(mut tabs: Vec<KeptTab>, folders: Vec<CloudFolder>) -> SpacesDoc {
        tabs.sort_by(|a, b| a.key.cmp(&b.key));
        SpacesDoc {
            v: 1,
            spaces: vec![Space {
                id: SPACE.into(),
                name: "Exowatt".into(),
                ..Default::default()
            }],
            tabs,
            groups: Some(folders),
            extra: Default::default(),
        }
    }

    fn tab(d: &mut SpacesDoc, n: usize) -> &mut KeptTab {
        let key = kept(n, None).key;
        d.tabs.iter_mut().find(|t| t.key == key).unwrap()
    }

    fn live(n: usize, group: Option<&str>) -> LiveKept {
        let t = kept(n, group);
        LiveKept {
            tab_id: format!("t{n}"),
            key: Some(t.key),
            space: SPACE.into(),
            pinned: false,
            url: t.url,
            title: t.title,
            pin: None,
            group: t.group,
        }
    }

    /// The incident: 36 tabs filed in the base and on the server; this
    /// device has them all, unfiled, and changed nothing.
    #[test]
    fn a_device_that_lacks_the_filing_does_not_unfile_anything() {
        let filed = doc(
            (0..36).map(|n| kept(n, Some(APPS))).collect(),
            vec![folder(APPS, "Exowatt › Apps")],
        );
        let drifted = doc((0..36).map(|n| kept(n, None)).collect(), vec![]);
        let out = outgoing(Some(&filed), drifted, &SpacesIntents::default());
        assert_eq!(out, filed, "every group and the folder stay the base's");
        // The merge against the unchanged server leaves them filed…
        let merged = merge::spaces(Some(&filed), &out, &filed);
        assert_eq!(merged, filed);
        // …and a receiver has nothing to apply.
        assert_eq!(Loss::between(&filed, &merged), Loss::default());
    }

    #[test]
    fn an_unfiling_made_here_goes_out() {
        let filed = doc(
            (0..36).map(|n| kept(n, Some(APPS))).collect(),
            vec![folder(APPS, "Apps")],
        );
        let mut intents = SpacesIntents::default();
        let before: Vec<LiveKept> = (0..36).map(|n| live(n, Some(APPS))).collect();
        let mut after = before.clone();
        after[3].group = None;
        let ids = vec![APPS.to_string()];
        assert!(record(
            &mut intents,
            &before,
            &after,
            |t| t.key.clone(),
            &ids,
            &ids
        ));
        assert_eq!(intents.filed.len(), 1);
        let mut local = filed.clone();
        tab(&mut local, 3).group = None;
        tab(&mut local, 4).group = None; // drifted, not changed here
        let mut out = outgoing(Some(&filed), local, &intents);
        assert_eq!(tab(&mut out, 3).group, None);
        assert_eq!(tab(&mut out, 4).group.as_deref(), Some(APPS));
        let loss = Loss::between(&filed, &out);
        assert_eq!(loss.unfiled, 1);
        assert!(!loss.holds_out());
        // Taken by the server: the intent is spent.
        let upto = intents.seq;
        intents.settle(upto);
        assert!(intents.is_empty());
    }

    #[test]
    fn eleven_unfilings_made_here_are_held() {
        let filed = doc(
            (0..36).map(|n| kept(n, Some(APPS))).collect(),
            vec![folder(APPS, "Apps")],
        );
        let before: Vec<LiveKept> = (0..36).map(|n| live(n, Some(APPS))).collect();
        let after: Vec<LiveKept> = (0..36)
            .map(|n| live(n, if n < 11 { None } else { Some(APPS) }))
            .collect();
        let mut intents = SpacesIntents::default();
        let ids = vec![APPS.to_string()];
        record(&mut intents, &before, &after, |t| t.key.clone(), &ids, &ids);
        let mut local = filed.clone();
        for n in 0..11 {
            tab(&mut local, n).group = None;
        }
        let out = outgoing(Some(&filed), local, &intents);
        let loss = Loss::between(&filed, &out);
        assert_eq!(loss.unfiled, 11);
        assert!(loss.holds_out());
        // Ten is still fine.
        let ten = Loss {
            unfiled: 10,
            ..Default::default()
        };
        assert!(!ten.holds_out() && !ten.holds_in());
    }

    #[test]
    fn a_mass_unfile_from_another_device_is_held_here() {
        let filed = doc(
            (0..36).map(|n| kept(n, Some(APPS))).collect(),
            vec![folder(APPS, "Apps")],
        );
        // An old build elsewhere published every group as none.
        let server = doc(
            (0..36).map(|n| kept(n, None)).collect(),
            vec![folder(APPS, "Apps")],
        );
        let merged = merge::spaces(Some(&filed), &filed, &server);
        let loss = Loss::between(&filed, &merged);
        assert_eq!(loss.unfiled, 36);
        assert!(loss.holds_in());
    }

    #[test]
    fn removals_and_folders_need_an_intent() {
        let base = doc(
            (0..3).map(|n| kept(n, Some(APPS))).collect(),
            vec![folder(APPS, "Apps"), folder("B", "Docs")],
        );
        // Here: tab 2 and both folders missing; only tab 1 and folder B were removed here.
        let local = doc(vec![kept(0, Some(APPS))], vec![]);
        let mut intents = SpacesIntents::default();
        intents.removed.insert(kept(1, None).key, 1);
        intents.dissolved.insert("B".into(), 2);
        let out = outgoing(Some(&base), local, &intents);
        let keys: Vec<&str> = out.tabs.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(
            keys,
            [kept(0, None).key.as_str(), kept(2, None).key.as_str()]
        );
        assert_eq!(out.groups.as_ref().unwrap(), &vec![folder(APPS, "Apps")]);
        // Dropping a folder that still holds tabs is held, however few.
        let gone = doc(base.tabs.clone(), vec![folder("B", "Docs")]);
        assert_eq!(Loss::between(&base, &gone).folders, 1);
        assert!(Loss::between(&base, &gone).holds_out());
    }

    #[test]
    fn what_changed_here_becomes_intents() {
        let before = vec![
            live(0, Some(APPS)),
            live(1, None),
            live(2, None),
            live(3, None),
        ];
        let mut moved = live(2, None);
        moved.space = "OTHER".into();
        let mut back = live(9, None);
        back.tab_id = "new".into();
        let after = vec![live(0, None), moved, live(3, None), back.clone()];
        let mut intents = SpacesIntents::default();
        assert!(record(
            &mut intents,
            &before,
            &after,
            |t| t.key.clone(),
            &[APPS.into(), "b".into()],
            &[APPS.to_lowercase()],
        ));
        let key = |n| kept(n, None).key;
        assert!(intents.filed.contains_key(&key(0)));
        assert!(intents.removed.contains_key(&key(1)));
        assert!(intents.removed.contains_key(&key(2)));
        assert!(!intents.filed.contains_key(&key(3)) && !intents.removed.contains_key(&key(3)));
        assert!(intents.filed.contains_key(&key(9)));
        assert_eq!(intents.dissolved.keys().collect::<Vec<_>>(), ["B"]);
        // Nothing changed: nothing recorded.
        let mut again = intents.clone();
        assert!(!record(
            &mut again,
            &after,
            &after,
            |t| t.key.clone(),
            &[],
            &[]
        ));
        // A later number survives an earlier publish's answer.
        let upto = intents.seq;
        intents.filed.insert(key(5), upto + 1);
        intents.settle(upto);
        assert_eq!(intents.filed.len(), 1);
    }
}
