//! Settings, keyed exactly like the Mac's `UserDefaults` (Mac: `Prefs.swift`,
//! `Shield.swift`, `Sleep.swift`, `Fork/Sections.swift`,
//! `Fork/Cloud/CloudMerge.swift` `CloudSettingsKeys`).
//!
//! Linux and Windows have no `UserDefaults`, so the same keys live in one
//! flat JSON object, `$XDG_CONFIG_HOME/copper/settings.json` (Windows:
//! `%LOCALAPPDATA%\Copper\Data\settings.json`;
//! [`crate::store::DataFile::Settings`]).
//! Keys this build does not know survive untouched. Typed accessors apply
//! the Mac's defaults; the cloud `settings` document carries the
//! [`CLOUD_SETTINGS_KEYS`] subset as strings.
//!
//! Linux-only keys (ignored by a Mac): `search.engine`.

use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Mac setting keys.
pub mod keys {
    /// `light` | `dark` | `system`. Default `light`.
    pub const LOOK: &str = "look";
    /// Tabs down the left (true) or across the top (false). Default false;
    /// legacy `manner == "side"` means true.
    pub const SIDEBAR: &str = "sidebar";
    /// Legacy layout key read when `sidebar` is absent.
    pub const MANNER: &str = "manner";
    /// Column width in points.
    pub const SIDEBAR_WIDTH: &str = "sidebar.width";
    /// `letters` | `icons`. Default `icons`: a site's own mark, its letter
    /// only where it has none.
    pub const GLYPH: &str = "glyph";
    /// `row` | `recent`. Default `row`.
    pub const TABS_SWITCHING: &str = "tabs.switching";
    /// `system` | `natural` | `inverted`. Default `system`.
    pub const SPACES_SWIPE: &str = "spaces.swipe";
    /// Put idle tabs to sleep. Default true.
    pub const TABS_SLEEP: &str = "tabs.sleep";
    /// Seconds before an idle tab sleeps (0/absent = 1800).
    pub const SLEEP_AFTER: &str = "sleep.after";
    /// How many pages stay loaded however long they are left (0/absent:
    /// from the computer's memory). For measurements, as on the Mac.
    pub const SLEEP_KEEP: &str = "sleep.keep";
    /// Ad blocker on. Default true.
    pub const SHIELD: &str = "shield";
    /// Hosts the ad blocker is off for, sorted.
    pub const SHIELD_PAUSED: &str = "shield.paused";
    /// Passkeys (not on Linux v1).
    pub const PASSKEYS: &str = "passkeys";
    /// Download folder path.
    pub const DOWNLOADS: &str = "downloads";
    /// Ask where to save each download. Default false.
    pub const DOWNLOADS_ASK: &str = "downloads.ask";
    /// Offer to save passwords. Default true.
    pub const PASSWORDS_SAVE: &str = "passwords.save";
    /// Fill saved passwords. Default true.
    pub const PASSWORDS_FILL: &str = "passwords.fill";
    /// Fill identities, cards, fields too. Default true.
    pub const AUTOFILL_EVERYTHING: &str = "autofill.everything";
    /// Where new passwords go.
    pub const PASSWORDS_BACKEND: &str = "passwords.backend";
    /// The welcome has been shown.
    pub const WELCOMED: &str = "welcomed";
    /// Spelling correction in pages. Default false.
    pub const AUTOCORRECT: &str = "autocorrect";
    /// `h12` | `h24` | `h48` | `never`. Default `h24`.
    pub const SECTIONS_ARCHIVE: &str = "sections.archive";
    /// `zoom.<host>`: a site's zoom (number; absent = 100%; Mac
    /// `Tab.rememberZoom`, host as `URL.host()` gives it, `www.` and all).
    /// Not synced (not in `CloudSettingsKeys` on the Mac).
    pub const ZOOM_PREFIX: &str = "zoom.";
    /// Last open settings page.
    pub const SETTINGS_PAGE: &str = "settings.page";
    /// Linux only: search engine id (see [`crate::address::ENGINES`]).
    pub const SEARCH_ENGINE: &str = "search.engine";
}

/// The keys the cloud `settings` document may carry (`CloudSettingsKeys.allowed`).
pub const CLOUD_SETTINGS_KEYS: [&str; 9] = [
    keys::LOOK,
    keys::SIDEBAR,
    keys::GLYPH,
    keys::TABS_SWITCHING,
    keys::TABS_SLEEP,
    keys::SPACES_SWIPE,
    keys::SHIELD,
    keys::AUTOCORRECT,
    keys::SECTIONS_ARCHIVE,
];

/// Light, dark, or follow the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Look {
    #[default]
    Light,
    Dark,
    System,
}

