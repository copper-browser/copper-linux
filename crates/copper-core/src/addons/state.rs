//! The on-disk addon store (contract §6.1), under `<data>/addons/`:
//!
//! ```text
//! addons/
//!   state.json                 AddonState (atomic writes, .bak kept)
//!   bundles/<id>/<sha256>/…    unpacked files (immutable)
//!   staging/<sha256>.json      downloaded, verified, not yet installed
//!   data/<id>/storage.json     copper.storage (0600)
//! ```
//!
//! [`AddonState`] is `state.json`; [`AddonStore`] knows the paths, stages
//! and unpacks bundles, and collects garbage. A developer folder named by
//! `COPPER_ADDONS_DEV_DIR` (debug builds and test worlds only) overrides its
//! manifest's slot ([`dev_addon_from_env`]).

use super::bundle::{Bundle, BundleError, is_sha256_hex, verify_bundle_for};
use super::manifest::{Manifest, ManifestError, valid_id};
use super::resolved::{ResolvedAddon, empty_object};
use super::slot::SlotId;
use super::{BRIDGE_VERSION, Platform};
use crate::date::IsoDate;
use crate::store::{Paths, StoreError, backup_path, quarantine, write_atomic};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// `state.json` `schema`.
pub const STATE_SCHEMA: u32 = 1;
/// The developer override.
pub const DEV_DIR_ENV: &str = "COPPER_ADDONS_DEV_DIR";

/// One installed (or, in `pending`, staged) addon.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    pub version: String,
    pub sha256: String,
    /// Installed: the unpacked folder, `bundles/<id>/<sha256>`. Pending: the
    /// staged file, `staging/<sha256>.json`. Relative to the addons folder
    /// (absolute for a developer folder); see [`AddonStore::resolve`].
    pub dir: String,
    /// Resolved settings.
    #[serde(default = "empty_object")]
    pub settings: Value,
    /// When installed (pending: when staged).
    #[serde(default)]
    pub installed_at: IsoDate,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub slot: String,
    /// The bundle's manifest (so nothing has to be re-read to run it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Installed {
    /// `name`, or the id.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// `state.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddonState {
    #[serde(default = "schema_one")]
    pub schema: u32,
    /// The `/v1/addons` revision last applied in full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Slot id → what runs there.
    #[serde(default)]
    pub installed: BTreeMap<String, Installed>,
    /// Slot id → staged update waiting for Install (`null` when none).
    #[serde(default, with = "null_when_empty")]
    pub pending: BTreeMap<String, Installed>,
    /// Addon ids the person turned off here ("Use in Agent sidebar" off).
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<IsoDate>,
    /// Slot id → what ran there before the last install; its files are kept
    /// until the next launch.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub previous: BTreeMap<String, Installed>,
    /// Addon id → the revision at which the person removed it here; it stays
    /// away until the cloud's revision changes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub removed: BTreeMap<String, String>,
    /// `link base|user id` the installs belong to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The developer folder, when [`DEV_DIR_ENV`] applies. Never saved.
    #[serde(skip)]
    pub dev: Option<Installed>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn schema_one() -> u32 {
    STATE_SCHEMA
}

impl Default for AddonState {
    fn default() -> Self {
        AddonState {
            schema: STATE_SCHEMA,
            revision: None,
            installed: BTreeMap::new(),
            pending: BTreeMap::new(),
            disabled: Vec::new(),
            last_check: None,
            previous: BTreeMap::new(),
            removed: BTreeMap::new(),
            owner: None,
            dev: None,
            extra: Map::new(),
        }
    }
}

mod null_when_empty {
    use super::*;

