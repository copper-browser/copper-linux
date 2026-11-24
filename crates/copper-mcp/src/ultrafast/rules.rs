//! Jev's claims, waits and notes (the pure statics of Ultrafast.swift): when
//! a DONE or BLOCKED is doubted, how long a WAIT lasts, what Jev is told
//! when a move is refused, and how a question's answer is found on the page.

use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine as _;
use copper_pagejs::jev::{JevAction, Observation};
use regex::Regex;
use serde_json::{Value, json};

use super::Step;
use super::strategies::{fold, literals};
use crate::intelligence::{Keys, Lane, Tier, family};

/// Asked beside every decision and read only when it is DONE: how likely is
/// it that something is visibly left undone (Jev's DONE alone was wrong one
/// run in five).
pub const UNMET_STATEMENT: &str = "The visible page text, the field values or the recent actions show that at least one thing the goal asks for was NOT applied or is still to do: for example a search that was typed but never submitted, a filter or option the page reports as off, any or none, or results that are not showing yet.";
/// At or above this, a DONE is not taken at its word (two of 96 finished
/// pages answered 0.51 and 0.53, so `verify` is not in Fast mode yet).
pub const UNMET_FLOOR: f64 = 0.5;
/// How long a click that shows no change is given to show one (`watch`).
pub const CLICK_PATIENCE: Duration = Duration::from_secs(1);
/// Under this, a DONE gets a second look first (r4: every DONE at 80% or
/// more passed; Amazon's false DONE was 67%).
pub const CLAIM_FLOOR: f64 = 0.8;
/// A BLOCKED needs as much (far-link said BLOCKED at 76% on the very page it
/// was sent to).
pub const BLOCKED_FLOOR: f64 = 0.8;

pub fn floor(claim: &str) -> f64 {
    if claim == "BLOCKED" {
        BLOCKED_FLOOR
    } else {
        CLAIM_FLOOR
    }
}

/// At or under this, Jev sees nothing left undone: a BLOCKED then is a DONE
/// it didn't dare say.
pub const NOTHING_UNMET: f64 = 0.2;
/// WAIT: what Jev reads changes, or the last action's data calls are
/// answered and `WAIT_LINGER` quiet — at most this long.
pub const WAIT_CAP: Duration = Duration::from_millis(1200);
/// Quiet this long after the last answer, and a WAIT lasts at least
/// `WAIT_FLOOR` (on Flights every WAIT of 175 ms or less was followed by
/// Reload, every one of 238 ms or more by DONE).
pub const WAIT_LINGER: Duration = Duration::from_millis(400);
pub const WAIT_FLOOR: Duration = Duration::from_millis(300);
/// After a search went in: how long its page is given to ask for its results.
pub const START_WAIT: Duration = Duration::from_millis(300);
/// Scrolls in a row that showed nothing new the goal names before scrolling
/// is not offered (`reach`; Session.barrenLimit).
pub const BARREN_LIMIT: usize = 4;

/// Airport codes (ZRH) the goal names and the run typed that the page shows
/// nowhere now: not in its text, its controls or its address (a search's
/// address often carries them, base64'd). A DONE with one missing picked
/// another airport, or none.
pub fn unshown_typed(history: &[Step], goal: &str, obs: &Observation) -> Vec<String> {
    let typed: BTreeSet<&str> = history
        .iter()
        .filter(|s| s.kind == "fill")
        .filter_map(|s| s.text.as_deref().map(str::trim))
        .collect();
    let codes: Vec<String> = literals(goal)
        .into_iter()
        .filter(|c| {
            c.chars().count() == 3
                && c.chars().all(|ch| ch.is_ascii_uppercase())
                && typed.contains(c.as_str())
        })
        .collect();
    if codes.is_empty() {
        return Vec::new();
    }
    let decoded = percent_encoding::percent_decode_str(&obs.url)
        .decode_utf8()
        .map(|s| s.into_owned())
        .unwrap_or_default();
    let mut seen = format!("{} {} {decoded}", obs.text, obs.url);
    for a in &obs.actions {
        seen += &format!(" {} {}", a.label, a.value.as_deref().unwrap_or(""));
    }
    // Binary once decoded: the code sits between bytes, not words.
    let lenient = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    let mut packed = String::new();
    let query = obs.url.split_once('?').map_or("", |(_, q)| q);
    let query = query.split('#').next().unwrap_or("");
    for item in query.split('&') {
        let Some((_, value)) = item.split_once('=') else {
            continue;
        };
        let Ok(value) = percent_encoding::percent_decode_str(value).decode_utf8() else {
            continue;
        };
        if value.chars().count() < 8 {
            continue;
        }
        let mut b64 = value.replace('-', "+").replace('_', "/");
        b64 += &"=".repeat((4 - b64.len() % 4) % 4);
        if let Ok(data) = lenient.decode(&b64) {
            packed += &String::from_utf8_lossy(&data);
        }
    }
    codes
        .into_iter()
        .filter(|code| {
            let word = Regex::new(&format!(r"\b{}\b", regex::escape(code))).expect("regex");
            !word.is_match(&seen) && !packed.contains(code.as_str())
        })
        .collect()
}

