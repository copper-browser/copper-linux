//! `Tools.Page.render` (Fork/MCP/Page.swift:59): a page value as text for the agent.
//!
//! The Mac renders what `evaluateJavaScript` hands back through Foundation:
//! a string as itself, `undefined`/`null` as `undefined`, an object or array
//! as `JSONSerialization` pretty-printed with sorted keys, anything else with
//! `String(describing:)`. These are reproduced byte for byte, quirks included
//! (checked against Swift on macOS 27):
//!
//! - pretty printing is two spaces with `" : "` between key and value, and an
//!   empty container prints as `{\n\n}` / `[\n\n]` at any depth;
//! - strings escape `/` as `\/`, `"` `\\` `\n` `\r` `\t` `\b` `\f`, other C0
//!   controls as `\u00XX`, and leave everything else (including non-ASCII) raw;
//! - numbers inside JSON print like C `%.17g`; a bare number like `%.16g`;
//!   a bare boolean as `1` / `0`;
//! - `sortedKeys` orders keys the way Foundation's localized comparison does
//!   for ASCII: punctuation before digits before letters (punctuation in
//!   collation order), digit runs by numeric value, case-insensitively with
//!   lowercase first on a tie. Non-ASCII characters sort after ASCII by code
//!   point (Foundation folds accents; such keys are rare in page results).

use std::cmp::Ordering;

use serde_json::Value;

/// `Page.render`: `None` is a JavaScript `undefined` result.
pub fn render(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "undefined".into(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => if *b { "1" } else { "0" }.into(),
        Some(Value::Number(n)) => format_g(n.as_f64().unwrap_or(0.0), 16),
        Some(container) => {
            let mut out = String::new();
            pretty(container, 0, &mut out);
            out
        }
    }
}

/// `JSONSerialization.data(withJSONObject:options:[.prettyPrinted, .sortedKeys])`
/// for an object or array.
pub fn pretty_json(value: &Value) -> String {
    let mut out = String::new();
    pretty(value, 0, &mut out);
    out
}

/// `JSONSerialization.data(withJSONObject:options:[.sortedKeys])` (compact).
pub fn compact_json(value: &Value) -> String {
    let mut out = String::new();
    compact(value, &mut out);
    out
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn pretty(value: &Value, depth: usize, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push_str("{\n");
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| compare_keys(a, b));
            for (i, key) in keys.iter().enumerate() {
                indent(depth + 1, out);
                string(key, out);
                out.push_str(" : ");
                pretty(&map[key.as_str()], depth + 1, out);
                if i + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            if keys.is_empty() {
                out.push('\n');
            }
            indent(depth, out);
            out.push('}');
        }
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                indent(depth + 1, out);
                pretty(item, depth + 1, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            if items.is_empty() {
                out.push('\n');
            }
            indent(depth, out);
            out.push(']');
        }
        scalar => compact(scalar, out),
    }
}

fn compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&format_g(n.as_f64().unwrap_or(0.0), 17)),
        Value::String(s) => string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                compact(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| compare_keys(a, b));
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(key, out);
                out.push(':');
                compact(&map[key.as_str()], out);
            }
            out.push('}');
        }
    }
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// C `printf("%.{precision}g", v)`.
pub fn format_g(v: f64, precision: usize) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.into();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let p = precision.max(1);
    // Scientific form with p significant digits gives the rounded exponent.
    let sci = format!("{:.*e}", p - 1, v);
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if exponent < -4 || exponent >= p as i32 {
        let mantissa = strip_zeros(mantissa);
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        let decimals = (p as i32 - 1 - exponent).max(0) as usize;
        strip_zeros(&format!("{v:.decimals$}")).to_string()
    }
}

fn strip_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// Collation rank of an ASCII character outside digits and letters, in the
/// order Foundation sorts them.
const PUNCTUATION: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Unit<'a> {
    /// A punctuation/space character, by collation rank.
    Mark(usize),
    /// A run of digits: (value without leading zeros, its length).
    Number(&'a str),
    /// A letter, case-folded.
    Letter(u8),
    /// Anything else, by code point.
    Other(u32),
}

fn units(s: &str) -> Vec<Unit<'_>> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap_or_default();
        if c.is_ascii_digit() {
            let start = i;
            while i < s.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let run = s[start..i].trim_start_matches('0');
            out.push(Unit::Number(run));
            continue;
        }
        out.push(if c.is_ascii_alphabetic() {
            Unit::Letter(c.to_ascii_lowercase() as u8)
        } else if let Some(rank) = PUNCTUATION.find(c).filter(|_| c.is_ascii()) {
            Unit::Mark(rank)
        } else if c.is_ascii() {
            Unit::Mark(PUNCTUATION.len() + c as usize)
        } else {
            Unit::Other(c as u32)
        });
        i += c.len_utf8();
    }
    out
}