    pub fn serialize<S: Serializer>(
        map: &BTreeMap<String, Installed>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        if map.is_empty() {
            s.serialize_none()
        } else {
            map.serialize(s)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, Installed>, D::Error> {
        Ok(Option::<BTreeMap<String, Installed>>::deserialize(d)?.unwrap_or_default())
    }
}

impl AddonState {
    /// What renders in `slot`: the developer folder for that slot if any,
    /// else the installed addon unless the person turned it off here.
    pub fn active_for(&self, slot: SlotId) -> Option<&Installed> {
        if let Some(dev) = self.dev.as_ref().filter(|d| d.slot == slot.as_str()) {
            return Some(dev);
        }
        self.installed
            .get(slot.as_str())
            .filter(|i| !self.is_disabled(&i.id))
    }

    /// The staged update for `slot`.
    pub fn pending_for(&self, slot: SlotId) -> Option<&Installed> {
        self.pending.get(slot.as_str())
    }

    /// Turned off here.
    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.iter().any(|d| d == id)
    }

    /// Turn an addon off (or back on) here. Returns whether that changed.
    pub fn set_disabled(&mut self, id: &str, disabled: bool) -> bool {
        let was = self.is_disabled(id);
        if disabled && !was {
            self.disabled.push(id.to_string());
            self.disabled.sort();
        } else if !disabled && was {
            self.disabled.retain(|d| d != id);
        }
        was != disabled
    }

    /// Stage `entry` as `slot`'s pending update.
    pub fn set_pending(&mut self, slot: SlotId, entry: Installed) {
        self.pending.insert(slot.as_str().to_string(), entry);
    }

    /// Remove `slot`'s installed addon now; its files stay until the next
    /// launch (`previous`). Returns what was there.
    pub fn uninstall(&mut self, slot: SlotId) -> Option<Installed> {
        let old = self.installed.remove(slot.as_str())?;
        self.previous.insert(slot.as_str().to_string(), old.clone());
        Some(old)
    }

    /// Remove an addon here until the cloud changes ("Remove"): installed
    /// and pending entries with this id go, and `revision` is remembered.
    pub fn remove_locally(&mut self, id: &str) -> Vec<SlotId> {
        let mut slots = Vec::new();
        for slot in SlotId::ALL {
            let here = self
                .installed
                .get(slot.as_str())
                .is_some_and(|i| i.id == id);
            if here {
                self.uninstall(*slot);
                slots.push(*slot);
            }
            if self.pending.get(slot.as_str()).is_some_and(|p| p.id == id) {
                self.pending.remove(slot.as_str());
                if !slots.contains(slot) {
                    slots.push(*slot);
                }
            }
        }
        self.removed
            .insert(id.to_string(), self.revision.clone().unwrap_or_default());
        slots
    }

    /// Drop `previous` (call at launch, before [`AddonStore::gc`]).
    pub fn forget_previous(&mut self) {
        self.previous.clear();
    }
}

/// What a slot shows, with everything needed to serve and run it.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveAddon {
    pub slot: SlotId,
    pub installed: Installed,
    pub manifest: Manifest,
    /// The folder `copper-addon://<id>/` serves from.
    pub dir: PathBuf,
    /// From [`DEV_DIR_ENV`].
    pub dev: bool,
}

/// The `addons` folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddonStore {
    root: PathBuf,
}

/// An id that is safe as a folder name (an invalid one is replaced by a
/// hash, so nothing can climb out of the store).
fn safe_id(id: &str) -> String {
    if valid_id(id) {
        id.to_string()
    } else {
        format!(
            "_invalid-{}",
            &super::bundle::sha256_hex(id.as_bytes())[..16]
        )
    }
}

fn safe_sha(sha: &str) -> String {
    if is_sha256_hex(sha) {
        sha.to_string()
    } else {
        format!(
            "_invalid-{}",
            &super::bundle::sha256_hex(sha.as_bytes())[..16]
        )
    }
}

impl AddonStore {
    /// `<data>/addons`.
    pub fn new(paths: &Paths) -> Self {
        AddonStore {
            root: paths.data.join("addons"),
        }
    }

