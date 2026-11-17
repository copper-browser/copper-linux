//! A secret Copper filled never reads back out to an agent (Mac
//! `Fork/MCP/AgentSecrets.swift`).
//!
//! Copper fills passwords, one-time codes, card numbers and codes and vault
//! fields for agents (`browser_sign_in`, `browser_autofill`, Jev's SIGN_IN)
//! and for the user without ever returning them. Two layers keep them from
//! coming back out another way:
//!
//! 1. Structure. The page scripts (`page/helper.js`, `jev/fast.js`) print the
//!    value of a secret box — a password box, one whose autocomplete promises
//!    a password, a one-time code, a card number or its code, or one Copper
//!    filled with a secret (`forms/relay.js` marks those) — as [`PLACEHOLDER`].
//! 2. Words (this module). Every secret string Copper filled is remembered
//!    here, in memory, per tab, for the tab's life, and any occurrence of it —
//!    raw, JSON-escaped (once or twice), URL-encoded, HTML-escaped — becomes
//!    [`PLACEHOLDER`] in whatever an agent or a model is handed: every tool's
//!    answer and error ([`crate::Mcp::tools_call`]), the Jev request
//!    ([`crate::intelligence::jev_ask`]), the router's
//!    (`Mcp::ask_model`), the agent pane's page context and turns, and the
//!    driver timeline ([`crate::drive::DriveEvent::scrubbed`]). That catches
//!    a page that copies the password where no structure rule can see it:
//!    into a div, the console, an address, another box.
//!
//! Nothing here is written to disk, logged or returned: the store answers
//! counts, never strings ([`held`] is for the page's own masking only).

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex, MutexGuard};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

use crate::host::{Content, TabId};

/// What a secret reads as wherever an agent could see it.
pub const PLACEHOLDER: &str = "•••• (secret, hidden)";

/// Shorter secrets are not hunted for in text (a two-letter "secret" would
/// rub out half of every answer). Digits-only secrets (a card's three-digit
/// code, a six-digit one-time code) count from three, and only standing
/// alone: "123" is not taken out of "41234" or a ref "e123".
pub const SHORTEST: usize = 4;
pub const SHORTEST_DIGITS: usize = 3;

#[derive(Default)]
struct Store {
    by_tab: HashMap<String, Vec<String>>,
    /// Every variant of every secret, longest first; rebuilt on change.
    compiled: Option<Vec<Needle>>,
}

