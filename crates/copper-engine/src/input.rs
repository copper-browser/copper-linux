//! Input events an agent (or test) can send to a page.
//!
//! Coordinates are CSS pixels relative to the top-left of the page viewport
//! (the same space as `getBoundingClientRect()`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    #[default]
    Left,
    Middle,
    Right,
}

/// Keyboard modifiers held during an input event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
    };
}

/// How an input action reached the page (`PageView::input`'s answer).
///
/// For trails, probes and benches; tools that only need the action done can
/// ignore it. `keydown`/`keyup`/`keypress` are dispatched from script on
/// every path (no engine API makes them trusted), so they never count here:
/// typing is `Trusted` when the text went in through the engine's editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// The engine's own input handling made the events the action is about
    /// (pointer, mouse, `click`, `dblclick`, focus, `beforeinput`/`input`,
    /// `submit`, `change`): `isTrusted == true`, as if a person did it.
    Trusted,
    /// Part of the action went each way (typing text that contains a Tab).
    Mixed,
    /// Every event was dispatched from script (`isTrusted == false`).
    Script,
}

impl Trust {
    /// The trust of an action made of two parts.
    pub fn and(self, other: Trust) -> Trust {
        if self == other { self } else { Trust::Mixed }
    }

    pub fn is_trusted(self) -> bool {
        self == Trust::Trusted
    }
}

/// Which input actions `PageView::input` delivers through the engine's own
/// input handling on one page (`isTrusted == true`), rather than from script.
///
/// Each engine fills this in from what it can do (WebKitGTK: a start-up
/// self-check of its event controllers; WebView2: everything goes through
/// the DevTools input domain). `PageView::set_trusted_input(false)` turns it
/// all off, for tests and benches that compare the two paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustedInput {
    /// Pointer moves and left clicks go through the engine.
    pub pointer: bool,
    /// Typing and editing keys go through the engine's editor.
    pub editing: bool,
    /// The page can be given focus without the window having it (a page an
    /// agent works in backstage still sees `focus` and `:focus`).
    pub page_focus: bool,
}

impl TrustedInput {
    /// Everything from script.
    pub const NONE: TrustedInput = TrustedInput {
        pointer: false,
        editing: false,
        page_focus: false,
    };
}

/// One input action. See `PageView::input` for how faithful each one is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEvent {
    /// `count` = 2 is a double click.
    Click {
        x: f64,
        y: f64,
        button: MouseButton,
        count: u32,
        modifiers: Modifiers,
    },
    /// Move the pointer (hover).
    Move {
        x: f64,
        y: f64,
        modifiers: Modifiers,
    },
    /// Press at `from`, move to `to`, release (left button).
    Drag {
        from_x: f64,
        from_y: f64,
        to_x: f64,
        to_y: f64,
        modifiers: Modifiers,
    },
    /// One key press. `key` is a DOM `KeyboardEvent.key` value (`"a"`,
    /// `"Enter"`, `"Backspace"`, `"ArrowDown"`…); `text` is what it types,
    /// if anything.
    Key {
        key: String,
        text: Option<String>,
        modifiers: Modifiers,
    },
    /// Wheel-scroll by `dx`/`dy` CSS pixels at the last pointer position
    /// (or the viewport centre).
    Scroll { dx: f64, dy: f64 },
    /// Type text into the focused element, one key at a time.
    Type(String),
}

impl InputEvent {
    /// A plain single left click.
    pub fn click(x: f64, y: f64) -> Self {
        InputEvent::Click {
            x,
            y,
            button: MouseButton::Left,
            count: 1,
            modifiers: Modifiers::NONE,
        }
    }

    /// A key press that types nothing (`Enter`, `Tab`, arrows…).
    pub fn key(key: impl Into<String>) -> Self {
        InputEvent::Key {
            key: key.into(),
            text: None,
            modifiers: Modifiers::NONE,
        }
    }
}
