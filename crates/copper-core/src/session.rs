//! `session.json`: what was open, in which space (Mac: `Session.swift`,
//! `Fork/Spaces.swift`, `Fork/SpaceTheme.swift`).
//!
//! These are the on-disk shapes. The live, mutable state the shell works on is
//! [`crate::browser::Browser`], which converts to and from [`SessionShape`].
//!
//! Every struct keeps unknown keys in `extra`, so a file a newer Mac wrote
//! comes back out with everything it had.

use crate::date::UnixTime;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One tab as the session keeps it (`Session.Entry`).
///
/// `pin` is the pinned tab's letter; `saved == None` on a loose tab means
/// "saved" (an upstream-shaped file); `seen` is **Unix** seconds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionEntry {
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// The pinned tab's letter (or emoji); `None` for a loose tab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// The space UUID the tab lives in (for a pin: the space it was made in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// `Some(true)` on the active tab; never written as `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// Tab group UUID (`groups.json`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Sections: `Some(false)` is a Today tab, `Some(true)`/`None` Saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<bool>,
    /// When it was last looked at, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<UnixTime>,
    /// Split view token: both halves of a kept split carry the same UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<String>,
    /// Keys this build does not know, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The whole file (`Session.Shape`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionShape {
    /// Every space's loose tabs, space by space. With `pins` present, the
    /// pinned entries here are only the copy older builds read; they are
    /// skipped on restore.
    #[serde(default)]
    pub tabs: Vec<SessionEntry>,
    /// Index into `tabs` of the active tab (upstream files have no `active`
    /// flag, only this).
    #[serde(default)]
    pub active: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spaces: Option<Vec<Space>>,
    /// The current space UUID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Global pins, shown in every space. `None` in files from before them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pins: Option<Vec<SessionEntry>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SessionShape {
    /// How many tabs this is, counting the older-builds copy of the pins
    /// once (`Session.Shape.tabCount`).
    pub fn tab_count(&self) -> usize {
        match &self.pins {
            Some(pins) => pins.len() + self.tabs.iter().filter(|t| t.pin.is_none()).count(),
            None => self.tabs.len(),
        }
    }
}

/// A space (`Space`): a row of tabs with a name, a colour and optionally its
/// own cookie jar.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Space {
    /// UUID, uppercase.
    pub id: String,
    pub name: String,
    /// 0…1 on the colour wheel; `None` is plain grey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<f64>,
    /// Cookie jar name; `None` shares the default one. Spaces with the same
    /// name sign in together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// An emoji, or an SF Symbol name behind `sf:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<SpaceTheme>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Space {
    /// A new space with a fresh id.
    pub fn new(name: impl Into<String>) -> Self {
        Space {
            id: crate::ids::new_uuid(),
            name: name.into(),
            ..Default::default()
        }
    }

    /// What stands for the space in the column: the icon if it is an emoji,
    /// else the first letter of the name, uppercased.
    pub fn letter(&self) -> String {
        if let Some(icon) = &self.icon
            && !icon.is_empty()
            && !icon.starts_with("sf:")
        {
            return icon.clone();
        }
        self.name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }
}

/// A space's look (`SpaceTheme`). The Mac decoder is lenient (every number
/// has a default), so every number is optional here and absent stays absent.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SpaceTheme {
    /// One colour is flat; two or three are a gradient.
    #[serde(default)]
    pub colors: Vec<ColorStop>,
    /// 0…1, default 0.8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intensity: Option<f64>,
    /// Film grain 0…1, default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain: Option<f64>,
    /// A file name in `themes/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<ThemeMotion>,
    /// 0…1, default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur: Option<f64>,
    /// -1…1, default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One gradient stop, sRGB 0…1.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ColorStop {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ColorStop {
    /// `#rrggbb`.
    pub fn hex(&self) -> String {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02x}{:02x}{:02x}", c(self.r), c(self.g), c(self.b))
    }
}

/// An animated backdrop (not drawn on Linux v1, but kept).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ThemeMotion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
