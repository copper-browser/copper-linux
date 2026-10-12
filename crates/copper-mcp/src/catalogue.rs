//! The tool catalogue and the server's instructions, word for word the
//! Mac's (`Tools.catalogue`, `Ultrafast.catalogue`, `CanvasTools.catalogue`,
//! `CloudHistoryTool.schema`). Names, descriptions and input schemas are
//! read from the catalogue captured from macOS Copper
//! (`fixtures/mac/tools-list.json`), so they cannot drift; this file only
//! decides which tools are offered and in what order.
//!
//! The Listen-transcript tools (`voice_*`) are not offered on Linux.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

/// The macOS catalogue, as captured (sorted by name).
pub const MAC_TOOLS_LIST: &str = include_str!("../../../fixtures/mac/tools-list.json");

/// Tools the Linux build does not offer.
pub const OMITTED: [&str; 2] = ["voice_status", "voice_transcript"];

/// Jev mode's tools, in the Mac's order (only when Jev mode is on).
pub const JEV: [&str; 4] = ["jev_run", "jev_step", "jev_extract", "jev_observe"];

/// The Playwright-shaped tools plus the probe and crash history, in order.
pub const PLAYWRIGHT: [&str; 26] = [
    "browser_tabs",
    "browser_sign_in",
    "browser_autofill",
    "browser_navigate",
    "browser_navigate_back",
    "browser_navigate_forward",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_fill_form",
    "browser_press_key",
    "browser_hover",
    "browser_select_option",
    "browser_drag",
    "browser_take_screenshot",
    "browser_evaluate",
    "browser_wait_for",
    "browser_scroll",
    "browser_get_text",
    "browser_console_messages",
    "browser_find",
    "browser_resize",
    "browser_close",
    "browser_groups",
    "browser_perf_probe",
    "browser_crashes",
];

/// The canvas tools, in order.
pub const CANVAS: [&str; 12] = [
    "canvas_list",
    "canvas_open",
    "canvas_read",
    "canvas_apply",
    "canvas_select",
    "canvas_focus",
    "canvas_create",
    "canvas_invite",
    "canvas_share_link",
    "canvas_join",
    "canvas_screenshot",
    "canvas_chat",
];

/// Offered to loopback clients only, never through an agent link.
pub const CLOUD_HISTORY: &str = "cloud_history_delete";

fn mac_tools() -> &'static HashMap<String, Value> {
    static TOOLS: OnceLock<HashMap<String, Value>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let parsed: Value =
            serde_json::from_str(MAC_TOOLS_LIST).expect("fixtures/mac/tools-list.json is JSON");
        parsed["result"]["tools"]
            .as_array()
            .expect("tools-list.json has result.tools")
            .iter()
            .map(|t| {
                (
                    t["name"].as_str().unwrap_or_default().to_string(),
                    t.clone(),
                )
            })
            .collect()
    })
}

/// One tool's definition (`name`, `description`, `inputSchema`).
pub fn tool(name: &str) -> Option<&'static Value> {
    mac_tools().get(name)
}

/// Whether `name` is a canvas tool.
pub fn is_canvas(name: &str) -> bool {
    CANVAS.contains(&name)
}

/// `tools/list`'s array. `jev`: Jev mode on; `loopback`: a client on this
/// machine (not an agent link), which also gets `cloud_history_delete`.
pub fn catalogue(jev: bool, loopback: bool) -> Vec<Value> {
    let mut names: Vec<&str> = Vec::new();
    if jev {
        names.extend(JEV);
    }
    names.extend(PLAYWRIGHT);
    names.extend(CANVAS);
    if loopback {
        names.push(CLOUD_HISTORY);
    }
    names.into_iter().filter_map(|n| tool(n).cloned()).collect()
}

/// What every tool tells the agent about its own tab (`Tools.tabNote`).
pub const TAB_NOTE: &str = "Work that the user need not watch goes in a tab of your own: browser_tabs new (or jev_run with newTab) opens one in the background without switching the user's tab, and every later call of yours lands there — snapshots, screenshots, clicks and typing all work while the user looks at something else. browser_tabs select binds you to another tab the same way; pass focus: true only when the user should see it.";