fn number_order(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn primary(a: &[Unit<'_>], b: &[Unit<'_>]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        let order = match (x, y) {
            (Unit::Number(m), Unit::Number(n)) => number_order(m, n),
            _ => x.cmp(y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// The key order of `JSONSerialization`'s `.sortedKeys` (see the module docs).
pub fn compare_keys(a: &str, b: &str) -> Ordering {
    primary(&units(a), &units(b))
        // "x1" before "x01": fewer characters (leading zeros) first.
        .then_with(|| a.len().cmp(&b.len()))
        // On a case-only difference, lowercase first.
        .then_with(|| {
            for (x, y) in a.chars().zip(b.chars()) {
                if x != y {
                    return match (x.is_ascii_lowercase(), y.is_ascii_lowercase()) {
                        (true, false) => Ordering::Less,
                        (false, true) => Ordering::Greater,
                        _ => x.cmp(&y),
                    };
                }
            }
            Ordering::Equal
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scalars_like_foundation() {
        assert_eq!(render(None), "undefined");
        assert_eq!(render(Some(&Value::Null)), "undefined");
        assert_eq!(render(Some(&json!("hi/there"))), "hi/there");
        assert_eq!(render(Some(&json!(true))), "1");
        assert_eq!(render(Some(&json!(false))), "0");
        assert_eq!(render(Some(&json!(3.0))), "3");
        assert_eq!(render(Some(&json!(1.5))), "1.5");
        assert_eq!(render(Some(&json!(0.1))), "0.1");
        assert_eq!(render(Some(&json!(1e21))), "1e+21");
        assert_eq!(render(Some(&json!(123456789012.0))), "123456789012");
        assert_eq!(render(Some(&json!(99.99))), "99.98999999999999");
        assert_eq!(render(Some(&json!(1e-7))), "1e-07");
    }

    #[test]
    #[allow(clippy::approx_constant)] // 3.14159 is a sample value, not π
    fn json_numbers_like_foundation() {
        // Each pair was printed by JSONSerialization on macOS.
        let cases = [
            (0.1, "0.10000000000000001"),
            (1.1, "1.1000000000000001"),
            (2.5, "2.5"),
            (3.14159, "3.1415899999999999"),
            (123.456, "123.456"),
            (1e-7, "9.9999999999999995e-08"),
            (1e20, "1e+20"),
            (1e21, "1e+21"),
            (1e16, "10000000000000000"),
            (0.000123, "0.00012300000000000001"),
            (1.0 / 3.0, "0.33333333333333331"),
            (-2.75, "-2.75"),
            (1e300, "1.0000000000000001e+300"),
            (99.99, "99.989999999999995"),
            (1e15 + 0.5, "1000000000000000.5"),
        ];
        for (v, want) in cases {
            assert_eq!(compact_json(&json!([v])), format!("[{want}]"), "{v}");
        }
    }

    #[test]
    fn pretty_like_foundation() {
        let value = json!({
            "b": 1.0, "a": ["x/y", true, null], "c": {}, "d": [],
            "e": "é\u{1}\"\\\n\t", "f": 2.5, "g": 1e-7, "h": 1e20
        });
        let want = "{\n  \"a\" : [\n    \"x\\/y\",\n    true,\n    null\n  ],\n  \"b\" : 1,\n  \"c\" : {\n\n  },\n  \"d\" : [\n\n  ],\n  \"e\" : \"é\\u0001\\\"\\\\\\n\\t\",\n  \"f\" : 2.5,\n  \"g\" : 9.9999999999999995e-08,\n  \"h\" : 1e+20\n}";
        assert_eq!(render(Some(&value)), want);
        assert_eq!(render(Some(&json!([]))), "[\n\n]");
        assert_eq!(render(Some(&json!({}))), "{\n\n}");
        assert_eq!(render(Some(&json!([1.0, "two"]))), "[\n  1,\n  \"two\"\n]");
    }

    #[test]
    fn escapes_like_foundation() {
        assert_eq!(
            compact_json(&json!(["a\r\u{8}\u{c}\u{7f}\u{2028}😀<>&'"])),
            "[\"a\\r\\b\\f\u{7f}\u{2028}😀<>&'\"]"
        );
    }

    #[test]
    fn key_order_like_foundation() {
        // Orders printed by JSONSerialization(.sortedKeys) on macOS.
        let sorted = |keys: &[&str]| {
            let mut v: Vec<String> = keys.iter().map(|s| s.to_string()).collect();
            v.sort_by(|a, b| compare_keys(a, b));
            v
        };
        assert_eq!(
            sorted(&[
                "a10",
                "a2",
                "A",
                "a",
                "e1",
                "x_y",
                "x-y",
                "xy",
                "x.y",
                "callbacks",
                "callSites",
                "Ab",
                "aB"
            ]),
            [
                "a",
                "A",
                "a2",
                "a10",
                "aB",
                "Ab",
                "callbacks",
                "callSites",
                "e1",
                "x_y",
                "x-y",
                "x.y",
                "xy"
            ]
        );
        assert_eq!(
            sorted(&["x", "x y", "x_", "x0", "xa", "x9", "x10", "x01", "x1"]),
            ["x", "x y", "x_", "x0", "x1", "x01", "x9", "x10", "xa"]
        );
        assert_eq!(
            sorted(&["Z", "a", "_", "B", "ab", "a b"]),
            ["_", "a", "a b", "ab", "B", "Z"]
        );
        let printable: Vec<String> = (32u8..=126).map(|c| format!("x{}y", c as char)).collect();
        let refs: Vec<&str> = printable.iter().map(String::as_str).collect();
        let order: String = sorted(&refs)
            .iter()
            .map(|k| k.chars().nth(1).unwrap())
            .collect();
        assert_eq!(
            order,
            " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789aAbBcCdDeEfFgGhHiIjJkKlLmMnNoOpPqQrRsStTuUvVwWxXyYzZ"
        );
    }
}
