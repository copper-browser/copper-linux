//! Spaces in the window: colour and tint ([`colour`]), and the header,
//! chips, menu, editor and delete dialog ([`ui`]) (Mac: `Fork/Spaces*.swift`,
//! `Fork/SpaceTheme.swift`, `Fork/SpaceTint.swift`, `docs/spaces.md`).
//!
//! The model (rows, pins, current space, profile) is copper-core's
//! `Browser`; the shell's `add_space`, `switch_space`, `rename_space`,
//! `set_space_colour`, `set_space_icon`, `set_space_profile`, `move_space`
//! and `remove_space` change it.

pub mod colour;
pub mod ui;

use copper_core::session::Space;

/// What a space wears as its mark: its emoji, else the first letter of its
/// name (an `sf:` symbol from a Mac has no Linux glyph, so it is the letter).
pub fn mark(space: &Space) -> String {
    match space.icon.as_deref().map(str::trim) {
        Some(icon) if !icon.is_empty() && !icon.starts_with("sf:") && !is_symbol_name(icon) => {
            icon.chars().take(2).collect()
        }
        _ => space
            .name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_else(|| "•".into()),
    }
}

/// The space's emoji, if it has one (not a letter, not a Mac symbol).
pub fn emoji(space: &Space) -> Option<String> {
    let icon = space.icon.as_deref()?.trim();
    if icon.is_empty() || icon.starts_with("sf:") || is_symbol_name(icon) {
        return None;
    }
    let m: String = icon.chars().take(2).collect();
    m.chars().any(|c| !c.is_alphanumeric()).then_some(m)
}

/// A Mac SF Symbol name stored bare (`briefcase.fill`).
fn is_symbol_name(icon: &str) -> bool {
    icon.len() > 2
        && icon
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == '.' || c.is_ascii_digit())
}

/// A name for a new space nobody has used yet.
pub fn fresh_name<'a>(spaces: impl IntoIterator<Item = &'a Space>) -> String {
    let names: Vec<String> = spaces.into_iter().map(|s| s.name.to_lowercase()).collect();
    if !names.contains(&"space".to_string()) {
        return "Space".into();
    }
    (2..)
        .map(|n| format!("Space {n}"))
        .find(|n| !names.contains(&n.to_lowercase()))
        .unwrap_or_else(|| "Space".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks() {
        let mut s = Space::new("work");
        assert_eq!(mark(&s), "W");
        s.icon = Some("🔥".into());
        assert_eq!(mark(&s), "🔥");
        s.icon = Some("sf:briefcase".into());
        assert_eq!(mark(&s), "W");
        s.icon = Some("briefcase.fill".into());
        assert_eq!(mark(&s), "W");
        let blank = Space::new("  ");
        assert_eq!(mark(&blank), "•");
    }

    #[test]
    fn fresh_names() {
        let a = Space::new("Home");
        assert_eq!(fresh_name([&a]), "Space");
        let b = Space::new("Space");
        let c = Space::new("Space 2");
        assert_eq!(fresh_name([&a, &b, &c]), "Space 3");
    }
}