/// What a tab wears beside its title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Glyph {
    Letters,
    #[default]
    Icons,
}

/// What Ctrl+Tab chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabSwitching {
    #[default]
    Row,
    Recent,
}

/// When Today tabs are archived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArchiveAfter {
    H12,
    #[default]
    H24,
    H48,
    Never,
}

/// Where the tabs are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabsPosition {
    /// Across the top (Mac `sidebar == false`).
    #[default]
    Top,
    /// Down the left (Mac `sidebar == true`).
    Left,
}

macro_rules! string_enum {
    ($t:ty { $($v:ident = $s:literal),* $(,)? }) => {
        impl $t {
            /// The stored string.
            pub fn as_str(self) -> &'static str {
                match self { $(<$t>::$v => $s),* }
            }
            /// From the stored string.
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some(<$t>::$v),)* _ => None }
            }
        }
    };
}
string_enum!(Look { Light = "light", Dark = "dark", System = "system" });
string_enum!(Glyph { Letters = "letters", Icons = "icons" });
string_enum!(TabSwitching { Row = "row", Recent = "recent" });
string_enum!(ArchiveAfter { H12 = "h12", H24 = "h24", H48 = "h48", Never = "never" });

impl ArchiveAfter {
    /// The window in hours; `None` for never (`Sections.Archive.hours`).
    pub fn hours(self) -> Option<f64> {
        match self {
            ArchiveAfter::H12 => Some(12.0),
            ArchiveAfter::H24 => Some(24.0),
            ArchiveAfter::H48 => Some(48.0),
            ArchiveAfter::Never => None,
        }
    }
}

/// The settings object. Mutations bump [`Settings::generation`]; persist
/// [`Settings::values`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    values: Map<String, Value>,
    generation: u64,
}

impl Settings {
    /// From the file's object.
    pub fn new(values: Map<String, Value>) -> Self {
        Settings {
            values,
            generation: 0,
        }
    }

    /// Every stored key, as written to the file.
    pub fn values(&self) -> &Map<String, Value> {
        &self.values
    }