pub fn missing_note(unshown: &[String]) -> Value {
    json!({
        "action": format!("DONE was refused: the page shows no {}, which the goal names. Make sure that exact value is chosen.", unshown.join(" or ")),
        "kind": "done", "text": null, "page_changed": false,
    })
}

/// What a DONE left in doubt says, for the note of an `unsure` ending.
pub fn unmet_summary(unshown: &[String], unmet: Option<f64>, sure: bool) -> String {
    let mut said: Vec<String> = Vec::new();
    if !unshown.is_empty() {
        said.push(format!("the page shows no {}", unshown.join(", ")));
    }
    if let Some(unmet) = unmet.filter(|u| *u >= UNMET_FLOOR) {
        said.push(format!(
            "something the goal asks for looks not done ({:.0}%)",
            unmet * 100.0
        ));
    }
    if !sure {
        said.push(format!(
            "Jev stayed under {}% after a second look",
            (CLAIM_FLOOR * 100.0) as i64
        ));
    }
    said.join("; ") + " — check the page"
}

/// What Jev reads after a click its target had something drawn over: the
/// controls under it are left out while it stays.
pub fn covered_note(label: &str, by: &str, others: usize) -> Value {
    let more = if others > 0 {
        format!(
            " and over {others} other control{}",
            if others == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };
    let label: String = label.chars().take(40).collect();
    json!({
        "action": format!("Not clicked: something ({by}) is drawn over {label}{more}, so they are left out while it stays. Use what is on top."),
        "kind": "click", "text": null, "page_changed": false,
    })
}

/// What Jev reads in `recent_actions` after a DONE was not taken.
pub fn refused_note() -> Value {
    json!({
        "action": "DONE was refused: the page still shows something the goal asks for as not applied or not done. Do what is missing.",
        "kind": "done", "text": null, "page_changed": false,
    })
}

/// What Jev reads when it chose to send again a form the page took without a
/// word: it is asked DONE or BLOCKED instead.
pub fn sent_note() -> Value {
    json!({
        "action": "That was sent once already and the page shows nothing new back. It is not sent again: answer DONE if sending it met the goal, else BLOCKED.",
        "kind": "done", "text": null, "page_changed": false,
    })
}

/// A goal that asks for something read off the page (a question, "tell
/// me…", "find the…"), not only a place to be or a thing done. Anything that
/// commits, types or changes something never is: those keep the strict DONE.
pub fn asks_answer(goal: &str) -> bool {
    static PATTERNS: OnceLock<[Regex; 5]> = OnceLock::new();
    let [acts, question, ask, find_the, place] = PATTERNS.get_or_init(|| {
        [
            r"\b(buy|order|purchase|pay|checkout|check out|book|reserve|submit|send|post|publish|sign (up|in)|log ?in|subscribe|register|delete|remove|cancel|approve|add to|apply|save|create|upload|transfer|reply|comment|download|install|enable|disable|turn (on|off)|change|set|fill|enter|type|select|choose|pick)\b",
            r"^\W*(what|who|whom|whose|when|where|which|why|how|is|are|was|were|does|do|did|list|name)\b",
            r"\b(tell me|let me know|find out|read (me|out)|look up|how (many|much|old|long|far|big)|what (is|are|was|were))\b",
            r"\bfind the\b",
            r"\b(open|go to|navigate|visit|page|article|link|site|website|tab)\b",
        ]
        .map(|p| Regex::new(p).expect("regex"))
    });
    let g = goal.to_lowercase();
    if acts.is_match(&g) {
        return false;
    }
    if g.contains('?') || question.is_match(&g) || ask.is_match(&g) {
        return true;
    }
    // "Find the price…" is a question; "find the pricing page" is a place to go.
    find_the.is_match(&g) && !place.is_match(&g)
}

/// Where an answer stands on the page: the page's own words around it, when
/// the answer is there verbatim (case, accents and spacing aside) or every
/// number in it is; else `None`. The evidence a question's DONE stands on.
pub fn cite(answer: &str, page: &str) -> Option<String> {
    static SPACE: OnceLock<Regex> = OnceLock::new();
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    let space = SPACE.get_or_init(|| Regex::new(r"\s+").expect("regex"));
    let number = NUMBER.get_or_init(|| Regex::new(r"\d[\d,]*(?:\.\d+)*").expect("regex"));
    let squeeze = |s: &str| {
        let s = s
            .replace(['\u{2018}', '\u{2019}'], "'")
            .replace(['\u{201C}', '\u{201D}'], "\"");
        space.replace_all(&s, " ").into_owned()
    };
    let text = squeeze(page);
    let said: String = squeeze(answer)
        .trim_matches([' ', '.', '"', '\''])
        .chars()
        .take(300)
        .collect();
    if said.is_empty()
        || ["null", "none", "n/a", "unknown", "not found", "not shown"]
            .contains(&said.to_lowercase().as_str())
    {
        return None;
    }
    // The page's words 80 characters either side of bytes `lo..hi`.
    let around = |lo: usize, hi: usize| {
        let before: String = {
            let mut b: Vec<char> = text[..lo].chars().rev().take(80).collect();
            b.reverse();
            b.into_iter().collect()
        };
        let after: String = text[hi..].chars().take(80).collect();
        format!("{before}{}{after}", &text[lo..hi])
            .trim_matches([' ', '\t'])
            .to_string()
    };
    if let Some((lo, hi)) = whole(&said, &text) {
        return Some(around(lo, hi));
    }
    // Accents aside: found in the folded text, cited from the page's.
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let folded: Vec<String> = chars.iter().map(|(_, c)| fold(&c.to_string())).collect();
    if folded.iter().all(|f| f.chars().count() == 1) {
        let flat: String = folded.concat();
        // Folded byte offset → original byte offset, char by char.
        let mut map: Vec<usize> = Vec::with_capacity(flat.len() + 1);
        for (f, (i, _)) in folded.iter().zip(&chars) {
            map.extend(std::iter::repeat_n(*i, f.len()));
        }
        map.push(text.len());
        if let Some((lo, hi)) = whole(&fold(&said), &flat) {
            return Some(around(map[lo], map[hi]));
        }
    }
    // Numerically: 1,931 is 1931; a version 0.28.1 stays whole.
    let numbers = |s: &str| -> Vec<(String, usize, usize)> {
        number
            .find_iter(s)
            .filter(|m| {
                !s[..m.start()]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ',')
            })
            .map(|m| (m.as_str().replace(',', ""), m.start(), m.end()))
            .collect()
    };
    let wanted = numbers(&said);
    let first = wanted.first()?;
    let on_page = numbers(&text);
    if !wanted.iter().all(|w| on_page.iter().any(|p| p.0 == w.0)) {
        return None;
    }
    let hit = on_page.iter().find(|p| p.0 == first.0)?;
    Some(around(hit.1, hit.2))
}

