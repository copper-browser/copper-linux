//! Where everything is kept, and how it is written.
//!
//! * [`Paths`] — `$XDG_DATA_HOME/copper` (default `~/.local/share/copper`)
//!   for data, `$XDG_CONFIG_HOME/copper` (default `~/.config/copper`) for
//!   `settings.json`; on Windows both are `%LOCALAPPDATA%\Copper\Data`
//!   ([`crate::platform`]). `COPPER_WORLD=<name>` puts a test run in
//!   `copper-<name>` folders so it never touches the real ones (the Mac's
//!   `SEARCH_PROBE` worlds).
//! * [`DataFile`] — every file, its relative path, its permissions (0600 for
//!   anything holding a secret on Unix; the per-user profile ACL on Windows)
//!   and its layout.
//! * [`Store`] — typed load/save. A save writes a sibling temporary, fsyncs,
//!   renames over the file and fsyncs the folder; the previous good version
//!   is kept as `<file>.bak`. A load that finds the file corrupt sets it aside
//!   as `<stem>.unreadable-<unix>.json` (the Mac's quarantine name) and reads
//!   the `.bak`; if both are bad, both are set aside and the caller is told
//!   ([`LoadSource::Reset`]) — never an empty session in silence.
//! * [`Store::save_session`] — the Mac's session guard — refuses an empty
//!   write before restore finished and keeps `session.previous.json` when the
//!   tab count drops by half or more.

use crate::json::{self, Style};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Every file the Linux build keeps, under the Mac's names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DataFile {
    Session,
    History,
    Bookmarks,
    Windows,
    Groups,
    Hidden,
    Downloads,
    Agent,
    Chat,
    Mcp,
    Trails,
    Cloud,
    CloudSync,
    CloudIntelligence,
    Canvases,
    Intelligence,
    Claude,
    Updates,
    Crashes,
    /// `settings.json` in the config folder (Linux stand-in for `UserDefaults`).
    Settings,
}

impl DataFile {
    /// Every file.
    pub const ALL: [DataFile; 20] = [
        DataFile::Session,
        DataFile::History,
        DataFile::Bookmarks,
        DataFile::Windows,
        DataFile::Groups,
        DataFile::Hidden,
        DataFile::Downloads,
        DataFile::Agent,
        DataFile::Chat,
        DataFile::Mcp,
        DataFile::Trails,
        DataFile::Cloud,
        DataFile::CloudSync,
        DataFile::CloudIntelligence,
        DataFile::Canvases,
        DataFile::Intelligence,
        DataFile::Claude,
        DataFile::Updates,
        DataFile::Crashes,
        DataFile::Settings,
    ];

    /// Path relative to its folder (data, or config for [`DataFile::Settings`]).
    pub fn rel_path(self) -> &'static str {
        match self {
            DataFile::Session => "session.json",
            DataFile::History => "history.json",
            DataFile::Bookmarks => "bookmarks.json",
            DataFile::Windows => "windows.json",
            DataFile::Groups => "groups.json",
            DataFile::Hidden => "hidden.json",
            DataFile::Downloads => "downloads.json",
            DataFile::Agent => "agent.json",
            DataFile::Chat => "chat.json",
            DataFile::Mcp => "mcp.json",
            DataFile::Trails => "trails.json",
            DataFile::Cloud => "cloud.json",
            DataFile::CloudSync => "cloud-sync.json",
            DataFile::CloudIntelligence => "cloud-intelligence.json",
            DataFile::Canvases => "canvas/canvases.json",
            DataFile::Intelligence => "intelligence.json",
            DataFile::Claude => "claude.json",
            DataFile::Updates => "updates.json",
            DataFile::Crashes => "crashes/crashes.json",
            DataFile::Settings => "settings.json",
        }
    }

    /// Holds a token or key: written 0600.
    pub fn secret(self) -> bool {
        matches!(
            self,
            DataFile::Agent
                | DataFile::Cloud
                | DataFile::CloudSync
                | DataFile::CloudIntelligence
                | DataFile::Intelligence
                | DataFile::Claude
                | DataFile::Mcp
        )
    }

    /// Layout the Mac uses for it.
    pub fn style(self) -> Style {
        match self {
            DataFile::Canvases | DataFile::Crashes | DataFile::Settings => Style::Pretty,
            _ => Style::Compact,
        }
    }

    /// Lives in the config folder rather than the data folder.
    pub fn in_config(self) -> bool {
        self == DataFile::Settings
    }
}

/// A Rust type that is the whole of one [`DataFile`].
pub trait FileModel: Serialize + DeserializeOwned {
    /// Which file.
    const FILE: DataFile;
}

