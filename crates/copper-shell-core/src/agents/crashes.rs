//! `browser_crashes`: what `crashes/crashes.json` keeps, in the shape the
//! Mac answers with (`Crashes.answer` in `Fork/CrashesRead.swift`): a
//! listing (`folder`, `count`, `crashes[]`, newest first) or one entry in
//! full (`index: N`). Pretty JSON, sorted keys, slashes as they are.

use copper_core::crashes::{CrashEntry, CrashIndex};
use copper_core::{DataFile, Store};
use serde_json::{Map, Value, json};

/// The answer for `args` (`index`, `limit`).
pub fn answer(store: &Store, args: &Map<String, Value>) -> Result<String, String> {
    let file = store.path(DataFile::Crashes);
    let folder = file
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut index = store
        .load::<CrashIndex>()
        .map(|l| l.value)
        .unwrap_or_default();
    index
        .entries
        .sort_by_key(|e| std::cmp::Reverse(e.time.unix()));
    let number = |key: &str| args.get(key).and_then(Value::as_f64).map(|n| n as i64);
    if let Some(n) = number("index") {
        let count = index.entries.len();
        if n < 1 || n as usize > count {
            return Err(if count == 0 {
                "No crashes recorded.".into()
            } else {
                format!("index is 1–{count}")
            });
        }
        let entry = &index.entries[n as usize - 1];
        return Ok(encode(&detail(entry, n, &folder)));
    }
    let limit = number("limit").unwrap_or(20).max(1) as usize;
    Ok(encode(&listing(&index, limit, &folder)))
}

/// What a person would call it (`Entry.headline`).
pub fn headline(e: &CrashEntry) -> String {
    match e.kind.as_str() {
        "unclean" => "quit unexpectedly, no report".into(),
        "exception" => e
            .exception
            .clone()
            .unwrap_or_else(|| "uncaught exception".into()),
        _ => match (&e.exception, &e.signal) {
            (Some(x), Some(s)) => format!("{x} ({s})"),
            (Some(x), None) => x.clone(),
            (None, _) => e
                .reason
                .as_ref()
                .map(|r| r.chars().take(60).collect())
                .unwrap_or_else(|| "crash".into()),
        },
    }
}

/// The first frame in Copper's own code (`Entry.topFrame.text`).
fn top_frame(e: &CrashEntry) -> Option<String> {
    let f = e
        .frames
        .as_ref()?
        .iter()
        .find(|f| f.image.eq_ignore_ascii_case("copper"))?;
    Some(match &f.symbol {
        Some(s) => format!("{s} + {}", f.offset),
        None => format!("{} + {}", f.image, f.offset),
    })
}

fn file_path(e: &CrashEntry, folder: &str) -> Option<String> {
    e.file.as_ref().map(|f| format!("{folder}/{f}"))
}

fn listing(index: &CrashIndex, limit: usize, folder: &str) -> Value {
    let rows: Vec<Value> = index
        .entries
        .iter()
        .take(limit)
        .enumerate()
        .map(|(n, e)| {
            let mut row = Map::new();
            row.insert("n".into(), json!(n + 1));
            row.insert("id".into(), json!(e.id));
            row.insert("kind".into(), json!(e.kind));
            row.insert("time".into(), json!(e.time.to_string()));
            row.insert("what".into(), json!(headline(e)));
            let mut opt = |k: &str, v: Option<String>| {
                if let Some(v) = v {
                    row.insert(k.into(), json!(v));
                }
            };
            opt("version", e.version.clone());
            opt("exception", e.exception.clone());
            opt("signal", e.signal.clone());
            opt("topFrame", top_frame(e));
            opt("file", file_path(e, folder));
            Value::Object(row)
        })
        .collect();
    json!({ "folder": folder, "count": index.entries.len(), "crashes": rows })
}

fn detail(e: &CrashEntry, n: i64, folder: &str) -> Value {
    let mut out = serde_json::to_value(e).unwrap_or_else(|_| json!({}));
    if let Some(o) = out.as_object_mut() {
        o.insert("n".into(), json!(n));
        o.insert("what".into(), json!(headline(e)));
        match file_path(e, folder) {
            Some(f) => {
                o.insert("file".into(), json!(f));
            }
            None => {
                o.remove("file");
            }
        }
    }
    out
}

fn encode(v: &Value) -> String {
    copper_pagejs::render::pretty_json(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use copper_core::Paths;

    fn store() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "copper-agents-crashes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (
            Store::new(Paths::new(dir.join("data"), dir.join("config"))),
            dir,
        )
    }

    #[test]
    fn empty_listing_and_index_errors() {
        let (store, dir) = store();
        let out: Value = serde_json::from_str(&answer(&store, &Map::new()).unwrap()).unwrap();
        assert_eq!(out["count"], 0);
        assert!(std::path::Path::new(out["folder"].as_str().unwrap()).ends_with("crashes"));
        let mut args = Map::new();
        args.insert("index".into(), json!(1));
        assert_eq!(answer(&store, &args).unwrap_err(), "No crashes recorded.");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn newest_first_with_headlines() {
        let (store, dir) = store();
        let index: CrashIndex = serde_json::from_value(json!({
            "entries": [
                {"id": "a", "kind": "fault", "time": "2026-10-01T10:00:00Z", "source": "web-process", "reason": "the web process crashed"},
                {"id": "b", "kind": "report", "time": "2026-10-02T10:00:00Z", "exception": "EXC_BAD_ACCESS", "signal": "SIGSEGV", "file": "b.ips",
                 "frames": [{"image": "libsystem", "offset": 1}, {"image": "Copper", "symbol": "Tab.go", "offset": 12}]}
            ]
        }))
        .unwrap();
        store.save(&index).unwrap();
        let out: Value = serde_json::from_str(&answer(&store, &Map::new()).unwrap()).unwrap();
        assert_eq!(out["count"], 2);
        assert_eq!(out["crashes"][0]["id"], "b");
        assert_eq!(out["crashes"][0]["what"], "EXC_BAD_ACCESS (SIGSEGV)");
        assert_eq!(out["crashes"][0]["topFrame"], "Tab.go + 12");
        assert!(
            std::path::Path::new(out["crashes"][0]["file"].as_str().unwrap())
                .ends_with(std::path::Path::new("crashes").join("b.ips"))
        );
        assert_eq!(out["crashes"][1]["what"], "the web process crashed");
        let mut args = Map::new();
        args.insert("index".into(), json!(2));
        let one: Value = serde_json::from_str(&answer(&store, &args).unwrap()).unwrap();
        assert_eq!(one["n"], 2);
        assert_eq!(one["source"], "web-process");
        args.insert("index".into(), json!(3));
        assert_eq!(answer(&store, &args).unwrap_err(), "index is 1–2");
        let _ = std::fs::remove_dir_all(dir);
    }
}
