//! Page JavaScript ported from macOS Copper, as `include_str!` assets plus typed results.
//!
//! Every curated asset under `js/` starts with `// from Sources/Search/<path>:<line> <symbol>`
//! (the Swift literal it came from, for diffing after Mac changes) and is one
//! JavaScript function expression. Swift spliced values into its scripts; here
//! the values travel as one JSON argument object, so page text and agent input
//! are never JavaScript source (the one exception is `browser_evaluate`, whose
//! input *is* source).
//!
//! ```
//! use copper_pagejs::{page, Script};
//!
//! let call = page::snapshot(None, true, 600);
//! assert_eq!(call.script(), Script::Snapshot);
//! // Run with WebKitGTK's call_async_javascript_function(body, None, world, None):
//! let body = call.function_body();
//! assert!(body.contains(r#""limit":600"#) && body.ends_with("});"));
//! // …then decode javascriptcore::Value::to_json(0):
//! let text: String = call.decode(Some("\"- button \\\"OK\\\" [ref=e1]\"")).unwrap();
//! assert_eq!(text, "- button \"OK\" [ref=e1]");
//! ```
//!
//! The script index (file, Swift origin, kind, world) is in the crate README
//! and in [`Script::ALL`].

mod call;
pub mod canvas;
pub mod curtain;
pub mod forms;
pub mod jev;
pub mod messages;
pub mod page;
pub mod probe;
pub mod render;
mod script;
pub mod tab;
pub mod trail;

pub use call::{Call, DecodeError, UserScript};
pub use script::{FRAME_WORLD, Frames, Injection, Kind, Script, World};

/// `(source, world)` for a script: the raw asset and where it runs.
pub fn source(script: Script) -> (&'static str, World) {
    (script.source(), script.world())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_has_a_swift_header_and_one_function() {
        for &script in Script::ALL {
            let origin = script.origin();
            assert!(
                origin.starts_with("Sources/Search/") && origin.contains(".swift:"),
                "{}: bad header {:?}",
                script.file(),
                script.source().lines().next()
            );
            let line: u32 = origin.rsplit(':').next().unwrap().parse().unwrap();
            assert!(line > 0);
            assert!(!script.symbol().is_empty(), "{}", script.file());
            let f = script.function();
            assert!(
                f.starts_with("function () {")
                    || f.starts_with("() =>")
                    || f.starts_with("({")
                    || f.starts_with("async ({"),
                "{}: not a function expression: {}",
                script.file(),
                &f[..f.len().min(40)]
            );
            assert!(
                !f.contains("/*INTERP"),
                "{}: Swift interpolation left in",
                script.file()
            );
        }
    }

    #[test]
    fn every_js_file_is_in_the_table() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/js");
        let mut on_disk = Vec::new();
        for dir in std::fs::read_dir(root).unwrap() {
            let dir = dir.unwrap().path();
            if !dir.is_dir() || dir.ends_with("extracted") {
                continue;
            }
            for file in std::fs::read_dir(&dir).unwrap() {
                let file = file.unwrap().path();
                if file.extension().is_some_and(|e| e == "js") {
                    on_disk.push(
                        file.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        on_disk.sort();
        let mut listed: Vec<String> = Script::ALL.iter().map(|s| s.file().to_string()).collect();
        listed.sort();
        assert_eq!(on_disk, listed);
    }

    #[test]
    fn readme_index_lists_every_script() {
        let readme = include_str!("../README.md");
        for &script in Script::ALL {
            assert!(
                readme.contains(&format!("`{}`", script.file())),
                "README index lacks {}",
                script.file()
            );
        }
    }

    #[test]
    fn calls_carry_installers_and_args() {
        let call = page::click(&page::Locator::Ref("e3".into()), false, "left");
        let body = call.function_body();
        assert!(body.starts_with("(function () {\n  if (window.__copper) return true;"));
        assert!(body.contains(r#""target":"ref:e3""#) && body.contains(r#""double":false"#));
        assert!(body.contains("\nreturn (({ target, double, button }) => window.__copper.click("));
        let expr = call.expression();
        assert!(expr.starts_with("((function () {") && expr.ends_with(")"));
        // No installer: a bare invocation.
        let body = forms::has_password().function_body();
        assert!(body.starts_with("return (() => !!(window.__officeForms"));
        // Hostile text stays data.
        let call = page::set_value(&page::Locator::Css("#q".into()), "\");alert(1);//\u{2028}");
        assert!(
            call.function_body()
                .contains(r#""text":"\");alert(1);//\u2028""#)
        );
    }

    #[test]
    fn evaluate_splices_only_the_agents_function() {
        let call = page::evaluate("() => document.title", None);
        assert!(
            call.function_body()
                .ends_with("return (() => document.title)();")
        );
        let call = page::evaluate("(el) => el.id", Some("e2"));
        assert!(call.function_body().ends_with(
            "return ((el) => el.id)((({ target }) => window.__copper.find(target))({\"target\":\"ref:e2\"}));"
        ));
    }

    #[test]
    fn user_scripts_are_statements() {
        let us = forms::relay();
        assert_eq!(us.injection, Injection::DocumentEnd);
        assert_eq!(us.frames, Frames::MainFrame);
        assert!(us.source.starts_with("(function () {") && us.source.ends_with("})({});"));
        let us = canvas::frame_reporter();
        assert_eq!(us.world, World::Isolated("CopperCanvasFrames"));
        assert_eq!(us.world.name(), Some("CopperCanvasFrames"));
        let us = curtain::style("#ad { display: none !important; }");
        assert!(
            us.source
                .ends_with(r##"({"css":"#ad { display: none !important; }"});"##)
        );
    }

    #[test]
    #[should_panic(expected = "not a user script")]
    fn a_call_is_not_a_user_script() {
        UserScript::new(Script::Snapshot, serde_json::json!({}));
    }

    #[test]
    fn decoding() {
        let rect = page::rect(&page::Locator::Ref("e1".into()));
        let found = rect
            .decode(Some(r#"{"x":10,"y":20,"width":30,"height":40}"#))
            .unwrap();
        let r = found.into_result(&page::Locator::Ref("e1".into())).unwrap();
        assert_eq!(r.center(), (25.0, 40.0));
        assert_eq!(r.scaled(2.0).width, 60.0);
        let missing = rect
            .decode(Some(r#"{"error":"no element for ref:e1"}"#))
            .unwrap();
        assert_eq!(
            missing
                .into_result(&page::Locator::Ref("e1".into()))
                .unwrap_err(),
            "ref:e1: no element for ref:e1. Take a fresh browser_snapshot — refs change when the page does."
        );
        assert!(forms::fill_login("a", "b").decode(None).unwrap().is_none());
        assert!(page::snapshot(None, false, 1).decode(Some("3")).is_err());
        let check = jev::check(4).decode(Some(r#"["k", null]"#)).unwrap();
        assert_eq!(check, ("k".to_string(), None));
    }

    #[test]
    fn source_pairs() {
        let (src, world) = source(Script::PageHelper);
        assert!(src.starts_with("// from Sources/Search/Fork/MCP/Page.swift:138 Page.helper"));
        assert_eq!(world, World::Page);
        assert_eq!(Script::PageHelper.kind(), Kind::Installer);
        assert!(Script::PerfProbe.is_async());
    }
}
