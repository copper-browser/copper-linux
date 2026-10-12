//! Plain value types shared by every engine implementation.

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use serde::{Deserialize, Serialize};

use crate::{EngineError, Result};

/// Identifies a profile (a cookie/storage jar). Used as a directory name
/// under `$XDG_DATA_HOME/copper/profiles/<id>`, so only `[A-Za-z0-9._-]`
/// is accepted, at most 128 characters, and it may not start with a dot.
/// Mac profile ids are UUID strings, which pass.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileId(String);

impl ProfileId {
    /// Validate and wrap a profile id.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        let ok_chars = id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if id.is_empty() || id.len() > 128 || !ok_chars || id.starts_with('.') {
            return Err(EngineError::InvalidArgument(format!(
                "profile id {id:?} must be 1-128 chars of [A-Za-z0-9._-] and not start with '.'"
            )));
        }
        Ok(ProfileId(id))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProfileId {
    type Error = EngineError;
    fn try_from(s: String) -> Result<Self> {
        ProfileId::new(s)
    }
}

impl From<ProfileId> for String {
    fn from(id: ProfileId) -> String {
        id.0
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The JavaScript world a script runs in. `Page` is the page's own world
/// (sees and can be seen by page scripts); `Isolated(name)` is a named world
/// that shares the DOM but not JS globals with the page. Globals set in an
/// isolated world persist across calls until the document changes, like the
/// page world's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum World {
    #[default]
    Page,
    Isolated(String),
}

impl World {
    /// A named isolated world.
    pub fn isolated(name: impl Into<String>) -> Self {
        World::Isolated(name.into())
    }

    /// The world name an engine API wants (`None` for the page world).
    pub fn name(&self) -> Option<&str> {
        match self {
            World::Page => None,
            World::Isolated(n) => Some(n),
        }
    }
}

/// Which part of the page a snapshot covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotRegion {
    /// What is on screen right now (view size).
    Visible,
    /// The whole scrollable document.
    FullDocument,
}

/// A PNG image with its pixel size. Used for snapshots and favicons.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// PNG-encoded bytes.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Image({}x{}, {} PNG bytes)",
            self.width,
            self.height,
            self.png.len()
        )
    }
}

/// When a user script runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionTime {
    /// Before any page script, as soon as the document element exists.
    DocumentStart,
    /// After the document is parsed (DOMContentLoaded time).
    #[default]
    DocumentEnd,
}

/// Which frames user content is injected into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frames {
    #[default]
    All,
    TopOnly,
}

/// Cascade level of a user style sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleLevel {
    /// Like a user's own style sheet (page `!important` still loses to user `!important`).
    #[default]
    User,
    /// Like an author style sheet appended to the page's own.
    Author,
}

/// A script injected into every matching document of a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserScript {
    pub source: String,
    #[serde(default)]
    pub injection_time: InjectionTime,
    #[serde(default)]
    pub frames: Frames,
    #[serde(default)]
    pub world: World,
    /// URL patterns (WebKit match patterns, e.g. `https://*.example.com/*`);
    /// empty means every URL.
    #[serde(default)]
    pub allow_list: Vec<String>,
    #[serde(default)]
    pub block_list: Vec<String>,
}

impl UserScript {
    /// Document-end, all frames, page world, every URL.
    pub fn new(source: impl Into<String>) -> Self {
        UserScript {
            source: source.into(),
            injection_time: InjectionTime::default(),
            frames: Frames::default(),
            world: World::default(),
            allow_list: Vec::new(),
            block_list: Vec::new(),
        }
    }

    pub fn at(mut self, time: InjectionTime) -> Self {
        self.injection_time = time;
        self
    }

    pub fn frames(mut self, frames: Frames) -> Self {
        self.frames = frames;
        self
    }

    pub fn world(mut self, world: World) -> Self {
        self.world = world;
        self
    }
}

/// A style sheet injected into every matching document of a page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserStyleSheet {
    pub source: String,
    #[serde(default)]
    pub frames: Frames,
    #[serde(default)]
    pub level: StyleLevel,
    #[serde(default)]
    pub world: World,
    #[serde(default)]
    pub allow_list: Vec<String>,
    #[serde(default)]
    pub block_list: Vec<String>,
}

impl UserStyleSheet {
    /// All frames, user level, page world, every URL.
    pub fn new(source: impl Into<String>) -> Self {
        UserStyleSheet {
            source: source.into(),
            frames: Frames::default(),
            level: StyleLevel::default(),
            world: World::default(),
            allow_list: Vec::new(),
            block_list: Vec::new(),
        }
    }

    pub fn frames(mut self, frames: Frames) -> Self {
        self.frames = frames;
        self
    }

    pub fn level(mut self, level: StyleLevel) -> Self {
        self.level = level;
        self
    }
}

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Handle for a user script added to a page.
    ScriptId
);
id_type!(
    /// Handle for a user style sheet added to a page.
    StyleSheetId
);
id_type!(
    /// Process-unique id of a download.
    DownloadId
);
id_type!(
    /// Id of a pending permission request; answer it with `PageView::answer_permission`.
    PermissionId
);

/// Options for in-page find.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FindOptions {
    pub case_sensitive: bool,
    pub backwards: bool,
    pub wrap_around: bool,
    pub at_word_starts: bool,
    /// Matches counted at most (WebKit stops counting here).
    pub max_matches: u32,
}

