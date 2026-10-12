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