macro_rules! file_model {
    ($($t:ty => $f:ident),* $(,)?) => { $(impl FileModel for $t { const FILE: DataFile = DataFile::$f; })* };
}
file_model! {
    crate::session::SessionShape => Session,
    Vec<crate::history::Visit> => History,
    Vec<crate::bookmarks::Bookmark> => Bookmarks,
    Vec<crate::groups::SavedWindow> => Windows,
    crate::groups::GroupsFile => Groups,
    crate::crashes::HiddenFile => Hidden,
    Vec<crate::downloads::Keep> => Downloads,
    crate::agent::AgentFile => Agent,
    crate::agent::ChatConfig => Chat,
    crate::agent::McpServersFile => Mcp,
    crate::trails::TrailsFile => Trails,
    crate::cloud::CloudFile => Cloud,
    crate::cloud::CloudSyncFile => CloudSync,
    crate::cloud::CloudProvided => CloudIntelligence,
    Vec<crate::canvas::CanvasEntry> => Canvases,
    crate::intelligence::IntelligenceKeys => Intelligence,
    crate::intelligence::ClaudeCredential => Claude,
    crate::updates::UpdatesFile => Updates,
    crate::crashes::CrashIndex => Crashes,
    serde_json::Map<String, Value> => Settings,
}

/// The data and config folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    /// Explicit folders (tests, `--data-dir`).
    pub fn new(data: impl Into<PathBuf>, config: impl Into<PathBuf>) -> Self {
        Paths {
            data: data.into(),
            config: config.into(),
        }
    }

    /// From the environment, per OS (see [`crate::platform`]).
    ///
    /// Linux: `XDG_DATA_HOME`/`XDG_CONFIG_HOME` (absolute only, per the XDG
    /// spec), else `~/.local/share` / `~/.config`; folder `copper`, or
    /// `copper-<world>` with `COPPER_WORLD` set.
    ///
    /// Windows: `%LOCALAPPDATA%\Copper\Data` for both (`Copper-<world>` with
    /// `COPPER_WORLD`); same file names and formats as Linux and the Mac.
    pub fn from_env() -> Self {
        #[cfg(windows)]
        {
            let data = crate::platform::WindowsLayout::from_env().data();
            Paths {
                config: data.clone(),
                data,
            }
        }
        #[cfg(not(windows))]
        {
            Self::from_vars(|name| std::env::var_os(name))
        }
    }

    /// The Linux layout of [`Paths::from_env`] over any environment (another
    /// process's, say): `get` answers `HOME`, `XDG_DATA_HOME`,
    /// `XDG_CONFIG_HOME` and `COPPER_WORLD`.
    #[cfg(not(windows))]
    pub fn from_vars(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        let home = get("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let xdg = |var: &str, fallback: &str| -> PathBuf {
            get(var)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        let world = get("COPPER_WORLD").and_then(|w| w.into_string().ok());
        let name = match crate::platform::world_from(world.as_deref()) {
            Some(w) => format!("copper-{w}"),
            None => "copper".to_string(),
        };
        Paths {
            data: xdg("XDG_DATA_HOME", ".local/share").join(&name),
            config: xdg("XDG_CONFIG_HOME", ".config").join(&name),
        }
    }

    /// Absolute path of a file.
    pub fn file(&self, f: DataFile) -> PathBuf {
        if f.in_config() {
            self.config.join(f.rel_path())
        } else {
            self.data.join(f.rel_path())
        }
    }

    /// Per-profile WebKit storage: `<data>/profiles/<name>` (`default` for
    /// the shared jar).
    pub fn profile_dir(&self, profile: Option<&str>) -> PathBuf {
        let name: String = profile
            .unwrap_or("default")
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.data.join("profiles").join(name)
    }
}

/// What went wrong reading or writing a file.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl StoreError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Where a loaded value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSource {
    /// The file itself.
    Primary,
    /// The file was missing or corrupt; `.bak` was read. A corrupt file was
    /// set aside at `quarantined`.
    Backup { quarantined: Option<PathBuf> },
    /// Neither exists: a first run.
    Missing,
    /// Both were unreadable and were set aside; the value is the default.
    /// Tell the user.
    Reset { quarantined: Vec<PathBuf> },
}

/// A loaded value and where it came from.
#[derive(Debug, Clone)]
pub struct Loaded<T> {
    pub value: T,
    pub source: LoadSource,
}

/// Typed, atomic access to the files under [`Paths`].
#[derive(Debug, Clone)]
pub struct Store {
    paths: Paths,
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `<path>.bak`.
pub fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

impl Store {
    /// A store over these folders. Nothing is created until the first save.
    pub fn new(paths: Paths) -> Self {
        Store { paths }
    }

