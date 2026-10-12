//! JSON encoding that reads like Swift's, and a structural diff.
//!
//! * [`encode`] writes any `Serialize` value with **sorted keys** (every Mac
//!   encoder that matters either sorts or has no stable order) and with
//!   integral doubles written as integers (`0`, not `0.0`), as Swift's
//!   `JSONEncoder` does. [`Style::Pretty`] matches `.prettyPrinted` closely
//!   enough for humans; byte equality with Swift is not a goal, semantic
//!   equality is.
//! * [`json_diff`] reports removed keys, added keys, type changes and value
//!   changes between two documents; numbers compare by value (an integer and
//!   a float holding the same number are equal).

use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter};
use serde_json::{Map, Value};
use std::io;

/// How [`encode`] lays out the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    /// One line, no spaces (Swift's default).
    #[default]
    Compact,
    /// Two-space indentation (Swift's `.prettyPrinted`).
    Pretty,
}

/// Serialize `value` the way the Mac would: sorted keys, integral doubles as
/// integers.
pub fn encode<T: Serialize + ?Sized>(value: &T, style: Style) -> serde_json::Result<Vec<u8>> {
    let value = sort_keys(serde_json::to_value(value)?);
    encode_value(&value, style)
}

/// [`encode`] for a value that is already a [`Value`].
pub fn encode_value(value: &Value, style: Style) -> serde_json::Result<Vec<u8>> {
    let value = sort_keys(value.clone());
    let mut out = Vec::with_capacity(256);
    match style {
        Style::Compact => {
            let mut ser =
                serde_json::Serializer::with_formatter(&mut out, SwiftNumbers(CompactFormatter));
            value.serialize(&mut ser)?;
        }
        Style::Pretty => {
            let mut ser = serde_json::Serializer::with_formatter(
                &mut out,
                SwiftNumbers(PrettyFormatter::with_indent(b"  ")),
            );
            value.serialize(&mut ser)?;
        }
    }
    Ok(out)
}

/// Rebuild every object with its keys in sorted order. With serde_json's
/// default `BTreeMap` maps this is a no-op; with `preserve_order` enabled
/// elsewhere in the build it makes the output sorted anyway.
pub fn sort_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = Map::new();
            for (k, v) in entries {
                out.insert(k, sort_keys(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sort_keys).collect()),
        other => other,
    }
}

/// Wraps a formatter so integral finite doubles print as integers, the way
/// Swift's `JSONEncoder` prints `Double`.
struct SwiftNumbers<F>(F);

const EXACT_INT: f64 = 9_007_199_254_740_992.0; // 2^53

impl<F: Formatter> Formatter for SwiftNumbers<F> {
    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, v: f64) -> io::Result<()> {
        if v.is_finite() && v.fract() == 0.0 && v.abs() < EXACT_INT {
            if v == 0.0 {
                // -0.0 too: Swift writes "-0"; "0" is the same number.
                return w.write_all(b"0");
            }
            return write!(w, "{}", v as i64);
        }
        self.0.write_f64(w, v)
    }
    fn write_f32<W: ?Sized + io::Write>(&mut self, w: &mut W, v: f32) -> io::Result<()> {
        self.write_f64(w, v as f64)
    }
    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_array(w)
    }
    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_array(w)
    }
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_array_value(w, first)
    }
    fn end_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_array_value(w)
    }
    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_object(w)
    }
    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_object(w)
    }
    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.0.begin_object_key(w, first)
    }
    fn end_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_object_key(w)
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.begin_object_value(w)
    }
    fn end_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.0.end_object_value(w)
    }
}

