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

/// A rectangle in view pixels: CSS pixels times the page zoom, the
/// coordinates of a scale-1 viewport picture (and of
/// [`crate::PageView::view_size`]). Equal to CSS pixels at 100 %.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ViewRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ViewRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        ViewRect {
            x,
            y,
            width,
            height,
        }
    }
}

/// What a [`crate::PageView::capture`] paints.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureArea {
    /// The viewport, as on screen.
    Viewport,
    /// The whole scrollable document.
    Document,
    /// Part of the viewport, from its top-left (inside it).
    ViewportRect(ViewRect),
    /// Part of the document, from its top-left (inside it), wherever the
    /// page is scrolled to.
    DocumentRect(ViewRect),
}

impl CaptureArea {
    /// Whether the area is measured from the document's top-left (else
    /// from the viewport's).
    pub fn in_document(self) -> bool {
        matches!(self, CaptureArea::Document | CaptureArea::DocumentRect(_))
    }

    /// The rectangle asked for, if any.
    pub fn rect(self) -> Option<ViewRect> {
        match self {
            CaptureArea::ViewportRect(r) | CaptureArea::DocumentRect(r) => Some(r),
            CaptureArea::Viewport | CaptureArea::Document => None,
        }
    }
}

/// [`crate::PageView::capture`]'s question: an area at a scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureRequest {
    pub area: CaptureArea,
    /// Picture pixels wanted per view pixel (the caller keeps it sane: 0.01
    /// to 4). An engine paints at least this where it can, re-rendering
    /// the page rather than enlarging pixels.
    pub scale: f64,
}

/// What [`crate::PageView::capture`] painted. It may cover more than the
/// area asked for (the whole viewport or document); the caller crops
/// `origin`-relative and scales to the size it wants.
#[derive(Clone, PartialEq)]
pub struct Capture {
    pub bitmap: Bitmap,
    /// Where the bitmap's top-left pixel sits in the area's coordinates
    /// (view pixels from the viewport's or the document's top-left).
    pub origin: (f64, f64),
    /// Bitmap pixels per view pixel: the asked scale or more when the
    /// engine rendered it, less when it could not (the caller then
    /// enlarges, and says so).
    pub scale: f64,
}

impl Capture {
    /// Keep only the pixels `rect` (view pixels in the area's coordinates)
    /// needs, plus one around it for scaling, so a small region of a big
    /// document leaves the UI thread small. Unchanged when `rect` misses.
    pub fn cropped(self, rect: ViewRect) -> Capture {
        let s = self.scale;
        if !(s.is_finite() && s > 0.0) {
            return self;
        }
        let (ox, oy) = self.origin;
        let x0 = ((rect.x - ox) * s).floor() - 1.0;
        let y0 = ((rect.y - oy) * s).floor() - 1.0;
        let x1 = ((rect.x + rect.width - ox) * s).ceil() + 1.0;
        let y1 = ((rect.y + rect.height - oy) * s).ceil() + 1.0;
        let x0 = x0.max(0.0);
        let y0 = y0.max(0.0);
        let x1 = x1.min(f64::from(self.bitmap.width));
        let y1 = y1.min(f64::from(self.bitmap.height));
        if x1 <= x0 || y1 <= y0 {
            return self;
        }
        let full = x0 == 0.0
            && y0 == 0.0
            && x1 == f64::from(self.bitmap.width)
            && y1 == f64::from(self.bitmap.height);
        if full {
            return self;
        }
        match self
            .bitmap
            .crop(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32)
        {
            Some(bitmap) => Capture {
                bitmap,
                origin: (ox + x0 / s, oy + y0 / s),
                scale: s,
            },
            None => self,
        }
    }
}

impl fmt::Debug for Capture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Capture({:?} at {:?}, scale {})",
            self.bitmap, self.origin, self.scale
        )
    }
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

/// Raw pixels: 8-bit RGBA with straight (not premultiplied) alpha, top row
/// first, rows `stride` bytes apart (`stride >= width * 4`).
///
/// What [`crate::PageView::snapshot_bitmap`] hands back for callers that
/// crop, scale or encode on a worker thread (canvas frame posters), so the
/// UI thread never PNG-encodes a whole window.
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: usize,
    /// `stride * height` bytes.
    pub rgba: Vec<u8>,
}