    /// A store at an explicit folder (tests).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        AddonStore { root: root.into() }
    }

    /// The `addons` folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `addons/state.json`.
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// `addons/bundles/<id>/<sha256>`.
    pub fn bundle_dir(&self, id: &str, sha256: &str) -> PathBuf {
        self.root
            .join("bundles")
            .join(safe_id(id))
            .join(safe_sha(sha256))
    }

    /// `addons/staging/<sha256>.json`.
    pub fn staging_path(&self, sha256: &str) -> PathBuf {
        self.root
            .join("staging")
            .join(format!("{}.json", safe_sha(sha256)))
    }

    /// `addons/data/<id>`.
    pub fn data_dir(&self, id: &str) -> PathBuf {
        self.root.join("data").join(safe_id(id))
    }

    /// `addons/data/<id>/storage.json`.
    pub fn storage_path(&self, id: &str) -> PathBuf {
        self.data_dir(id).join("storage.json")
    }

    /// An entry's `dir` as a path (relative ones are under the store).
    pub fn resolve(&self, dir: &str) -> PathBuf {
        let p = Path::new(dir);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            dir.split('/')
                .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                .fold(self.root.clone(), |acc, s| acc.join(s))
        }
    }

    /// Read `state.json` (falling back to `.bak`, setting a corrupt file
    /// aside); missing = a fresh state. The developer folder, when
    /// [`DEV_DIR_ENV`] applies, is attached as [`AddonState::dev`].
    pub fn load(&self) -> AddonState {
        let mut state = self.load_saved();
        state.dev = dev_addon_from_env();
        state
    }

    pub(crate) fn load_saved(&self) -> AddonState {
        let path = self.state_path();
        for (i, p) in [path.clone(), backup_path(&path)].into_iter().enumerate() {
            let Ok(bytes) = fs::read(&p) else { continue };
            match serde_json::from_slice::<AddonState>(&bytes) {
                Ok(s) => return s,
                Err(e) => {
                    tracing::warn!(file = %p.display(), error = %e, "addons state unreadable; set aside");
                    let _ = quarantine(&p);
                    if i == 1 {
                        break;
                    }
                }
            }
        }
        AddonState::default()
    }

    /// Write `state.json` atomically (previous good copy kept as `.bak`).
    pub fn save(&self, state: &AddonState) -> Result<(), StoreError> {
        let path = self.state_path();
        let bytes = crate::json::encode(state, crate::json::Style::Pretty).map_err(|e| {
            StoreError::Json {
                path: path.clone(),
                source: e,
            }
        })?;
        write_atomic(&path, &bytes, false, true)
    }

    /// Keep downloaded, verified bytes in `staging/<sha256>.json` and return
    /// the `pending` entry for them.
    pub fn stage(
        &self,
        bytes: &[u8],
        bundle: &Bundle,
        resolved: &ResolvedAddon,
    ) -> Result<Installed, BundleError> {
        let path = self.staging_path(&bundle.sha256);
        write_atomic(&path, bytes, false, false).map_err(|e| BundleError::Io {
            path: path.clone(),
            source: std::io::Error::other(e.to_string()),
        })?;
        Ok(Installed {
            id: bundle.manifest.id.clone(),
            version: bundle.manifest.version.clone(),
            sha256: bundle.sha256.clone(),
            dir: format!("staging/{}.json", safe_sha(&bundle.sha256)),
            settings: resolved.settings.clone(),
            installed_at: IsoDate::now(),
            name: bundle.manifest.name.clone(),
            slot: bundle.manifest.slot.clone(),
            manifest: Some(bundle.manifest.clone()),
            extra: Map::new(),
        })
    }

    /// The staged bundle for a `pending` entry, checked again (sha256, id,
    /// version, slot) before it is installed.
    pub fn read_staged(&self, pending: &Installed) -> Result<(Vec<u8>, Bundle), BundleError> {
        let path = self.staging_path(&pending.sha256);
        let bytes = fs::read(&path).map_err(|e| BundleError::Io {
            path: path.clone(),
            source: e,
        })?;
        let bundle = verify_bundle_for(
            &bytes,
            &pending.sha256,
            &pending.id,
            &pending.version,
            &pending.slot,
        )?;
        Ok((bytes, bundle))
    }

    /// Unpack a verified bundle into `bundles/<id>/<sha256>/` and return the
    /// relative `dir`. A folder already there with other contents is
    /// corrupt (the name is the content's hash): it is removed and written
    /// again.
    pub fn unpack(&self, bundle: &Bundle) -> Result<String, BundleError> {
        let dir = self.bundle_dir(&bundle.manifest.id, &bundle.sha256);
        match bundle.write_to(&dir) {
            Err(BundleError::ExistsDifferent(_)) => {
                tracing::warn!(dir = %dir.display(), "bundle folder doesn't match its hash; rewriting");
                fs::remove_dir_all(&dir).map_err(|e| BundleError::Io {
                    path: dir.clone(),
                    source: e,
                })?;
                bundle.write_to(&dir)?;
            }
            other => other?,
        }
        Ok(format!(
            "bundles/{}/{}",
            safe_id(&bundle.manifest.id),
            safe_sha(&bundle.sha256)
        ))
    }

    /// What a slot shows: [`AddonState::active_for`], with its folder and
    /// manifest, if the folder is there and the addon fits this build
    /// (bridge version, platform).
    pub fn active(&self, state: &AddonState, slot: SlotId) -> Option<ActiveAddon> {
        let installed = state.active_for(slot)?;
        let dev = state.dev.as_ref().is_some_and(|d| d == installed);
        let dir = self.resolve(&installed.dir);
        let manifest = match &installed.manifest {
            Some(m) => m.clone(),
            None => Manifest::from_slice(&fs::read(dir.join("manifest.json")).ok()?).ok()?,
        };
        if manifest.bridge > BRIDGE_VERSION
            || !manifest.runs_on(Platform::current())
            || !dir.is_dir()
        {
            return None;
        }
        Some(ActiveAddon {
            slot,
            installed: installed.clone(),
            manifest,
            dir,
            dev,
        })
    }

    /// Delete bundle folders and staged files nothing refers to (kept:
    /// `installed`, `previous`, `pending`), plus leftover temporaries.
    /// Returns how many entries went. Data folders are never touched.
    pub fn gc(&self, state: &AddonState) -> usize {
        let mut keep_dirs = BTreeSet::new();
        for i in state.installed.values().chain(state.previous.values()) {
            keep_dirs.insert(self.bundle_dir(&i.id, &i.sha256));
        }
        let keep_staged: BTreeSet<PathBuf> = state
            .pending
            .values()
            .map(|p| self.staging_path(&p.sha256))
            .collect();
        let mut removed = 0;
        let bundles = self.root.join("bundles");
        for addon in read_dir_paths(&bundles) {
            if !addon.is_dir() {
                removed += remove_any(&addon);
                continue;
            }
            for version in read_dir_paths(&addon) {
                if !keep_dirs.contains(&version) {
                    removed += remove_any(&version);
                }
            }
            if read_dir_paths(&addon).is_empty() {
                let _ = fs::remove_dir(&addon);
            }
        }
        for staged in read_dir_paths(&self.root.join("staging")) {
            if !keep_staged.contains(&staged) {
                removed += remove_any(&staged);
            }
        }
        removed
    }

    /// Delete every addon's `copper.storage` (a different person signed in).
    pub fn wipe_data(&self) {
        let _ = fs::remove_dir_all(self.root.join("data"));
    }
}