#[derive(Clone)]
struct Needle {
    text: String,
    digits: bool,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn store() -> MutexGuard<'static, Store> {
    STORE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A secret Copper is about to put into `tab`'s page. Called before the
/// fill, so an echo the page makes the instant the value lands is known.
pub fn remember(tab: &TabId, secret: &str) {
    if secret.is_empty() {
        return;
    }
    let mut s = store();
    let list = s.by_tab.entry(tab.0.clone()).or_default();
    if list.iter().any(|known| known == secret) {
        return;
    }
    list.push(secret.to_string());
    s.compiled = None;
}

/// The tab is gone: what it was given goes with it.
pub fn forget(tab: &TabId) {
    let mut s = store();
    if s.by_tab.remove(&tab.0).is_some() {
        s.compiled = None;
    }
}

/// How many secrets `tab` holds (tests; never the strings).
pub fn count(tab: &TabId) -> usize {
    store().by_tab.get(&tab.0).map_or(0, Vec::len)
}

/// The secrets `tab` holds that are long enough to look for: only for the
/// page's own masking before a picture (`screenshot`), never returned.
pub(crate) fn held(tab: &TabId) -> Vec<String> {
    store()
        .by_tab
        .get(&tab.0)
        .map(|list| {
            list.iter()
                .filter(|s| s.chars().count() >= SHORTEST)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn needles() -> Vec<Needle> {
    let mut s = store();
    if let Some(compiled) = &s.compiled {
        return compiled.clone();
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for secret in s.by_tab.values().flatten() {
        let digits = is_digits(secret);
        let floor = if digits { SHORTEST_DIGITS } else { SHORTEST };
        if secret.chars().count() < floor {
            continue;
        }
        for variant in variants(secret) {
            if !variant.is_empty() && seen.insert(variant.clone()) {
                out.push(Needle {
                    digits: is_digits(&variant),
                    text: variant,
                });
            }
        }
    }
    // Longest first: an escaped form goes whole before its raw core.
    out.sort_by_key(|n| std::cmp::Reverse(n.text.len()));
    s.compiled = Some(out.clone());
    out
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `encodeURIComponent` keeps these.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');
/// A form body (`application/x-www-form-urlencoded`) keeps these, and
/// writes a space as `+`.
const FORM: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'*')
    .remove(b' ');
/// What a URL's query keeps as it is (WHATWG / Foundation `urlQueryAllowed`).
const QUERY: &AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'|')
    .add(b'\\')
    .add(b'^')
    .add(b'%')
    .add(b'[')
    .add(b']');

fn json_inner(s: &str) -> String {
    let quoted = serde_json::to_string(s).unwrap_or_default();
    quoted
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .unwrap_or(&quoted)
        .to_string()
}

fn lower_hex(encoded: &str) -> String {
    let mut out = String::with_capacity(encoded.len());
    let mut hex = 0;
    for c in encoded.chars() {
        if c == '%' {
            hex = 2;
            out.push(c);
        } else if hex > 0 {
            out.extend(c.to_lowercase());
            hex -= 1;
        } else {
            out.push(c);
        }
    }
    out
}

/// The forms a secret takes on its way out of a page: itself, in a JSON
/// string (also with `\/`, with non-ASCII as `\u` escapes, and escaped twice,
/// as JSON inside JSON), URL-encoded (a component, a form body, a query; upper-
/// and lowercase hex), HTML-escaped, and a card number in groups of four.
pub fn variants(secret: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![secret.to_string()];
    let mut add = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };
    let json = json_inner(secret);
    add(json.clone());
    add(json.replace('/', "\\/"));
    if !json.is_ascii() {
        let mut ascii = String::new();
        for unit in json.encode_utf16() {
            if unit < 0x80 {
                ascii.push(unit as u8 as char);
            } else {
                ascii.push_str(&format!("\\u{unit:04x}"));
            }
        }
        add(ascii);
    }
    let twice = json_inner(&json);
    add(twice.clone());
    add(twice.replace('/', "\\/"));

    let component = utf8_percent_encode(secret, COMPONENT).to_string();
    add(lower_hex(&component));
    add(component);
    let form = utf8_percent_encode(secret, FORM)
        .to_string()
        .replace(' ', "+");
    add(lower_hex(&form));
    add(form);
    let query = utf8_percent_encode(secret, QUERY).to_string();
    add(lower_hex(&query));
    add(query);

    add(secret
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;"));

    let n = secret.len();
    if is_digits(secret) && (12..=19).contains(&n) {
        let groups: Vec<&str> = (0..n)
            .step_by(4)
            .map(|i| &secret[i..(i + 4).min(n)])
            .collect();
        add(groups.join(" "));
        add(groups.join("-"));
    }
    out
}

fn scrub_with(text: &str, needles: &[Needle]) -> String {
    let mut text = text.to_string();
    for needle in needles {
        if !text.contains(&needle.text) {
            continue;
        }
        text = if needle.digits {
            replace_digit_run(&text, &needle.text)
        } else {
            text.replace(&needle.text, PLACEHOLDER)
        };
    }
    text
}

/// `run` replaced wherever it stands alone: no letter or digit either side.
fn replace_digit_run(text: &str, run: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(run) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + run.len()..].chars().next();
        let alone = !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric());
        out.push_str(&rest[..at]);
        out.push_str(if alone { PLACEHOLDER } else { run });
        rest = &rest[at + run.len()..];
    }
    out.push_str(rest);
    out
}

/// `text` with every known secret, in any of its encodings, as
/// [`PLACEHOLDER`]. Every live tab's secrets, not only one tab's: a page can
/// hand a value to another tab, and an answer may name several.
pub fn scrub(text: &str) -> String {
    if text.is_empty() {
        return text.to_string();
    }
    let needles = needles();
    if needles.is_empty() {
        return text.to_string();
    }
    scrub_with(text, &needles)
}

/// A JSON value with every string in it scrubbed, object keys too: for a
/// model request, before it is serialized, so a replacement can never break
/// the JSON around it.
pub fn scrub_value(value: &Value) -> Value {
    let needles = needles();
    if needles.is_empty() {
        return value.clone();
    }
    walk(value, &needles)
}

fn walk(value: &Value, needles: &[Needle]) -> Value {
    match value {
        Value::String(s) => Value::String(scrub_with(s, needles)),
        Value::Array(list) => Value::Array(list.iter().map(|v| walk(v, needles)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (scrub_with(k, needles), walk(v, needles)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// What every tool answer passes through on its way out: texts scrubbed,
/// pictures as they are (they are masked before they are taken).
pub fn outgoing(content: Vec<Content>) -> Vec<Content> {
    let needles = needles();
    if needles.is_empty() {
        return content;
    }
    content
        .into_iter()
        .map(|c| match c {
            Content::Text(s) => Content::Text(scrub_with(&s, &needles)),
            other => other,
        })
        .collect()
}

/// A picture can't be scrubbed after it is taken. Before one is taken of a
/// page holding a secret Copper filled, what shows that secret as plain text
/// (a box that is no password box, words the page wrote) is drawn masked;
/// `masking: false` puts it back. One JavaScript function body; its argument
/// is `{ masking, secrets }`, answers how many things it masked.
pub const MASK_SCRIPT: &str = r#"({ masking, secrets }) => {
  const held = (secrets || []).filter(s => typeof s === 'string' && s.length >= 4);
  const kept = window.__copperMasked || (window.__copperMasked = []);
  if (!masking) { for (const [el, prop, value, priority] of kept.splice(0)) { try { el.style.setProperty(prop, value, priority); } catch (e) {} } return 0; }
  const has = s => !!s && held.some(x => String(s).includes(x));
  const mask = (el, prop, value) => { kept.push([el, prop, el.style.getPropertyValue(prop), el.style.getPropertyPriority(prop)]); el.style.setProperty(prop, value, 'important'); };
  const marked = window.__copperSecretFields;
  let n = 0;
  for (const el of document.querySelectorAll('input, textarea')) {
    const type = String(el.type).toLowerCase();
    if (type === 'password' || type === 'hidden') continue;
    if (has(el.value) || (el.value && marked && marked.has(el))) { mask(el, '-webkit-text-security', 'disc'); mask(el, 'color', 'transparent'); n++; }
  }
  if (document.body && held.length) {
    const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walk.nextNode(); node; node = walk.nextNode()) {
      const parent = node.parentElement;
      if (parent && has(node.textContent)) { mask(parent, 'color', 'transparent'); mask(parent, 'text-shadow', 'none'); n++; }
    }
  }
  return n;
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str) -> TabId {
        TabId(format!("secrets-test-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn fresh() -> String {
        format!(
            "Zq7/{} ä\"&<x>%+",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        )
    }

    /// Every way the secret could be written, as Copper knows it and as a
    /// page (JSON.stringify, encodeURIComponent) or serde would write it.
    fn encodings(secret: &str) -> Vec<String> {
        let mut all = variants(secret);
        all.push(serde_json::to_string(secret).unwrap());
        all.push(utf8_percent_encode(secret, COMPONENT).to_string());
        all
    }

    fn leaks(text: &str, secret: &str) -> Vec<String> {
        encodings(secret)
            .into_iter()
            .filter(|e| text.contains(e.as_str()))
            .collect()
    }

    #[test]
    fn every_encoding_becomes_the_placeholder() {
        let t = tab("enc");
        let secret = fresh();
        remember(&t, &secret);
        let object = serde_json::json!({ "password": secret }).to_string();
        let twice = serde_json::to_string(&object).unwrap();
        let url = format!(
            "https://x.test/collect?pw={}",
            utf8_percent_encode(&secret, COMPONENT)
        );
        let text = [
            secret.clone(),
            object,
            twice,
            url,
            encodings(&secret).join(" | "),
        ]
        .join("\n");
        assert!(!leaks(&text, &secret).is_empty());
        let clean = scrub(&text);
        assert!(
            leaks(&clean, &secret).is_empty(),
            "leaked: {:?}",
            leaks(&clean, &secret).len()
        );
        assert!(clean.contains(PLACEHOLDER));
        forget(&t);
    }

    #[test]
    fn digits_only_where_they_stand_alone() {
        let t = tab("digits");
        let otp = format!(
            "{:06}",
            100_000 + (uuid::Uuid::new_v4().as_u128() % 900_000) as u32
        );
        let card = format!(
            "4{:015}",
            uuid::Uuid::new_v4().as_u128() % 1_000_000_000_000_000
        );
        remember(&t, &otp);
        remember(&t, &card);
        assert_eq!(
            scrub(&format!("code: {otp}.")),
            format!("code: {PLACEHOLDER}.")
        );
        let kept = format!("ref e{otp} and 9{otp}9");
        assert_eq!(scrub(&kept), kept);
        let groups: Vec<&str> = (0..16).step_by(4).map(|i| &card[i..i + 4]).collect();
        let shown = scrub(&format!(
            "[{card}] {} {}",
            groups.join(" "),
            groups.join("-")
        ));
        assert!(
            !shown.contains(&card)
                && !shown.contains(&groups.join(" "))
                && !shown.contains(&groups.join("-"))
        );
        forget(&t);
    }

    #[test]
    fn too_short_to_hunt_for() {
        let t = tab("short");
        remember(&t, "q7!");
        assert_eq!(scrub("q7! stays"), "q7! stays");
        forget(&t);
    }

    #[test]
    fn the_set_dies_with_the_tab() {
        let t = tab("dies");
        let secret = fresh();
        remember(&t, &secret);
        assert_eq!(count(&t), 1);
        assert_eq!(scrub(&secret), PLACEHOLDER);
        forget(&t);
        assert_eq!(count(&t), 0);
        assert_eq!(scrub(&format!("x {secret} y")), format!("x {secret} y"));
    }

    #[test]
    fn model_requests_stay_valid_json() {
        let t = tab("json");
        let secret = fresh();
        remember(&t, &secret);
        let state = serde_json::json!({
            "page": { "text": format!("Checking {secret}"), "title": secret },
            "elements": [{ "label": "Password", "value": secret }],
            secret.clone(): [secret, 3, true],
        });
        let text = serde_json::to_string(&scrub_value(&state)).unwrap();
        assert!(leaks(&text, &secret).is_empty());
        assert!(text.contains(PLACEHOLDER));
        assert!(serde_json::from_str::<Value>(&text).is_ok());
        forget(&t);
    }

    #[test]
    fn tool_answers_are_scrubbed_pictures_kept() {
        let t = tab("content");
        let secret = fresh();
        remember(&t, &secret);
        let out = outgoing(vec![
            Content::text(format!("console: typed password {secret}")),
            Content::Image {
                data: vec![1, 2, 3],
                mime: "image/png".into(),
            },
        ]);
        let Content::Text(text) = &out[0] else {
            panic!("text first")
        };
        assert!(leaks(text, &secret).is_empty() && text.contains(PLACEHOLDER));
        assert!(matches!(&out[1], Content::Image { data, .. } if data == &vec![1, 2, 3]));
        forget(&t);
    }
}