    /// [`Paths::from_env`].
    pub fn from_env() -> Self {
        Store::new(Paths::from_env())
    }

    /// The folders.
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Absolute path of a file.
    pub fn path(&self, f: DataFile) -> PathBuf {
        self.paths.file(f)
    }

    /// Load a file, falling back to `.bak` (see the module docs). `Ok(None)`
    /// value means neither exists ([`LoadSource::Missing`]) or both were
    /// unreadable ([`LoadSource::Reset`]).
    pub fn load_value(&self, f: DataFile) -> Result<Loaded<Option<Value>>, StoreError> {
        self.load_with(f, |bytes| serde_json::from_slice::<Value>(bytes))
    }

    /// Load a typed file. Missing or reset files give `T::default()`.
    pub fn load<T: FileModel + Default>(&self) -> Result<Loaded<T>, StoreError> {
        let loaded = self.load_with(T::FILE, |bytes| serde_json::from_slice::<T>(bytes))?;
        Ok(Loaded {
            value: loaded.value.unwrap_or_default(),
            source: loaded.source,
        })
    }

    /// Load a typed file, keeping "absent" distinct from a default value.
    pub fn load_opt<T: FileModel>(&self) -> Result<Loaded<Option<T>>, StoreError> {
        self.load_with(T::FILE, |bytes| serde_json::from_slice::<T>(bytes))
    }

    fn load_with<T>(
        &self,
        f: DataFile,
        parse: impl Fn(&[u8]) -> serde_json::Result<T>,
    ) -> Result<Loaded<Option<T>>, StoreError> {
        let path = self.path(f);
        let bak = backup_path(&path);
        let mut quarantined = Vec::new();
        if let Some(bytes) = read_opt(&path)? {
            match parse(&bytes) {
                Ok(v) => {
                    return Ok(Loaded {
                        value: Some(v),
                        source: LoadSource::Primary,
                    });
                }
                Err(e) => {
                    tracing::warn!(file = %path.display(), error = %e, "unreadable; trying the backup");
                    quarantined.push(quarantine(&path)?);
                }
            }
        }
        match read_opt(&bak)? {
            Some(bytes) => match parse(&bytes) {
                Ok(v) => {
                    if quarantined.is_empty() {
                        tracing::warn!(file = %path.display(), "missing; read the backup");
                    }
                    Ok(Loaded {
                        value: Some(v),
                        source: LoadSource::Backup {
                            quarantined: quarantined.pop(),
                        },
                    })
                }
                Err(e) => {
                    tracing::error!(file = %bak.display(), error = %e, "backup unreadable too; starting empty");
                    quarantined.push(quarantine(&bak)?);
                    Ok(Loaded {
                        value: None,
                        source: LoadSource::Reset { quarantined },
                    })
                }
            },
            None if quarantined.is_empty() => Ok(Loaded {
                value: None,
                source: LoadSource::Missing,
            }),
            None => {
                tracing::error!(file = %path.display(), "unreadable and no backup; starting empty");
                Ok(Loaded {
                    value: None,
                    source: LoadSource::Reset { quarantined },
                })
            }
        }
    }

    /// Write a typed file atomically, keeping the previous version as `.bak`.
    pub fn save<T: FileModel>(&self, value: &T) -> Result<(), StoreError> {
        let path = self.path(T::FILE);
        let bytes = json::encode(value, T::FILE.style()).map_err(|e| StoreError::Json {
            path: path.clone(),
            source: e,
        })?;
        write_atomic(&path, &bytes, T::FILE.secret(), true)
    }

    /// Write raw JSON to a file atomically (with `.bak` rotation).
    pub fn save_value(&self, f: DataFile, value: &Value) -> Result<(), StoreError> {
        let path = self.path(f);
        let bytes = json::encode_value(value, f.style()).map_err(|e| StoreError::Json {
            path: path.clone(),
            source: e,
        })?;
        write_atomic(&path, &bytes, f.secret(), true)
    }

    /// The session guard (`SessionGuard.write`). Returns `Ok(false)` when the
    /// write was refused: an empty session before restore finished, over a
    /// file that still has tabs. When the count drops to half or less the
    /// old file is first copied to `session.previous.json`.
    pub fn save_session(
        &self,
        shape: &crate::session::SessionShape,
        restored: bool,
    ) -> Result<bool, StoreError> {
        let path = self.path(DataFile::Session);
        let old = read_opt(&path)?
            .and_then(|b| serde_json::from_slice::<crate::session::SessionShape>(&b).ok())
            .map(|s| s.tab_count())
            .unwrap_or(0);
        let new = shape.tab_count();
        if new == 0 && old > 0 && !restored {
            tracing::warn!(old, "refused empty session write before restore completed");
            return Ok(false);
        }
        if old > 0 && (new == 0 || new * 2 < old) {
            let previous = path.with_file_name("session.previous.json");
            if let Err(e) = fs::copy(&path, &previous) {
                tracing::warn!(error = %e, "could not keep session.previous.json");
            }
        }
        self.save(shape)?;
        Ok(true)
    }
}

fn read_opt(path: &Path) -> Result<Option<Vec<u8>>, StoreError> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::io(path, e)),
    }
}

