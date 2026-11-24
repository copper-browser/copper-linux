//! The tool catalogue and the server's instructions, word for word the
//! Mac's (`Tools.catalogue`, `Ultrafast.catalogue`, `CanvasTools.catalogue`,
//! `CloudHistoryTool.schema`). Names, descriptions and input schemas are
//! read from the catalogue captured from macOS Copper
//! (`fixtures/mac/tools-list.json`), so they cannot drift; this file only
//! decides which tools are offered and in what order.
//!
//! The fixture leaves out what Linux does not take (frames, new-password
//! autofill, Jev's `vision`/`private`, browser_drag's "fails when the page
//! did not change") and says "this computer" for "this Mac". The
//! Listen-transcript tools (`voice_*`) are not offered on Linux.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

/// The macOS catalogue, as captured (sorted by name).
pub const MAC_TOOLS_LIST: &str = include_str!("../../../fixtures/mac/tools-list.json");

/// Tools the Linux build does not offer.
pub const OMITTED: [&str; 2] = ["voice_status", "voice_transcript"];

/// Jev mode's tools, in the Mac's order (only when Jev mode is on).
pub const JEV: [&str; 4] = ["jev_run", "jev_step", "jev_extract", "jev_observe"];

/// The Playwright-shaped tools plus the probe and crash history, in the
/// Mac's order (`Tools.playwright`).
pub const PLAYWRIGHT: [&str; 30] = [
    "browser_tabs",
    "browser_sign_in",
    "browser_autofill",
    "browser_vault_save",
    "browser_vault_note",
    "browser_navigate",
    "browser_navigate_back",
    "browser_navigate_forward",
    "browser_snapshot",
    "browser_click",
    "browser_mouse_click_xy",
    "browser_type",
    "browser_fill_form",
    "browser_press_key",
    "browser_hover",
    "browser_select_option",
    "browser_handle_dialog",
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

/// The browser's own state and the page tools (`StateTools.catalogue`), in
/// the Mac's order, after browser_crashes.
pub const STATE: [&str; 10] = [
    "browser_file_upload",
    "browser_pdf_save",
    "browser_downloads",
    "browser_network_requests",
    "browser_cookies",
    "browser_history",
    "browser_bookmarks",
    "browser_permissions",
    "browser_handoff",
    "browser_spaces",
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

/// What tools/list offers of the canvas tools while no canvas is in use:
/// the two ways to get one (`CanvasTools.doors`). Every canvas tool still
/// answers by name.
pub const CANVAS_DOORS: [&str; 2] = ["canvas_create", "canvas_join"];

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

/// `tools/list`'s array (`Tools.offered`). `jev`: Jev mode on; `loopback`:
/// a client on this machine (not an agent link), which also gets
/// `cloud_history_delete`; `canvas_in_use`: all canvas tools, else only
/// `CANVAS_DOORS` (`CanvasTools.listed`).
pub fn catalogue(jev: bool, loopback: bool, canvas_in_use: bool) -> Vec<Value> {
    let mut names: Vec<&str> = Vec::new();
    if jev {
        names.extend(JEV);
    }
    names.extend(PLAYWRIGHT);
    names.extend(STATE);
    names.extend(
        CANVAS
            .into_iter()
            .filter(|n| canvas_in_use || CANVAS_DOORS.contains(n)),
    );
    if loopback {
        names.push(CLOUD_HISTORY);
    }
    names.into_iter().filter_map(|n| tool(n).cloned()).collect()
}

/// What every tool tells the agent about its own tab (`Tools.tabNote`).
pub const TAB_NOTE: &str = "Work that the user need not watch goes in a tab of your own: browser_tabs new (or jev_run with newTab) opens one in the background without switching the user's tab, and every later call of yours lands there — snapshots, screenshots, clicks and typing all work while the user looks at something else. browser_tabs select binds you to another tab the same way; pass focus: true only when the user should see it. Any page tool takes tab (an id or index from browser_tabs list) to work on another tab without changing which is yours.";

/// What every tool tells the agent about `reason` (`Tools.reasonNote`).
pub const REASON_NOTE: &str = "The user watches every call in a pane beside the page: pass `reason` (one short sentence: what you are doing and why) on each call, and `element` for anything you click or type into. If a call answers \"Stopped by the user\", stop and say so — do not retry.";

/// `CanvasTools.instructions`.
pub const CANVAS_NOTE: &str = "Canvases are the user's whiteboards — each is a copper://canvas/<id> tab; Personal always exists and is private. Use canvas_* tools, never browser_* clicks, to change one. Coordinates are board units: x grows right, y grows down, (0,0) is arbitrary; shapes have x, y (top-left), w, h. Always canvas_read first — on a big canvas read overview:true first, then a frame or region, or follow next a page at a time: reuse its ids, place new shapes beside existing ones (or omit x/y to auto-place near the view), and change a shape with update/move/resize instead of re-adding it. canvas_apply runs every op in one transaction and returns {applied, ids, errors}; colors are yellow, pink, blue, green, purple, gray, white or #hex; connect draws an arrow between two shapes; clear needs confirm:true and only on an explicit request. For a big batch (dozens of shapes) send several canvas_apply calls of at most 40 ops each rather than one huge call, and give added shapes your own ids (shape.id) so connect ops can name them.";

/// The canvas sentence while no canvas is in use (`CanvasTools.briefing`).
pub const CANVAS_BRIEFING: &str = "No canvas in use yet: canvas_create makes one and canvas_join opens an invite; the other canvas_* tools (read, apply, …) are listed once one exists.";

/// canvas_apply's op shapes (`CanvasTools.opsGuide`), handed over with a
/// caller's first canvas_read (and to canvas_apply with no ops) rather than
/// paid in every tools/list.
pub const OPS_GUIDE: &str = "canvas_apply ops: {op:'add', shape:{type:'sticky'|'text'|'frame'|'arrow'|'image'|'link'|'checklist'|'timeline', id?, x?, y?, w?, h?, color?, text?|title?|url?|from?/to?|label?}} (omit x/y to place it in free space near the view; give it your own id, e.g. 'n1', to connect it later in the same call). A link with live:true (or type:'web') shows the site itself on the board, an iframe everyone on the board can browse — each person signed in as themselves; it defaults to 960×640 and needs an http(s) url; update {live:false} turns it back into a card. A checklist is a clickable RSVP / to-do card: {type:'checklist', title, columns?:['Yes','No'] (1–4; one column = to-do boxes), rows:['Ann', …] (≤60), picks?:{'Ann':'Yes'}, notes?:{'Ann':'Back at 6'}, assignees?:{'Fix login':['Grace Hopper','ann@example.com']}} — it is stored as a sticky with view:'checklist' whose text is the list ('### Title' then '- [ ] Ann' / '- [x] Ann · No' lines, a row's comment as indented '  > …' lines under it, then one indented '  @Name' line per assignee), so older Coppers show it as a note with task boxes; each row has at most one pick and one comment (multi-line text, shown collapsed under the row) and up to 12 assignees (people on the canvas named by name, email or id — each newly assigned one is notified like a chat @mention; anyone else by a name of their own); in update, picks, notes and assignees name a row by label, null clears it (assignees replace the row's list), and rows/columns/title replace them in the text (rows keep their picks, comments and assignees by label). {op:'update', id, patch:{view:'checklist', columns?}} turns a sticky that holds a heading and a task list into a card, ticks kept; view:null turns it back into a note. A timeline: {type:'timeline', title?, start:'YYYY-MM-DD', end, step?:'day'|'week'|'month'|'quarter'|'year', items?:[{date, end?, label, text?, color?}]} (text makes a bubble, end a section), stored as a sticky with view:'timeline' whose text is the timeline; update takes the same (items replaces the list) plus addItems:[…] and editItems:{ref: patch|null} (ref: 'YYYY-MM-DD · label', a label, a date or an index; null removes). {op:'update', id, patch:{…}}; {op:'move', id, dx, dy}; {op:'resize', id, w, h}; {op:'delete', id}; {op:'connect', from:id, to:id, label?} (an arrow); comment threads on a shape, Figma-style: {op:'comment', on:shapeId, text} starts one (its ids[i] is the thread id), {op:'reply', thread, text} adds onto one (a resolved thread opens again), {op:'resolve', thread, resolved:true|false} resolves or reopens; {op:'clear', confirm:true} only when the user explicitly asked to wipe the board.";

/// `initialize`'s `instructions` (`Tools.instructions`), without the Listen
/// sentence (no voice tools here) and the new-password clause (no
/// new-password autofill here). `canvas_in_use` picks the canvas sentence.
pub fn instructions(jev: bool, canvas_in_use: bool) -> String {
    let lead = if jev {
        "Copper is the user's own browser, signed in as them; the current tab is the one they are looking at — work there unless the task names another tab. ANY task: the FIRST call is jev_run with the whole goal, every concrete place/date/name/filter and stop condition; no browser_tabs or snapshot first. Add url only for a named page that is not current. jev_observe = fast indexed read; jev_extract = JSON values; jev_step = one decision. BLOCKED → browser_snapshot/click/type for that part, then jev_run again. DONE is Jev's claim — verify with jev_observe/jev_extract. Screenshot when layout matters; nothing is sandboxed — act as the user. For sign-in forms call browser_sign_in first; it fills a saved account in-process and never returns the password. For checkout, address, or sign-up forms call browser_autofill (kind card|identity) — it fills from the vault and never returns the values. To keep a login or backup codes you were given, call browser_vault_save or browser_vault_note."
    } else {
        "Copper is the user's own browser, signed in as them; the current tab is the one they are looking at — work there unless the task names another tab. First action: call browser_snapshot with interactive: true on the current tab, then act with its refs. Call browser_tabs only when another tab is named. Take a screenshot when layout matters. Nothing is sandboxed — act as the user would. For sign-in forms call browser_sign_in first; it fills a saved account in-process and never returns the password. For checkout, address, or sign-up forms call browser_autofill (kind card|identity) — it fills from the vault and never returns the values. To keep a login or backup codes you were given, call browser_vault_save or browser_vault_note."
    };
    let canvas = if canvas_in_use {
        CANVAS_NOTE
    } else {
        CANVAS_BRIEFING
    };
    format!("{lead} {TAB_NOTE} {REASON_NOTE} {canvas}")
}

/// The protocol versions `initialize` agrees to; anything else gets 2025-06-18.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mac's autofill clause Linux leaves out: no new-password kind here.
    const NEW_PASSWORD: &str = "; for a new or changed password call it with kind new-password";

    #[test]
    fn every_offered_name_is_in_the_mac_catalogue() {
        let all = catalogue(true, true, true);
        assert_eq!(all.len(), mac_tools().len() - OMITTED.len());
        for name in mac_tools().keys() {
            if !OMITTED.contains(&name.as_str()) {
                assert!(
                    all.iter().any(|t| t["name"] == name.as_str()),
                    "{name} not offered"
                );
            }
        }
        assert_eq!(
            catalogue(false, false, true).len(),
            all.len() - JEV.len() - 1
        );
    }

    #[test]
    fn instructions_match_the_mac_minus_voice() {
        let init: Value =
            serde_json::from_str(include_str!("../../../fixtures/mac/mcp-initialize.json"))
                .unwrap();
        let mac = init["result"]["instructions"]
            .as_str()
            .unwrap()
            .replacen(NEW_PASSWORD, "", 1);
        let ours = instructions(true, true);
        assert!(
            mac.starts_with(&ours),
            "instructions drifted from the Mac's"
        );
        assert!(mac[ours.len()..].starts_with(" Listen transcripts"));
        let bare = instructions(true, false);
        assert!(bare.ends_with(CANVAS_BRIEFING) && !bare.contains(CANVAS_NOTE));
        assert!(instructions(false, true).starts_with("Copper is") && !ours.contains(NEW_PASSWORD));
    }

    /// The Mac's `toolsListStaysUnderItsBudget`: what a loopback agent pays
    /// at the start of every session.
    #[test]
    fn tools_list_stays_under_its_budget() {
        let bare = catalogue(true, true, false);
        let all = catalogue(true, true, true);
        let bare_bytes = serde_json::to_vec(&bare).unwrap().len();
        let bytes = serde_json::to_vec(&all).unwrap().len();
        println!("tools/list bytes (jev, loopback): no canvas {bare_bytes}, canvas {bytes}");
        assert!(
            bare_bytes < 25_000,
            "tools/list with no canvas is {bare_bytes} bytes"
        );
        assert!(bytes <= 32_000, "tools/list is {bytes} bytes");
        assert!(!all.iter().any(|t| {
            t["inputSchema"]["required"]
                .as_array()
                .is_some_and(Vec::is_empty)
        }));
        for name in ["browser_mouse_click_xy", "browser_handoff", "canvas_apply"] {
            assert!(all.iter().any(|t| t["name"] == name), "{name}");
        }
        let canvas: Vec<&str> = bare
            .iter()
            .filter_map(|t| t["name"].as_str())
            .filter(|n| n.starts_with("canvas_"))
            .collect();
        assert_eq!(canvas, CANVAS_DOORS);
        assert!(OPS_GUIDE.contains("op:'connect'"));
    }

    /// `reason` and `tab` are bare strings, explained once in the
    /// instructions; which tools take them is the Mac's.
    #[test]
    fn reason_and_tab_as_the_mac_has_them() {
        let bare = serde_json::json!({"type": "string"});
        let no_reason = ["browser_perf_probe"];
        let no_tab = [
            "browser_resize",
            "browser_history",
            "browser_crashes",
            "browser_perf_probe",
            "cloud_history_delete",
        ];
        for t in catalogue(true, true, true) {
            let name = t["name"].as_str().unwrap();
            let props = &t["inputSchema"]["properties"];
            if name.starts_with("jev_") || no_reason.contains(&name) {
                assert!(props.get("reason").is_none(), "{name} reason");
            } else {
                assert_eq!(props["reason"], bare, "{name} reason");
            }
            if name.starts_with("canvas_") || no_tab.contains(&name) {
                assert!(props.get("tab").is_none(), "{name} tab");
            } else if name == "browser_tabs" {
                assert_eq!(props["tab"]["description"], "Tab id for close/select");
            } else {
                assert_eq!(props["tab"], bare, "{name} tab");
            }
        }
    }
}
