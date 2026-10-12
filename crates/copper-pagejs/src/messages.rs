//! Script message handlers the page scripts post to, and what they send.
//! Register each with `UserContentManager::register_script_message_handler(name, world)`;
//! the page posts with `window.webkit.messageHandlers.<name>.postMessage(…)`
//! exactly as under WKWebView.

use serde::{Deserialize, Serialize};

use crate::script::FRAME_WORLD;

/// Scroll position and swipe arbitration (`ScrollRelay`, `Swipe.watch`).
pub const SCROLL: &str = "officeScroll";
/// The hide-anything picker (`VeilRelay`).
pub const VEIL: &str = "officeVeil";
/// Image right-click (`ImageRelay`).
pub const IMAGES: &str = "officeImages";
/// Sign-in, focus and fullscreen (`FormRelay`; also the lean canvas's typing relay).
pub const FORMS: &str = "officeForms";
/// The canvas page bridge (`CanvasHost.handler`); accepts objects or JSON strings,
/// only from the bundled main frame.
pub const CANVAS: &str = "canvas";
/// Canvas live-frame reports (`CanvasHost.frameHandler`), in [`FRAME_WORLD`].
pub const FRAME: &str = "copperFrame";

/// Every handler Linux v1 registers, with its world (`None` = page world).
pub const HANDLERS: [(&str, Option<&str>); 6] = [
    (SCROLL, None),
    (VEIL, None),
    (IMAGES, None),
    (FORMS, None),
    (CANVAS, None),
    (FRAME, Some(FRAME_WORLD)),
];

/// `officeScroll`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScrollMessage {
    Position {
        y: f64,
        max: f64,
    },
    /// "taken" (the page scrolls sideways) or "free" (a swipe may navigate).
    Side {
        side: String,
    },
}

/// `officeVeil`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum VeilMessage {
    Trouble {
        trouble: String,
    },
    Off {
        off: bool,
    },
    Picked {
        selector: String,
        #[serde(default)]
        label: Option<String>,
        #[serde(default)]
        note: Option<String>,
    },
}

/// `officeImages`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageMessage {
    pub src: String,
}

/// A field's rect in viewport CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// What the focused box is (`classify`): kind, group, label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldInfo {
    pub kind: String,
    pub group: String,
    pub label: String,
}

/// `officeForms`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FormsMessage {
    /// A sign-in is on the page.
    Form,
    /// A sign-in was sent with these values.
    Submit {
        #[serde(default)]
        user: String,
        #[serde(default)]
        password: String,
    },
    /// The sign-in boxes went away without a new page.
    Settled,
    /// Where the caret is.
    Focus {
        #[serde(default)]
        typing: bool,
        #[serde(default)]
        rect: Option<FieldRect>,
        #[serde(default)]
        field: Option<FieldInfo>,
        #[serde(default)]
        hint: String,
    },
    /// Going (or gone) full screen.
    Fullscreen {
        #[serde(default)]
        on: bool,
    },
}

/// `copperFrame`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameMessage {
    /// "nav", "escape" or "blank".
    pub kind: String,
    /// The frame's `copper-frame:` window name.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}