/// `said` in `text`, case aside, as whole words and numbers: "2" is not in
/// "2012", nor "0.28" in "0.28.1". Byte range of the first such match.
fn whole(said: &str, text: &str) -> Option<(usize, usize)> {
    let re = Regex::new(&format!("(?i){}", regex::escape(said))).ok()?;
    let ln = |c: char| c.is_alphanumeric();
    let starts = said.chars().next().is_some_and(ln);
    let ends = said.chars().next_back().is_some_and(ln);
    let mut at = 0;
    while let Some(m) = re.find_at(text, at) {
        let mut before = text[..m.start()].chars().rev();
        let b1 = before.next();
        let b2 = before.next();
        let mut after = text[m.end()..].chars();
        let a1 = after.next();
        let a2 = after.next();
        // (?<![\p{L}\p{N}]|\p{N}[.,]) and (?![\p{L}\p{N}]|[.,]\p{N})
        let open = !starts
            || !(b1.is_some_and(ln)
                || (b1.is_some_and(|c| c == '.' || c == ',') && b2.is_some_and(char::is_numeric)));
        let close = !ends
            || !(a1.is_some_and(ln)
                || (a1.is_some_and(|c| c == '.' || c == ',') && a2.is_some_and(char::is_numeric)));
        if open && close {
            return Some((m.start(), m.end()));
        }
        at = m.start() + text[m.start()..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// What a question's answer is asked as: short and in the page's own words,
/// so it can be found there.
pub fn answer_schema() -> Value {
    json!({"type": "object", "properties": {"answer": {
        "type": ["string", "null"],
        "description": "The answer in as few words as the page itself uses (a name, title, number, date or version), copied exactly from the page; null when the page does not show it.",
    }}})
}

/// A field's name for the run's memory of what was typed where: its role
/// and label, lowercased, spacing and punctuation evened out ("Where to? "
/// and "Where to?" are one field), in its document and form (`scope`).
pub fn field_key(action: &JevAction) -> String {
    let label = action.label.to_lowercase();
    let words: Vec<&str> = label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    format!(
        "{}|{}|{}",
        action.role.as_deref().unwrap_or(""),
        words.join(" "),
        action.scope.as_deref().unwrap_or("")
    )
}

/// A field whose typing opens a list to pick from (a combobox, a search
/// box): the read after it must offer that list, so nothing else is typed
/// in the same breath.
pub fn suggests(action: &JevAction) -> bool {
    matches!(action.role.as_deref(), Some("combobox" | "searchbox"))
}

/// The small model the gateway answers a field's value with fastest
/// (~0.6 s and every value right, where haiku took ~1.15 s).
pub const FAST_TEXT_MODEL: &str = "llama";

/// Who writes a field's value, in the order they are asked: the text model
/// Settings names; else the fast one on a gateway; and haiku behind either,
/// so a gateway without that name costs one failed request, not the run.
pub fn text_models(keys: &Keys) -> Vec<String> {
    let named = keys.text_model.trim();
    let mut out: Vec<String> = if !named.is_empty() {
        vec![named.to_string()]
    } else if keys.lane == Lane::Key {
        vec![FAST_TEXT_MODEL.to_string()]
    } else {
        Vec::new()
    };
    if !out.iter().any(|m| family(m) == Some(Tier::Haiku)) {
        out.push("haiku".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(url: &str, text: &str) -> Observation {
        serde_json::from_value(json!({
            "url": url, "title": "", "text": text, "marker": "", "w": 800.0, "h": 600.0,
            "scroll": {"y": 0.0, "height": 600.0}, "page_key": "", "guards": {}, "actions": [],
        }))
        .unwrap()
    }

    fn action(v: Value) -> JevAction {
        serde_json::from_value(v).unwrap()
    }

    fn step(number: usize, kind: &str, label: &str, text: Option<&str>) -> Step {
        Step {
            number,
            operation: if kind == "fill" { "TYPE_TEXT" } else { "CLICK" }.into(),
            kind: kind.into(),
            label: label.into(),
            text: text.map(String::from),
            confidence: 1.0,
            probability: 1.0,
            page_changed: Some(true),
            ..Step::default()
        }
    }

    #[test]
    fn floors() {
        assert!(floor("BLOCKED") == 0.8 && floor("DONE") == CLAIM_FLOOR);
        // r4: every DONE at 80% or more passed; Amazon's false DONE was 67%.
        assert!(CLAIM_FLOOR == 0.8 && 0.67 < floor("DONE"));
        // A WAIT is never shorter than the 238 ms that ended in DONE on Flights.
        assert!(
            WAIT_FLOOR >= Duration::from_millis(240) && WAIT_LINGER >= Duration::from_millis(400)
        );
        // far-link's BLOCKED at 76% on the page it was sent to (r3) is doubted now.
        assert!(0.76 < floor("BLOCKED"));
        assert_eq!(BARREN_LIMIT, 4);
    }

    #[test]
    fn a_field_is_remembered_by_role_and_label() {
        let key = |v: Value| field_key(&action(v));
        let f = |role: &str, label: &str| json!({"id": "e1", "kind": "fill", "role": role, "label": label});
        assert_eq!(
            key(f("combobox", "Where to? ")),
            key(f("combobox", "where to?"))
        );
        assert_ne!(
            key(f("combobox", "Where to?")),
            key(f("combobox", "Where from?"))
        );
        assert_ne!(key(f("textbox", "Name")), key(f("searchbox", "Name")));
        // "Search" in another document or form is another field.
        let s = |label: &str, scope: &str| json!({"id": "e1", "kind": "fill", "role": "searchbox", "label": label, "scope": scope});
        assert_ne!(key(s("Search", "1.5|q")), key(s("Search", "9.1|q")));
        assert_eq!(key(s("Search", "1.5|q")), key(s("search ", "1.5|q")));
    }

    #[test]
    fn text_models_fast_first_haiku_last() {
        let mut keys = Keys::default();
        assert_eq!(text_models(&keys), [FAST_TEXT_MODEL, "haiku"]);
        keys.text_model = "haiku".into();
        assert_eq!(text_models(&keys), ["haiku"]);
        keys.text_model = "luna".into();
        assert_eq!(text_models(&keys), ["luna", "haiku"]);
        keys.text_model = String::new();
        keys.lane = Lane::Claude;
        assert_eq!(text_models(&keys), ["haiku"]);
    }

    // From aMoveThatChangedNothingGetsPicture: the quiet send and how it is said.
    #[test]
    fn a_quiet_send_is_said_as_it_was() {
        let mut step = step(1, "click", "Sign in", None);
        step.page_changed = Some(false);
        step.sent = true;
        step.form_sent = true;
        // A search or a retry that sent is not a commit: never "quiet", the guard stays off.
        assert!(!step.quiet_send());
        step.commit = true;
        // Said as it was: sent, and the page didn't change.
        let recent = step.recent();
        assert!(step.quiet_send() && recent["page_changed"] == false && recent["sent"] == true);
        assert!(
            recent["action"]
                .as_str()
                .unwrap()
                .contains("no confirmation")
        );
        // A fetch POST with no form is the same send (a Pay button): the guard arms on it.
        step.form_sent = false;
        assert!(step.quiet_send());
        step.page_changed = Some(true);
        assert!(!step.quiet_send() && step.recent()["sent"] == true);
        // The read moved but showed nothing new: still quiet.
        step.nothing_new = true;
        assert!(step.quiet_send());
    }

    #[test]
    fn typed_codes_must_show() {
        let goal = "One-way flights ZRH → LHR on 2027-01-20, 1 adult, economy.";
        let typed = |v: &str| step(1, "fill", "Where from?", Some(v));
        let history = [typed("ZRH"), typed("LHR")];
        // r3's results address carries both airports, base64'd.
        let carried = obs(
            "https://www.google.com/travel/flights/search?tfs=CBwQAhoeEgoyMDI3LTAxLTIwagcIARIDWlJIcgcIARIDTEhSQAFIAXABggELCP___________wGYAQI&hl=en",
            "Flights",
        );
        assert!(unshown_typed(&history, goal, &carried).is_empty());
        // London City picked instead of Heathrow: LHR shows nowhere.
        let city = obs(
            "https://www.google.com/travel/flights/search?tfs=CBwQAhoeEgoyMDI3LTAxLTIwagcIARIDWlJIcgcIARIDTENZQAFIAXABggELCP___________wGYAQI",
            "Zurich ZRH to London LCY",
        );
        assert_eq!(unshown_typed(&history, goal, &city), ["LHR"]);
        // Codes the goal names but nothing typed, or typed words that aren't codes: not checked.
        assert!(unshown_typed(&[], goal, &city).is_empty());
        assert!(
            unshown_typed(&[typed("Zurich")], "Flights from Zurich to London", &city).is_empty()
        );
    }

    #[test]
    fn notes_say_what_happened() {
        assert_eq!(
            missing_note(&["LHR".into()])["action"],
            "DONE was refused: the page shows no LHR, which the goal names. Make sure that exact value is chosen."
        );
        assert_eq!(
            unmet_summary(&["LHR".into(), "ZRH".into()], Some(0.62), false),
            "the page shows no LHR, ZRH; something the goal asks for looks not done (62%); Jev stayed under 80% after a second look — check the page"
        );
        assert_eq!(unmet_summary(&[], Some(0.3), true), " — check the page");
        assert_eq!(
            covered_note("Search", "cookie banner", 2)["action"],
            "Not clicked: something (cookie banner) is drawn over Search and over 2 other controls, so they are left out while it stays. Use what is on top."
        );
        assert!(
            covered_note("Go", "x", 1)["action"]
                .as_str()
                .unwrap()
                .contains("over 1 other control,")
        );
        assert_eq!(refused_note()["kind"], "done");
        assert_eq!(sent_note()["page_changed"], false);
        assert_eq!(
            answer_schema()["properties"]["answer"]["type"],
            json!(["string", "null"])
        );
    }

    // A question's DONE stands on its answer found on the page; anything that
    // commits or goes somewhere keeps the strict DONE.
    #[test]
    fn questions_are_told_from_tasks() {
        for goal in [
            "What is the title of the top story right now?",
            "Open the httpx repository and tell me which license it uses.",
            "Search for Gödel's incompleteness theorems, open that article, and tell me the year they were published.",
            "Of the 2012 films listed here, which one won Best Picture?",
            "how many open issues does this repo have",
            "Find the price of the cheapest plan.",
        ] {
            assert!(asks_answer(goal), "{goal}");
        }
        for goal in [
            "Open the abstract page of arXiv paper 1706.03762.",
            "Open the comments page of the top story.",
            "Find and open the Wikipedia article about Gödel’s incompleteness theorems.",
            "Find the link to the article \"Zanzibar\" further down this page and open it.",
            "Use the destination search and filters to find Design stays in Lisbon with Free cancellation, then open Casa Flora.",
            "Find one-way flights from Zurich to London on January 20, 2027. Do not select or book a flight.",
            "Approve invoice \"INV-1042\" and nothing else.",
            "What should I buy? Order the cheapest one.",
            "Sign in with the work email \"maya@northwind.test\".",
        ] {
            assert!(!asks_answer(goal), "{goal}");
        }
    }

    #[test]
    fn answers_are_cited_from_the_page() {
        let page = "Gödel's incompleteness theorems\nThey were published by Kurt Gödel in 1931, and  are important.\nhttpx 0.28.1 · Released: Dec 6, 2024 · 12,345 downloads";
        assert!(
            cite("1931", page)
                .unwrap()
                .contains("published by Kurt Gödel in 1931")
        );
        // case, accents, spacing aside
        assert!(cite("published by kurt godel in 1931, and are important.", page).is_some());
        // numerically
        assert!(cite("0.28.1", page).is_some() && cite("12345", page).is_some());
        // every number in it
        assert!(cite("httpx 0.28.1 (released December 6, 2024)", page).is_some());
        assert!(cite("1930", page).is_none() && cite("0.28", page).is_none());
        assert!(
            cite("Argo", page).is_none()
                && cite("null", page).is_none()
                && cite("", page).is_none()
        );
        // not inside a longer number
        assert!(cite("2", "films of 2012").is_none());
        // whole words
        assert!(cite("Kurt", page).is_some() && cite("urt", page).is_none());
    }
}
