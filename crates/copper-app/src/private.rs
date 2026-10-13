//! Private tabs (Mac: "shy" tabs, `Browser.newShyTab`).
//!
//! A private tab gets its own ephemeral profile (nothing written to disk,
//! no cookies shared with any other tab, private or not), is never written
//! to `session.json`, never enters the closed-tab stack, and records no
//! history. The column shows it on the private tint and the field says
//! so. All of that hangs on `Tab::private` in copper-core; this module only
//! names the profiles and the look.

use crate::spaces::colour::{Theme, Tint};
use copper_core::TabId;
use copper_engine::ProfileId;

/// A private tab's own, ephemeral, profile.
pub fn profile_id(tab: &TabId) -> ProfileId {
    let clean: String = tab
        .0
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(100)
        .collect();
    ProfileId::new(format!("private-{clean}"))
        .unwrap_or_else(|_| ProfileId::new("private").expect("valid id"))
}

/// The column's tint while a private tab is selected.
pub fn tint(night: bool) -> Tint {
    Tint::new(Theme::private(), night)
}

/// The field's placeholder while a private tab is selected.
pub const PLACEHOLDER: &str = "Private — search or enter an address";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_private_tab_has_its_own_profile() {
        let a = TabId::new();
        let b = TabId::new();
        assert_ne!(profile_id(&a), profile_id(&b));
        assert!(profile_id(&a).as_str().starts_with("private-"));
    }

    #[test]
    fn private_column_is_a_cool_violet_in_both_looks() {
        assert!(tint(true).dark);
        let day = tint(false);
        assert!(!day.dark);
        let h = day.hue.expect("a hue");
        assert!((0.66..0.78).contains(&h), "violet: {h}");
        let copper =
            crate::spaces::colour::Tint::new(crate::spaces::colour::Theme::copper(), false);
        let (hp, _, _) = day.ground().to_hsv();
        let (hc, _, _) = copper.ground().to_hsv();
        assert!(
            (hp - hc).abs() > 0.3,
            "visibly not the copper column: {hp} vs {hc}"
        );
    }
}