    /// Bumped on every change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A raw value.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Set a raw value (no-op if unchanged).
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        if self.values.get(key) != Some(&value) {
            self.values.insert(key.to_string(), value);
            self.generation += 1;
        }
    }

    /// Remove a key.
    pub fn remove(&mut self, key: &str) {
        if self.values.remove(key).is_some() {
            self.generation += 1;
        }
    }

    /// A bool, or `default`. Accepts `"true"`/`"false"` strings and 0/1 too.
    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        match self.values.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) if s == "true" || s == "false" => s == "true",
            Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(default),
            _ => default,
        }
    }

    /// A string, if stored as one.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.values.get(key).and_then(Value::as_str)
    }

    /// A number, if stored as one.
    pub fn number(&self, key: &str) -> Option<f64> {
        self.values.get(key).and_then(Value::as_f64)
    }

    /// Light/dark/system (default light).
    pub fn look(&self) -> Look {
        self.string(keys::LOOK)
            .and_then(Look::parse)
            .unwrap_or_default()
    }
    pub fn set_look(&mut self, v: Look) {
        self.set(keys::LOOK, v.as_str());
    }

    /// Tabs on top or on the left.
    pub fn tabs_position(&self) -> TabsPosition {
        let left = match self.values.get(keys::SIDEBAR) {
            Some(_) => self.bool_or(keys::SIDEBAR, false),
            None => self.string(keys::MANNER) == Some("side"),
        };
        if left {
            TabsPosition::Left
        } else {
            TabsPosition::Top
        }
    }
    pub fn set_tabs_position(&mut self, p: TabsPosition) {
        self.set(keys::SIDEBAR, p == TabsPosition::Left);
    }

    /// Column width, if one was dragged.
    pub fn sidebar_width(&self) -> Option<f64> {
        self.number(keys::SIDEBAR_WIDTH)
    }
    pub fn set_sidebar_width(&mut self, w: f64) {
        self.set(keys::SIDEBAR_WIDTH, w);
    }

    pub fn glyph(&self) -> Glyph {
        self.string(keys::GLYPH)
            .and_then(Glyph::parse)
            .unwrap_or_default()
    }
    pub fn set_glyph(&mut self, v: Glyph) {
        self.set(keys::GLYPH, v.as_str());
    }

    pub fn tab_switching(&self) -> TabSwitching {
        self.string(keys::TABS_SWITCHING)
            .and_then(TabSwitching::parse)
            .unwrap_or_default()
    }
    pub fn set_tab_switching(&mut self, v: TabSwitching) {
        self.set(keys::TABS_SWITCHING, v.as_str());
    }

    /// Idle tabs sleep (default true).
    pub fn sleeps_tabs(&self) -> bool {
        self.bool_or(keys::TABS_SLEEP, true)
    }
    pub fn set_sleeps_tabs(&mut self, on: bool) {
        self.set(keys::TABS_SLEEP, on);
    }

    /// Seconds before an idle tab sleeps (default 30 minutes).
    pub fn sleep_after_secs(&self) -> f64 {
        self.number(keys::SLEEP_AFTER)
            .filter(|s| *s > 0.0)
            .unwrap_or(1800.0)
    }

    /// `sleep.after` was set by hand (measurements): every tab past it
    /// sleeps, whatever the memory budget says (Mac `sleepIdle`).
    pub fn sleep_after_set(&self) -> bool {
        self.number(keys::SLEEP_AFTER).is_some_and(|s| s > 0.0)
    }

    /// `sleep.keep`, when set: how many pages stay loaded.
    pub fn sleep_keep(&self) -> Option<usize> {
        self.number(keys::SLEEP_KEEP)
            .filter(|n| *n >= 1.0)
            .map(|n| n as usize)
    }

    /// The ad blocker (default on).
    pub fn shield(&self) -> bool {
        self.bool_or(keys::SHIELD, true)
    }
    pub fn set_shield(&mut self, on: bool) {
        self.set(keys::SHIELD, on);
    }

    /// Hosts the ad blocker is off for.
    pub fn shield_paused(&self) -> Vec<String> {
        match self.values.get(keys::SHIELD_PAUSED) {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }
    /// Whether the ad blocker is off on `host`.
    pub fn shield_paused_on(&self, host: &str) -> bool {
        self.shield_paused().iter().any(|h| h == host)
    }
    /// Turn the ad blocker off (or back on) for one host; stored sorted.
    pub fn set_shield_paused(&mut self, host: &str, off: bool) {
        let mut hosts = self.shield_paused();
        hosts.retain(|h| h != host);
        if off {
            hosts.push(host.to_string());
        }
        hosts.sort();
        hosts.dedup();
        self.set(keys::SHIELD_PAUSED, Value::from(hosts));
    }
    /// Whether the shield applies to `host`.
    pub fn shield_active_on(&self, host: &str) -> bool {
        self.shield() && !self.shield_paused_on(host)
    }

    /// The download folder, if chosen.
    pub fn downloads_dir(&self) -> Option<&str> {
        self.string(keys::DOWNLOADS)
    }
    pub fn set_downloads_dir(&mut self, path: &str) {
        self.set(keys::DOWNLOADS, path);
    }
    pub fn downloads_ask(&self) -> bool {
        self.bool_or(keys::DOWNLOADS_ASK, false)
    }
    pub fn saves_passwords(&self) -> bool {
        self.bool_or(keys::PASSWORDS_SAVE, true)
    }
    pub fn fills_passwords(&self) -> bool {
        self.bool_or(keys::PASSWORDS_FILL, true)
    }
    pub fn fills_everything(&self) -> bool {
        self.bool_or(keys::AUTOFILL_EVERYTHING, true)
    }
    pub fn welcomed(&self) -> bool {
        self.bool_or(keys::WELCOMED, false) || self.values.contains_key(keys::GLYPH)
    }
    pub fn autocorrect(&self) -> bool {
        self.bool_or(keys::AUTOCORRECT, false)
    }
    pub fn archive_after(&self) -> ArchiveAfter {
        self.string(keys::SECTIONS_ARCHIVE)
            .and_then(ArchiveAfter::parse)
            .unwrap_or_default()
    }
    pub fn set_archive_after(&mut self, v: ArchiveAfter) {
        self.set(keys::SECTIONS_ARCHIVE, v.as_str());
    }

    /// The zoom kept for `host` (1.0 when none).
    pub fn zoom_for(&self, host: &str) -> f64 {
        self.number(&format!("{}{host}", keys::ZOOM_PREFIX))
            .unwrap_or(1.0)
    }
    /// Keep `zoom` for `host`; 100% forgets it (Mac `Tab.rememberZoom`).
    pub fn set_zoom_for(&mut self, host: &str, zoom: f64) {
        let key = format!("{}{host}", keys::ZOOM_PREFIX);
        if (zoom - 1.0).abs() < 0.01 {
            self.remove(&key);
        } else {
            self.set(&key, zoom);
        }
    }

    /// The search engine (Linux addition; Google by default).
    pub fn search_engine(&self) -> &'static crate::address::SearchEngine {
        crate::address::SearchEngine::by_id(self.string(keys::SEARCH_ENGINE).unwrap_or("google"))
    }
    pub fn set_search_engine(&mut self, id: &str) {
        self.set(keys::SEARCH_ENGINE, id);
    }

    /// The cloud `settings` document's values for this device: the allowed
    /// keys that are set, as strings (`CloudApply`).
    pub fn cloud_values(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        out.insert(keys::LOOK.into(), self.look().as_str().into());
        out.insert(
            keys::SIDEBAR.into(),
            (self.tabs_position() == TabsPosition::Left).to_string(),
        );
        out.insert(keys::GLYPH.into(), self.glyph().as_str().into());
        out.insert(
            keys::TABS_SWITCHING.into(),
            self.tab_switching().as_str().into(),
        );
        out.insert(keys::TABS_SLEEP.into(), self.sleeps_tabs().to_string());
        out.insert(
            keys::SPACES_SWIPE.into(),
            self.string(keys::SPACES_SWIPE)
                .unwrap_or("system")
                .to_string(),
        );
        out.insert(keys::SHIELD.into(), self.shield().to_string());
        out.insert(keys::AUTOCORRECT.into(), self.autocorrect().to_string());
        out.insert(
            keys::SECTIONS_ARCHIVE.into(),
            self.archive_after().as_str().into(),
        );
        filter_cloud_values(&out)
    }

    /// Apply a cloud document's values (filtered first). Booleans are stored
    /// as booleans, enums as strings.
    pub fn apply_cloud_values(&mut self, values: &BTreeMap<String, String>) {
        for (k, v) in filter_cloud_values(values) {
            match k.as_str() {
                keys::SIDEBAR | keys::TABS_SLEEP | keys::SHIELD | keys::AUTOCORRECT => {
                    self.set(&k, v == "true")
                }
                _ => self.set(&k, v),
            }
        }
    }
}