/// Move a bad file aside as `<stem>.unreadable-<unix>[-n].json` (for a
/// `.bak`: `<stem>.bak.unreadable-…`). Returns the new path.
pub fn quarantine(path: &Path) -> Result<PathBuf, StoreError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix(".json").unwrap_or(&name);
    let stamp = crate::date::now_unix() as i64;
    let mut n = 0;
    loop {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let aside = path.with_file_name(format!("{stem}.unreadable-{stamp}{suffix}.json"));
        if !aside.exists() {
            fs::rename(path, &aside).map_err(|e| StoreError::io(path, e))?;
            return Ok(aside);
        }
        n += 1;
    }
}

/// Write `bytes` to `path` atomically: a sibling temporary (0600 when
/// `secret`), `fsync`, rename over `path`, `fsync` the folder. With
/// `keep_backup`, a current file that still parses as JSON is first copied to
/// `<path>.bak` (atomically too), so a corrupt file never replaces a good
/// backup.
pub fn write_atomic(
    path: &Path,
    bytes: &[u8],
    secret: bool,
    keep_backup: bool,
) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir).map_err(|e| StoreError::io(dir, e))?;
    if keep_backup
        && let Some(current) = read_opt(path)?
        && serde_json::from_slice::<serde::de::IgnoredAny>(&current).is_ok()
    {
        write_file(&backup_path(path), &current, secret)?;
    }
    write_file(path, bytes, secret)
}

fn write_file(path: &Path, bytes: &[u8], secret: bool) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(if secret { 0o600 } else { 0o644 });
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        // Windows has no mode bits. The store lives under the user's
        // `%LOCALAPPDATA%`, whose inherited ACL already grants only the user,
        // SYSTEM and Administrators — the same reach as 0600 under a 0700
        // home — so secret files rely on that ACL and get no extra DACL.
        #[cfg(windows)]
        let _ = secret;
        replace(&tmp, path)?;
        // Linux fsyncs the folder so the rename is durable. Windows cannot
        // open a folder this way (it needs FILE_FLAG_BACKUP_SEMANTICS), and
        // NTFS journals the rename itself, so the failed open is ignored.
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(StoreError::io(path, e));
    }
    Ok(())
}

/// Rename `tmp` over `path`.
#[cfg(not(windows))]
fn replace(tmp: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(tmp, path)
}

/// Rename `tmp` over `path` (std's rename replaces an existing file on
/// Windows too). Windows refuses the replace while another process holds
/// `path` open without `FILE_SHARE_DELETE` — typically an antivirus scan or
/// the search indexer, for a few milliseconds — with `ERROR_ACCESS_DENIED` or
/// `ERROR_SHARING_VIOLATION`. Retry those with a growing pause, about two
/// seconds in all; any other error, or one that outlasts the retries, is
/// returned.
#[cfg(windows)]
fn replace(tmp: &Path, path: &Path) -> std::io::Result<()> {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    const ATTEMPTS: u32 = 20;
    let mut attempt = 0;
    loop {
        match fs::rename(tmp, path) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt + 1 < ATTEMPTS
                    && matches!(
                        e.raw_os_error(),
                        Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
                    ) =>
            {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 * u64::from(attempt)));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(all(test, not(windows)))]
mod paths_tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn paths_from_any_environment() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            }
        };
        let p = Paths::from_vars(env(&[("HOME", "/home/a")]));
        assert_eq!(p.data, PathBuf::from("/home/a/.local/share/copper"));
        assert_eq!(p.config, PathBuf::from("/home/a/.config/copper"));
        let p = Paths::from_vars(env(&[
            ("HOME", "/home/a"),
            ("XDG_DATA_HOME", "/srv/data"),
            ("XDG_CONFIG_HOME", "relative/ignored"),
            ("COPPER_WORLD", "Bot_1a2b"),
        ]));
        assert_eq!(p.data, PathBuf::from("/srv/data/copper-bot1a2b"));
        assert_eq!(p.config, PathBuf::from("/home/a/.config/copper-bot1a2b"));
        let p = Paths::from_vars(env(&[("HOME", "/h"), ("COPPER_WORLD", "__")]));
        assert_eq!(p.data, PathBuf::from("/h/.local/share/copper-test"));
    }
}