impl Bitmap {
    /// `width` x `height` pixels from (`x`, `y`), clipped to the bitmap,
    /// rows packed; `None` when nothing is left.
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> Option<Bitmap> {
        let x1 = x.saturating_add(width).min(self.width);
        let y1 = y.saturating_add(height).min(self.height);
        if x >= x1 || y >= y1 {
            return None;
        }
        let stride = (x1 - x) as usize * 4;
        let mut rgba = Vec::with_capacity(stride * (y1 - y) as usize);
        for row in y..y1 {
            rgba.extend_from_slice(self.row(row)?.get(x as usize * 4..x1 as usize * 4)?);
        }
        Some(Bitmap {
            width: x1 - x,
            height: y1 - y,
            stride,
            rgba,
        })
    }

    /// The bytes of row `y` (`width * 4` of them), if it exists.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let start = self.stride.checked_mul(y as usize)?;
        let len = (self.width as usize).checked_mul(4)?;
        self.rgba.get(start..start.checked_add(len)?)
    }
}

impl fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Bitmap({}x{}, stride {}, {} bytes)",
            self.width,
            self.height,
            self.stride,
            self.rgba.len()
        )
    }
}

/// A request for a URL of a scheme registered with
/// [`crate::Engine::register_scheme`] (`copper://canvas/<id>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemeRequest {
    /// The full URL asked for.
    pub uri: String,
    /// The page that asked, when it is one this engine made (`None` for an
    /// engine-internal view such as the inspector).
    pub page: Option<PageId>,
}

/// A successful answer to a [`SchemeRequest`].
#[derive(Clone, PartialEq, Eq)]
pub struct SchemeBody {
    pub data: Vec<u8>,
    /// The content type (`text/html; charset=utf-8`).
    pub mime: String,
}

impl fmt::Debug for SchemeBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SchemeBody({}, {} bytes)", self.mime, self.data.len())
    }
}

/// What a scheme handler answers: the body, or why there is none (the page
/// sees a failed load with that text).
pub type SchemeResponse = std::result::Result<SchemeBody, String>;

/// Answers requests for one custom scheme, synchronously, on the main
/// thread.
pub type SchemeHandler = Box<dyn Fn(&SchemeRequest) -> SchemeResponse>;

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
id_type!(
    /// Process-unique identity of a page: the same for every clone of one
    /// page, never reused for another, never 0. Lets the shell tell "the
    /// tab's page now" from "the page it had before a crash or a sleep"
    /// without holding the page.
    PageId
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

    /// A bitmap whose pixel (x, y) is `[x, y, 0, 255]`.
    fn ramp(width: u32, height: u32, stride: usize) -> Bitmap {
        let mut rgba = vec![0u8; stride * height as usize];
        for y in 0..height {
            for x in 0..width {
                let i = y as usize * stride + x as usize * 4;
                rgba[i..i + 4].copy_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        Bitmap {
            width,
            height,
            stride,
            rgba,
        }
    }

    #[test]
    fn bitmap_crop() {
        let b = ramp(10, 8, 48);
        let c = b.crop(2, 3, 4, 2).unwrap();
        assert_eq!((c.width, c.height, c.stride), (4, 2, 16));
        assert_eq!(&c.rgba[..4], &[2, 3, 0, 255]);
        assert_eq!(c.row(1).unwrap()[12..16], [5, 4, 0, 255]);
        let edge = b.crop(8, 6, 10, 10).unwrap();
        assert_eq!((edge.width, edge.height), (2, 2));
        assert!(b.crop(10, 0, 1, 1).is_none());
    }

    #[test]
    fn capture_crops_to_what_a_rect_needs() {
        // A document picture at scale 2 whose top-left is at (0, 0).
        let shot = Capture {
            bitmap: ramp(100, 100, 400),
            origin: (0.0, 0.0),
            scale: 2.0,
        };
        let c = shot.clone().cropped(ViewRect::new(10.0, 20.0, 5.0, 5.0));
        // 20..30 x 40..50 device pixels, one more on each side.
        assert_eq!((c.bitmap.width, c.bitmap.height), (12, 12));
        assert_eq!(c.origin, (9.5, 19.5));
        assert_eq!(&c.bitmap.rgba[..2], &[19, 39]);
        // A rect that covers it all, or misses it, keeps it whole.
        assert_eq!(
            shot.clone().cropped(ViewRect::new(0.0, 0.0, 50.0, 50.0)),
            shot
        );
        assert_eq!(
            shot.clone().cropped(ViewRect::new(500.0, 0.0, 5.0, 5.0)),
            shot
        );
    }

    #[test]
    fn world_names() {
        assert_eq!(World::Page.name(), None);
        assert_eq!(World::isolated("copper").name(), Some("copper"));
    }
}
