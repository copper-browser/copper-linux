//! Hide anything (Curtain.swift): the picker user script (`window.__officeVeil`,
//! posts to `officeVeil`), the persisted stylesheet, and peeking.

use serde_json::{Value, json};

use crate::call::{Call, UserScript};
use crate::script::Script;

/// The picker: document start, main frame.
pub fn picker() -> UserScript {
    UserScript::new(Script::CurtainPicker, json!({}))
}

/// `Curtain.css`: one rule per selector, so one the browser can't parse
/// doesn't take the rest down; `spared` is left out (a peek).
pub fn css<'a>(selectors: impl IntoIterator<Item = &'a str>, spared: Option<&str>) -> String {
    selectors
        .into_iter()
        .filter(|s| Some(*s) != spared)
        .map(|s| format!("{s} {{ display: none !important; }}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `Curtain.host`: lowercased, without a leading `www.`.
pub fn host_key(host: &str) -> String {
    let host = host.to_lowercase();
    host.strip_prefix("www.")
        .map(str::to_string)
        .unwrap_or(host)
}

/// The stylesheet in before the first paint: document start, main frame.
pub fn style(css: &str) -> UserScript {
    UserScript::new(Script::CurtainStyle, json!({ "css": css }))
}

/// The stylesheet replaced in a page already showing (after a hide or restore).
pub fn restyle(css: &str) -> Call<Value> {
    Call::new(Script::CurtainStyle, json!({ "css": css }))
}

pub fn start() -> Call<Value> {
    Call::new(Script::CurtainStart, json!({}))
}

pub fn stop() -> Call<Value> {
    Call::new(Script::CurtainStop, json!({}))
}

/// Show `selector` while its row is hovered: `css` is the sheet without it.
pub fn peek(css: &str, selector: &str) -> Call<Value> {
    Call::new(
        Script::CurtainPeek,
        json!({ "css": css, "selector": selector }),
    )
}

pub fn unpeek(css: &str) -> Call<Value> {
    Call::new(Script::CurtainUnpeek, json!({ "css": css }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_like_the_mac() {
        assert_eq!(
            css(["#a", ".b c"], None),
            "#a { display: none !important; }\n.b c { display: none !important; }"
        );
        assert_eq!(
            css(["#a", ".b"], Some("#a")),
            ".b { display: none !important; }"
        );
        assert_eq!(host_key("WWW.Example.test"), "example.test");
    }
}