/// One difference between two JSON documents, with a JSON-pointer-ish path
/// (`/tabs/3/url`).
#[derive(Debug, Clone, PartialEq)]
pub enum JsonDifference {
    /// The key exists in the original and not in the new document.
    Removed { path: String },
    /// The key exists only in the new document.
    Added { path: String },
    /// Same path, different JSON type (null/bool/number/string/array/object).
    TypeChanged {
        path: String,
        from: &'static str,
        to: &'static str,
    },
    /// Same path and type, different value (arrays of different length are
    /// reported here too).
    ValueChanged {
        path: String,
        from: Value,
        to: Value,
    },
}

impl JsonDifference {
    /// True for differences that would lose Mac data or break a Mac decoder:
    /// removed keys and type changes.
    pub fn is_breaking(&self) -> bool {
        matches!(
            self,
            JsonDifference::Removed { .. } | JsonDifference::TypeChanged { .. }
        )
    }
}

/// The JSON type name of a value.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Every difference from `original` to `new`. Empty means semantically equal.
pub fn json_diff(original: &Value, new: &Value) -> Vec<JsonDifference> {
    let mut out = Vec::new();
    diff_into(original, new, &mut String::new(), &mut out);
    out
}

fn numbers_equal(a: &serde_json::Number, b: &serde_json::Number) -> bool {
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
        return x == y;
    }
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn diff_into(a: &Value, b: &Value, path: &mut String, out: &mut Vec<JsonDifference>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, va) in x {
                let len = path.len();
                path.push('/');
                path.push_str(k);
                match y.get(k) {
                    Some(vb) => diff_into(va, vb, path, out),
                    None => out.push(JsonDifference::Removed { path: path.clone() }),
                }
                path.truncate(len);
            }
            for k in y.keys().filter(|k| !x.contains_key(*k)) {
                out.push(JsonDifference::Added {
                    path: format!("{path}/{k}"),
                });
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(JsonDifference::ValueChanged {
                    path: path.clone(),
                    from: Value::from(x.len()),
                    to: Value::from(y.len()),
                });
            }
            for (i, (va, vb)) in x.iter().zip(y).enumerate() {
                let len = path.len();
                path.push('/');
                path.push_str(&i.to_string());
                diff_into(va, vb, path, out);
                path.truncate(len);
            }
        }
        (Value::Number(x), Value::Number(y)) => {
            if !numbers_equal(x, y) {
                out.push(JsonDifference::ValueChanged {
                    path: path.clone(),
                    from: a.clone(),
                    to: b.clone(),
                });
            }
        }
        _ if type_name(a) != type_name(b) => out.push(JsonDifference::TypeChanged {
            path: path.clone(),
            from: type_name(a),
            to: type_name(b),
        }),
        _ => {
            if a != b {
                out.push(JsonDifference::ValueChanged {
                    path: path.clone(),
                    from: a.clone(),
                    to: b.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn integral_doubles_print_as_integers() {
        let v = json!({"b": 1.0, "a": [0.0, 0.5, 813521284.427064, -2.0], "c": 1u64 << 60});
        let text = String::from_utf8(encode_value(&v, Style::Compact).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{"a":[0,0.5,813521284.427064,-2],"b":1,"c":1152921504606846976}"#
        );
    }

    #[test]
    fn diff_reports_kinds() {
        let a = json!({"x": 1, "y": "s", "z": [1, 2], "gone": true});
        let b = json!({"x": 1.0, "y": 2, "z": [1, 3], "new": null});
        let d = json_diff(&a, &b);
        assert!(d.contains(&JsonDifference::Removed {
            path: "/gone".into()
        }));
        assert!(d.contains(&JsonDifference::Added {
            path: "/new".into()
        }));
        assert!(
            d.iter()
                .any(|x| matches!(x, JsonDifference::TypeChanged { path, .. } if path == "/y"))
        );
        assert!(
            d.iter()
                .any(|x| matches!(x, JsonDifference::ValueChanged { path, .. } if path == "/z/1"))
        );
        assert!(
            !d.iter()
                .any(|x| matches!(x, JsonDifference::ValueChanged { path, .. } if path == "/x"))
        );
    }
}
