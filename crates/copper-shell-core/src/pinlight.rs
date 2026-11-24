//! A pin with something new on it wears a small copper dot (Mac:
//! `Fork/PinLight.swift`).
//!
//! A page says it has news the way Gmail, Slack and X tell a tab bar: its
//! title starts with an unread count, "(3) Inbox". The dot sits on the
//! pin's square in the column and the top strip, and hides while the pin is
//! the tab you are on ([`crate::TabView::lit`]). A sleeping pin keeps its
//! last title, so its dot stays.

/// "(1) Paper", "(12) Slack", Gmail's "Inbox (3) - …": yes. "(0) x",
/// "Paper", "Year (2026)": no (`PinLight.unread`).
pub fn lit(title: &str) -> bool {
    let rest = title.strip_prefix("Inbox ").unwrap_or(title);
    let Some((count, after)) = rest.strip_prefix('(').and_then(|r| r.split_once(')')) else {
        return false;
    };
    count.starts_with(|c: char| c.is_ascii_digit())
        && count.chars().all(|c| c.is_ascii_digit() || c == ',')
        && count.chars().any(|c| c != '0' && c != ',')
        && after.chars().next().is_none_or(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::lit;

    // PinLightTests.swift: a title that starts with an unread count above zero.

    #[test]
    fn leading_count_above_zero_lights() {
        for title in [
            "(1) Paper",
            "(3) Inbox",
            "(12) Slack | general",
            "(1,204) Home / X",
            "Inbox (3) - me@x.com - Gmail",
            "(2)",
        ] {
            assert!(lit(title), "{title:?}");
        }
    }

    #[test]
    fn anything_else_stays_dark() {
        for title in [
            "(0) x",
            "Paper",
            "Year (2026)",
            "",
            "(a) b",
            "() Paper",
            "(1)Paper",
            " (1) Paper",
            "Inbox - me@x.com - Gmail",
        ] {
            assert!(!lit(title), "{title:?}");
        }
    }
}