/// Only the allowed keys, and only values of the expected shape
/// (`CloudSettingsKeys.filter`).
pub fn filter_cloud_values(values: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    values
        .iter()
        .filter(|(k, v)| {
            CLOUD_SETTINGS_KEYS.contains(&k.as_str())
                && match k.as_str() {
                    keys::LOOK => ["light", "dark", "system"].contains(&v.as_str()),
                    keys::GLYPH => ["letters", "icons"].contains(&v.as_str()),
                    keys::TABS_SWITCHING => ["row", "recent"].contains(&v.as_str()),
                    keys::SPACES_SWIPE => ["system", "natural", "inverted"].contains(&v.as_str()),
                    keys::SECTIONS_ARCHIVE => ["h12", "h24", "h48", "never"].contains(&v.as_str()),
                    _ => v.as_str() == "true" || v.as_str() == "false",
                }
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let mut s = Settings::default();
        assert_eq!(s.look(), Look::Light);
        assert!(s.shield());
        assert_eq!(s.tabs_position(), TabsPosition::Top);
        s.set(keys::MANNER, "side");
        assert_eq!(s.tabs_position(), TabsPosition::Left);
        s.set_tabs_position(TabsPosition::Top);
        assert_eq!(s.tabs_position(), TabsPosition::Top);
        s.set_shield_paused("b.test", true);
        s.set_shield_paused("a.test", true);
        assert_eq!(s.shield_paused(), vec!["a.test", "b.test"]);
        assert!(!s.shield_active_on("a.test"));
        s.set_shield_paused("a.test", false);
        assert!(s.shield_active_on("a.test"));
        assert_eq!(s.search_engine().id, "google");
        let mut cloud = BTreeMap::new();
        cloud.insert("look".to_string(), "dark".to_string());
        cloud.insert("glyph".to_string(), "weird".to_string());
        cloud.insert("passkeys".to_string(), "true".to_string());
        cloud.insert("shield".to_string(), "false".to_string());
        s.apply_cloud_values(&cloud);
        assert_eq!(s.look(), Look::Dark);
        assert!(!s.shield());
        assert!(s.get("passkeys").is_none());
        assert_eq!(s.cloud_values()["look"], "dark");
    }

    #[test]
    fn zoom_is_kept_per_host_and_forgotten_at_100() {
        let mut s = Settings::default();
        assert_eq!(s.zoom_for("www.example.com"), 1.0);
        s.set_zoom_for("www.example.com", 1.25);
        assert_eq!(s.number("zoom.www.example.com"), Some(1.25));
        assert_eq!(s.zoom_for("www.example.com"), 1.25);
        assert_eq!(s.zoom_for("example.com"), 1.0);
        assert!(!s.cloud_values().contains_key("zoom.www.example.com"));
        s.set_zoom_for("www.example.com", 1.0);
        assert!(s.get("zoom.www.example.com").is_none());
        assert_eq!(ArchiveAfter::H48.hours(), Some(48.0));
        assert_eq!(ArchiveAfter::Never.hours(), None);
    }
}