impl Default for FindOptions {
    fn default() -> Self {
        FindOptions {
            case_sensitive: false,
            backwards: false,
            wrap_around: true,
            at_word_starts: false,
            max_matches: 1000,
        }
    }
}

/// `SameSite` policy of a cookie.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameSite {
    None,
    #[default]
    Lax,
    Strict,
}

/// A cookie as stored in a profile's jar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// Host (`example.com`) or domain (`.example.com`) the cookie belongs to.
    pub domain: String,
    #[serde(default = "root_path")]
    pub path: String,
    /// Expiry as Unix seconds; `None` is a session cookie.
    #[serde(default)]
    pub expires: Option<i64>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    #[serde(default)]
    pub same_site: SameSite,
}

fn root_path() -> String {
    "/".into()
}

impl Cookie {
    /// A session cookie for `domain`, path `/`, `SameSite=Lax`.
    pub fn new(
        name: impl Into<String>,
        value: impl Into<String>,
        domain: impl Into<String>,
    ) -> Self {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            expires: None,
            secure: false,
            http_only: false,
            same_site: SameSite::Lax,
        }
    }

    pub fn with_same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }
}

/// A set of website-data kinds (bit set) for `Profile::clear_website_data`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WebsiteDataKinds(u32);

impl WebsiteDataKinds {
    pub const MEMORY_CACHE: Self = Self(1 << 0);
    pub const DISK_CACHE: Self = Self(1 << 1);
    pub const COOKIES: Self = Self(1 << 2);
    pub const LOCAL_STORAGE: Self = Self(1 << 3);
    pub const SESSION_STORAGE: Self = Self(1 << 4);
    pub const INDEXEDDB: Self = Self(1 << 5);
    pub const SERVICE_WORKERS: Self = Self(1 << 6);
    /// Cache API storage (`caches`).
    pub const DOM_CACHE: Self = Self(1 << 7);
    pub const HSTS: Self = Self(1 << 8);
    pub const OFFLINE_APP_CACHE: Self = Self(1 << 9);
    /// What a hard reload clears: the memory and disk HTTP caches only.
    pub const CACHES: Self = Self(Self::MEMORY_CACHE.0 | Self::DISK_CACHE.0);
    pub const ALL: Self = Self((1 << 10) - 1);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for WebsiteDataKinds {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for WebsiteDataKinds {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Per-page options fixed at creation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PageOptions {
    /// Override the engine's user agent.
    pub user_agent: Option<String>,
    /// Enable the web inspector (`PageView::show_inspector`).
    pub developer_extras: bool,
    /// Report `console.*` calls and uncaught errors as `PageEvent::Console`.
    /// Works by wrapping `console` in the page world, so it is opt-in.
    pub capture_console: bool,
    /// Let script call `window.open` without a user gesture (popup blocker off).
    pub allow_script_popups: bool,
    /// Initial page zoom (1.0 = 100 %).
    pub zoom: f64,
    /// Start muted.
    pub muted: bool,
}

impl Default for PageOptions {
    fn default() -> Self {
        PageOptions {
            user_agent: None,
            developer_extras: false,
            capture_console: false,
            allow_script_popups: false,
            zoom: 1.0,
            muted: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_ids() {
        assert!(ProfileId::new("default").is_ok());
        assert!(ProfileId::new("6F9619FF-8B86-D011-B42D-00C04FC964FF").is_ok());
        assert!(ProfileId::new("work_2.v1").is_ok());
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "a b",
            "ü",
            &"x".repeat(129),
        ] {
            assert!(ProfileId::new(bad).is_err(), "{bad:?} should be rejected");
        }
        let json = serde_json::to_string(&ProfileId::new("p1").unwrap()).unwrap();
        assert_eq!(json, "\"p1\"");
        assert!(serde_json::from_str::<ProfileId>("\"../x\"").is_err());
    }

    #[test]
    fn data_kinds() {
        let k = WebsiteDataKinds::COOKIES | WebsiteDataKinds::DISK_CACHE;
        assert!(k.contains(WebsiteDataKinds::COOKIES));
        assert!(!k.contains(WebsiteDataKinds::CACHES));
        assert!(WebsiteDataKinds::ALL.contains(WebsiteDataKinds::OFFLINE_APP_CACHE));
        assert!(WebsiteDataKinds::empty().is_empty());
    }

    #[test]
    fn partial_json() {
        let o: PageOptions = serde_json::from_str(r#"{"capture_console": true}"#).unwrap();
        assert!(o.capture_console && o.zoom == 1.0);
        let c: Cookie =
            serde_json::from_str(r#"{"name": "a", "value": "b", "domain": "x.com"}"#).unwrap();
        assert_eq!(
            c,
            Cookie::new("a", "b", "x.com").with_same_site(SameSite::Lax)
        );
        let s: UserScript = serde_json::from_str(r#"{"source": "1"}"#).unwrap();
        assert_eq!(s, UserScript::new("1"));
        let w: World = serde_json::from_str(r#"{"isolated": "copper"}"#).unwrap();
        assert_eq!(w, World::isolated("copper"));
    }

    #[test]
    fn world_names() {
        assert_eq!(World::Page.name(), None);
        assert_eq!(World::isolated("copper").name(), Some("copper"));
    }
}