fn read_dir_paths(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default()
}

fn remove_any(path: &Path) -> usize {
    let ok = if path.is_dir() {
        fs::remove_dir_all(path).is_ok()
    } else {
        fs::remove_file(path).is_ok()
    };
    usize::from(ok)
}

/// Why a developer folder can't be used.
#[derive(Debug, thiserror::Error)]
pub enum DevError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("the entry {0:?} is not in the folder")]
    MissingEntry(String),
}

/// An unpacked addon folder (`manifest.json` + files) as an entry for its
/// manifest's slot, settings at their manifest defaults.
pub fn dev_addon(dir: &Path) -> Result<Installed, DevError> {
    let dir = dir.canonicalize().map_err(|e| DevError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    let path = dir.join("manifest.json");
    let bytes = fs::read(&path).map_err(|e| DevError::Io { path, source: e })?;
    let manifest = Manifest::from_slice(&bytes)?;
    manifest.validate()?;
    if !dir.join(&manifest.entry).is_file() {
        return Err(DevError::MissingEntry(manifest.entry.clone()));
    }
    Ok(Installed {
        id: manifest.id.clone(),
        version: manifest.version.clone(),
        sha256: "dev".into(),
        dir: dir.to_string_lossy().into_owned(),
        settings: manifest.default_settings(),
        installed_at: IsoDate::now(),
        name: manifest.name.clone(),
        slot: manifest.slot.clone(),
        manifest: Some(manifest),
        extra: Map::new(),
    })
}

/// [`dev_addon`] for `COPPER_ADDONS_DEV_DIR`, honoured only in debug builds
/// or a test world (`COPPER_WORLD` set). A folder that doesn't load is
/// logged and ignored.
pub fn dev_addon_from_env() -> Option<Installed> {
    let dir = std::env::var_os(DEV_DIR_ENV).filter(|d| !d.is_empty())?;
    let world = std::env::var("COPPER_WORLD").is_ok_and(|w| !w.trim().is_empty());
    if !cfg!(debug_assertions) && !world {
        tracing::warn!("{DEV_DIR_ENV} is ignored outside debug builds and test worlds");
        return None;
    }
    match dev_addon(Path::new(&dir)) {
        Ok(i) => {
            tracing::info!(id = %i.id, slot = %i.slot, "developer addon folder in use");
            Some(i)
        }
        Err(e) => {
            tracing::warn!(error = %e, "{DEV_DIR_ENV} not used");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::testkit;
    use serde_json::json;

    #[test]
    fn state_round_trips_and_keeps_unknown_keys() {
        let text = r#"{"schema":1,"revision":"r_1","installed":{"agent-sidebar":{"id":"example.sidebar","version":"1.0.0","sha256":"ab","dir":"bundles/example.sidebar/ab","settings":{"a":1},"installed_at":"2026-10-19T00:00:00Z","x":2}},"pending":null,"disabled":[],"last_check":"2026-10-19T00:00:00Z","future":true}"#;
        let s: AddonState = serde_json::from_str(text).unwrap();
        assert!(s.pending.is_empty());
        assert_eq!(s.installed["agent-sidebar"].settings["a"], 1);
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["pending"], Value::Null);
        assert_eq!(back["future"], true);
        assert_eq!(back["installed"]["agent-sidebar"]["x"], 2);
        assert_eq!(back["schema"], 1);
    }

    #[test]
    fn save_load_is_atomic_with_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AddonStore::at(tmp.path().join("addons"));
        assert_eq!(store.load_saved(), AddonState::default());
        let mut s = AddonState {
            revision: Some("r_1".into()),
            ..Default::default()
        };
        store.save(&s).unwrap();
        s.revision = Some("r_2".into());
        store.save(&s).unwrap();
        assert_eq!(store.load_saved().revision.as_deref(), Some("r_2"));
        fs::write(store.state_path(), b"{broken").unwrap();
        assert_eq!(
            store.load_saved().revision.as_deref(),
            Some("r_1"),
            "the .bak is read"
        );
    }

    #[test]
    fn paths_stay_inside_the_store() {
        let store = AddonStore::at("/data/addons");
        let sha = "a".repeat(64);
        assert_eq!(
            store.bundle_dir("example.sidebar", &sha),
            Path::new("/data/addons/bundles/example.sidebar").join(&sha)
        );
        let invalid = |p: PathBuf, parent: &str| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.starts_with("_invalid-") && p.parent() == Some(Path::new(parent))
        };
        let evil = store.bundle_dir("../../etc", "x");
        assert!(
            evil.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("_invalid-")
        );
        assert!(invalid(
            evil.parent().unwrap().to_path_buf(),
            "/data/addons/bundles"
        ));
        assert!(invalid(store.data_dir(".."), "/data/addons/data"));
        assert_eq!(
            store.resolve("bundles/../../x"),
            Path::new("/data/addons/bundles/x")
        );
        assert_eq!(
            store.storage_path("example.sidebar"),
            Path::new("/data/addons/data/example.sidebar/storage.json")
        );
    }

    #[test]
    fn stage_install_gc() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AddonStore::at(tmp.path().join("addons"));
        let manifest = testkit::manifest_json("example.sidebar", "1.0.0");
        let bytes = testkit::served_bundle(&manifest, &[("index.html", b"<p>hi</p>".as_slice())]);
        let resolved = testkit::resolved_for(&manifest, &bytes);
        let bundle =
            crate::addons::bundle::verify_bundle(&bytes, &resolved.sha256, &resolved).unwrap();
        let pending = store.stage(&bytes, &bundle, &resolved).unwrap();
        assert_eq!(pending.dir, format!("staging/{}.json", resolved.sha256));
        let (_, again) = store.read_staged(&pending).unwrap();
        let dir = store.unpack(&again).unwrap();
        assert!(store.resolve(&dir).join("index.html").is_file());
        // Unreferenced: everything goes.
        let mut state = AddonState::default();
        assert_eq!(store.gc(&state), 2);
        assert!(!store.resolve(&dir).exists());
        // Referenced: kept.
        store.unpack(&again).unwrap();
        state.installed.insert(
            "agent-sidebar".into(),
            Installed {
                dir: dir.clone(),
                ..pending.clone()
            },
        );
        fs::create_dir_all(tmp.path().join("addons/bundles/old.addon/xyz")).unwrap();
        assert_eq!(store.gc(&state), 1);
        assert!(store.resolve(&dir).exists());
        assert!(!tmp.path().join("addons/bundles/old.addon").exists());
        let active = store.active(&state, SlotId::AgentSidebar).unwrap();
        assert_eq!(active.manifest.id, "example.sidebar");
        assert!(!active.dev);
        state.set_disabled("example.sidebar", true);
        assert!(store.active(&state, SlotId::AgentSidebar).is_none());
    }

    #[test]
    fn dev_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = testkit::manifest_json("example.dev", "0.1.0");
        fs::write(
            tmp.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            dev_addon(tmp.path()),
            Err(DevError::MissingEntry(_))
        ));
        fs::write(tmp.path().join("index.html"), b"<p>dev</p>").unwrap();
        let dev = dev_addon(tmp.path()).unwrap();
        assert_eq!(
            dev.settings,
            json!({"apiUrl": "https://api.example.com/v1"})
        );
        let mut state = AddonState::default();
        state.set_disabled("example.dev", true);
        state.dev = Some(dev);
        let store = AddonStore::at(tmp.path().join("addons"));
        let active = store.active(&state, SlotId::AgentSidebar).unwrap();
        assert!(active.dev, "the dev folder wins, disabled or not");
        assert_eq!(active.dir, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn local_toggles() {
        let mut s = AddonState::default();
        assert!(s.set_disabled("a.b", true));
        assert!(!s.set_disabled("a.b", true));
        assert!(s.is_disabled("a.b"));
        assert!(s.set_disabled("a.b", false));
        s.installed.insert(
            "agent-sidebar".into(),
            Installed {
                id: "a.b".into(),
                ..Default::default()
            },
        );
        s.revision = Some("r_9".into());
        assert_eq!(s.remove_locally("a.b"), vec![SlotId::AgentSidebar]);
        assert!(s.installed.is_empty());
        assert_eq!(s.removed["a.b"], "r_9");
        assert!(s.previous.contains_key("agent-sidebar"));
    }
}
