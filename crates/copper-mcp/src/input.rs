//! Playwright's key and modifier names to [`InputEvent`]s
//! (Fork/MCP/Input.swift `Input.modifiers` / `Input.key`).

use copper_engine::{InputEvent, Modifiers};

/// `["Shift", "ControlOrMeta"]` → modifiers. On Linux `ControlOrMeta` is
/// Control, as in Playwright on Linux (the Mac maps it to ⌘).
pub fn modifiers(names: &[String]) -> Modifiers {
    let mut m = Modifiers::NONE;
    for name in names {
        match name.to_lowercase().as_str() {
            "alt" | "option" => m.alt = true,
            "control" | "ctrl" | "controlormeta" => m.ctrl = true,
            "meta" | "cmd" | "command" | "super" => m.meta = true,
            "shift" => m.shift = true,
            _ => {}
        }
    }
    m
}

fn union(a: Modifiers, b: Modifiers) -> Modifiers {
    Modifiers {
        shift: a.shift || b.shift,
        ctrl: a.ctrl || b.ctrl,
        alt: a.alt || b.alt,
        meta: a.meta || b.meta,
    }
}

/// Playwright's special key names (lowercased) to DOM `KeyboardEvent.key`
/// values and the text they type.
fn special(lower: &str) -> Option<(&'static str, Option<&'static str>)> {
    Some(match lower {
        "enter" | "return" => ("Enter", Some("\r")),
        "tab" => ("Tab", Some("\t")),
        "space" => (" ", Some(" ")),
        "backspace" => ("Backspace", None),
        "delete" => ("Delete", None),
        "escape" | "esc" => ("Escape", None),
        "arrowleft" => ("ArrowLeft", None),
        "arrowright" => ("ArrowRight", None),
        "arrowdown" => ("ArrowDown", None),
        "arrowup" => ("ArrowUp", None),
        "home" => ("Home", None),
        "end" => ("End", None),
        "pageup" => ("PageUp", None),
        "pagedown" => ("PageDown", None),
        "f1" => ("F1", None),
        "f2" => ("F2", None),
        "f3" => ("F3", None),
        "f4" => ("F4", None),
        "f5" => ("F5", None),
        "f6" => ("F6", None),
        "f7" => ("F7", None),
        "f8" => ("F8", None),
        "f9" => ("F9", None),
        "f10" => ("F10", None),
        "f11" => ("F11", None),
        "f12" => ("F12", None),
        _ => return None,
    })
}

/// One key, named the way Playwright names them — `Enter`, `ArrowDown`,
/// `a`, `Shift+Tab`, `Control+a`. An unknown multi-character name is typed
/// as text, as on the Mac.
pub fn key(named: &str, held: Modifiers) -> InputEvent {
    let mut flags = held;
    let mut name = named.to_string();
    if name.contains('+') && name.chars().count() > 1 {
        let mut parts: Vec<String> = name.split('+').map(str::to_string).collect();
        name = parts.pop().unwrap_or_default();
        flags = union(flags, modifiers(&parts));
        if name.is_empty() {
            // "Control++": the key is the plus itself.
            name = "+".into();
        }
    }
    let lower = name.to_lowercase();
    if let Some((key, text)) = special(&lower) {
        return InputEvent::Key {
            key: key.to_string(),
            text: text.map(str::to_string),
            modifiers: flags,
        };
    }
    let mut chars = name.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return InputEvent::Type(name);
    };
    if ch.is_uppercase() {
        flags.shift = true;
    }
    let text = if flags.shift {
        ch.to_uppercase().collect::<String>()
    } else {
        ch.to_string()
    };
    InputEvent::Key {
        key: text.clone(),
        text: Some(text),
        modifiers: flags,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(
            key("Enter", Modifiers::NONE),
            InputEvent::Key {
                key: "Enter".into(),
                text: Some("\r".into()),
                modifiers: Modifiers::NONE
            }
        );
        let InputEvent::Key {
            key: k,
            modifiers: m,
            ..
        } = key("Control+Shift+K", Modifiers::NONE)
        else {
            panic!()
        };
        assert_eq!(k, "K");
        assert!(m.ctrl && m.shift);
        assert_eq!(
            key("hello", Modifiers::NONE),
            InputEvent::Type("hello".into())
        );
        let InputEvent::Key {
            key: k,
            modifiers: m,
            ..
        } = key("A", Modifiers::NONE)
        else {
            panic!()
        };
        assert_eq!(k, "A");
        assert!(m.shift);
        assert!(modifiers(&["ControlOrMeta".into()]).ctrl);
    }
}