/// What every tool tells the agent about `reason` (`Tools.reasonNote`).
pub const REASON_NOTE: &str = "The user watches every call in a pane beside the page: pass `reason` (one short sentence: what you are doing and why) on each call, and `element` for anything you click or type into. If a call answers \"Stopped by the user\", stop and say so — do not retry.";

/// `CanvasTools.instructions`.
pub const CANVAS_NOTE: &str = "Canvases are the user's whiteboards — each is a copper://canvas/<id> tab; Personal always exists and is private. Use canvas_* tools, never browser_* clicks, to change one. Coordinates are board units: x grows right, y grows down, (0,0) is arbitrary; shapes have x, y (top-left), w, h. Always canvas_read first: reuse its ids, place new shapes beside existing ones (or omit x/y to auto-place near the view), and change a shape with update/move/resize instead of re-adding it. canvas_apply runs every op in one transaction and returns {applied, ids, errors}; colors are yellow, pink, blue, green, purple, gray, white or #hex; connect draws an arrow between two shapes; clear needs confirm:true and only on an explicit request. For a big batch (dozens of shapes) send several canvas_apply calls of at most 40 ops each rather than one huge call, and give added shapes your own ids (shape.id) so connect ops can name them.";

/// `initialize`'s `instructions` (`Tools.instructions`, without the Listen
/// sentence: no voice tools here).
pub fn instructions(jev: bool) -> String {
    let lead = if jev {
        "Copper is the user's own browser, signed in as them; the current tab is the one they are looking at — work there unless the task names another tab. ANY task: the FIRST call is jev_run with the whole goal, every concrete place/date/name/filter and stop condition; no browser_tabs or snapshot first. Add url only for a named page that is not current. jev_observe = fast indexed read; jev_extract = JSON values; jev_step = one decision. BLOCKED → browser_snapshot/click/type for that part, then jev_run again. DONE is Jev's claim — verify with jev_observe/jev_extract. Screenshot when layout matters; nothing is sandboxed — act as the user. For sign-in forms call browser_sign_in first; it fills a saved account in-process and never returns the password. For checkout, address, or sign-up forms call browser_autofill (kind card|identity) — it fills from the vault and never returns the values."
    } else {
        "Copper is the user's own browser, signed in as them; the current tab is the one they are looking at — work there unless the task names another tab. First action: call browser_snapshot with interactive: true on the current tab, then act with its refs. Call browser_tabs only when another tab is named. Take a screenshot when layout matters. Nothing is sandboxed — act as the user would. For sign-in forms call browser_sign_in first; it fills a saved account in-process and never returns the password. For checkout, address, or sign-up forms call browser_autofill (kind card|identity) — it fills from the vault and never returns the values."
    };
    format!("{lead} {TAB_NOTE} {REASON_NOTE} {CANVAS_NOTE}")
}

/// The protocol versions `initialize` agrees to; anything else gets 2025-06-18.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_name_is_in_the_mac_catalogue() {
        let all = catalogue(true, true);
        assert_eq!(all.len(), mac_tools().len() - OMITTED.len());
        for name in mac_tools().keys() {
            if !OMITTED.contains(&name.as_str()) {
                assert!(
                    all.iter().any(|t| t["name"] == name.as_str()),
                    "{name} not offered"
                );
            }
        }
        assert_eq!(catalogue(false, false).len(), all.len() - JEV.len() - 1);
    }

    #[test]
    fn instructions_match_the_mac_minus_voice() {
        let init: Value =
            serde_json::from_str(include_str!("../../../fixtures/mac/mcp-initialize.json"))
                .unwrap();
        let mac = init["result"]["instructions"].as_str().unwrap();
        let ours = instructions(true);
        assert!(
            mac.starts_with(&ours),
            "instructions drifted from the Mac's"
        );
        assert!(mac[ours.len()..].starts_with(" Listen transcripts"));
    }
}
