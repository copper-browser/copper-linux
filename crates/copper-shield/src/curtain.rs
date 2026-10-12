//! `hidden.json` → the stylesheet for one site (`Curtain.css(on:without:)`).
//!
//! The rules themselves (one per selector, `display: none !important`) and
//! the host key (lowercased, no `www.`) come from `copper_pagejs::curtain`,
//! which is the byte-for-byte port of the Mac's; this is only the lookup in
//! the file. The result goes in as a user style sheet at document start, so
//! nothing is ever seen appearing and vanishing.

use copper_core::crashes::HiddenFile;
use copper_pagejs::curtain::{css, host_key};

/// The site key `hidden.json` uses for a URL's host.
pub fn key_for_host(host: &str) -> String {
    host_key(host)
}

/// The stylesheet for `host` (any case, with or without `www.`); empty when
/// nothing is hidden there. `spared` leaves one selector out (a peek).
pub fn stylesheet(hidden: &HiddenFile, host: Option<&str>, spared: Option<&str>) -> String {
    let Some(host) = host else {
        return String::new();
    };
    let Some(veils) = hidden.get(&host_key(host)) else {
        return String::new();
    };
    css(veils.iter().map(|v| v.selector.as_str()), spared)
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::crashes::Veil;
    use copper_core::date::SwiftDate;

    fn veil(selector: &str) -> Veil {
        Veil {
            selector: selector.into(),
            label: selector.into(),
            note: None,
            date: SwiftDate(0.0),
            extra: Default::default(),
        }
    }

    #[test]
    fn one_rule_per_selector() {
        let mut file = HiddenFile::new();
        file.insert(
            "example.com".into(),
            vec![veil("#cookie-bar"), veil("aside.related")],
        );
        assert_eq!(
            stylesheet(&file, Some("WWW.Example.com"), None),
            "#cookie-bar { display: none !important; }\naside.related { display: none !important; }"
        );
        assert_eq!(
            stylesheet(&file, Some("example.com"), Some("#cookie-bar")),
            "aside.related { display: none !important; }"
        );
        assert_eq!(stylesheet(&file, Some("other.org"), None), "");
        assert_eq!(stylesheet(&file, None, None), "");
    }
}
