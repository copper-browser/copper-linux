//! A script plus its JSON arguments, ready for the engine, and its typed result.

use std::fmt;
use std::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::script::{Frames, Injection, Kind, Script, World};

/// One invocation of a curated script with its arguments.
///
/// The arguments are a JSON object passed as the function's single parameter
/// — page text and agent input are never spliced into JavaScript source.
/// `T` is the shape of the result, decoded with [`Call::decode`].
pub struct Call<T = Value> {
    script: Script,
    args: Value,
    /// Only `browser_evaluate`: the agent's own function text.
    custom: Option<String>,
    _result: PhantomData<fn() -> T>,
}

impl<T> Clone for Call<T> {
    fn clone(&self) -> Self {
        Call {
            script: self.script,
            args: self.args.clone(),
            custom: self.custom.clone(),
            _result: PhantomData,
        }
    }
}

impl<T> fmt::Debug for Call<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("script", &self.script)
            .field("args", &self.args)
            .field("custom", &self.custom)
            .finish()
    }
}

/// A result that did not have the expected shape.
#[derive(Debug)]
pub struct DecodeError {
    pub script: Script,
    pub json: String,
    pub error: serde_json::Error,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} returned an unexpected value {}: {}",
            self.script.file(),
            self.json,
            self.error
        )
    }
}

impl std::error::Error for DecodeError {}

fn to_value(args: impl Serialize) -> Value {
    // Arguments are plain data built by this crate; serialisation can't fail.
    serde_json::to_value(args).unwrap_or(Value::Null)
}

/// JSON as a JavaScript expression. JSON is valid JavaScript (U+2028/U+2029
/// included since ES2019); they are escaped anyway for older parsers.
fn literal(value: &Value) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "null".into())
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

impl<T> Call<T> {
    /// A call of `script` with `args` (any serialisable object).
    pub fn new(script: Script, args: impl Serialize) -> Self {
        Call {
            script,
            args: to_value(args),
            custom: None,
            _result: PhantomData,
        }
    }

    pub(crate) fn custom(script: Script, args: impl Serialize, function: String) -> Self {
        Call {
            custom: Some(function),
            ..Call::new(script, args)
        }
    }

    pub fn script(&self) -> Script {
        self.script
    }

    pub fn args(&self) -> &Value {
        &self.args
    }

    /// The world to run in (`world_name` for WebKitGTK).
    pub fn world(&self) -> World {
        self.script.world()
    }

    /// Whether the result is a promise.
    pub fn is_async(&self) -> bool {
        self.script.is_async() || self.custom.is_some()
    }

    fn invocation(&self) -> String {
        let call = format!("({})({})", self.script.function(), literal(&self.args));
        match &self.custom {
            None => call,
            Some(function) if self.args.is_null() => format!("({function})()"),
            Some(function) => format!("({function})({call})"),
        }
    }

    fn installer(&self) -> Option<String> {
        self.script
            .requires()
            .map(|installer| format!("({})()", installer.function()))
    }

    /// A function body for `call_async_javascript_function` (WebKitGTK) /
    /// `callAsyncJavaScript` — the way to run every call: a returned promise
    /// is awaited. The required installer runs first, in the same turn.
    pub fn function_body(&self) -> String {
        match self.installer() {
            Some(installer) => format!("{installer};\nreturn {};", self.invocation()),
            None => format!("return {};", self.invocation()),
        }
    }

    /// One expression for `evaluate_javascript`. Fine for synchronous calls;
    /// an async call's value is a promise there, so prefer [`Call::function_body`].
    pub fn expression(&self) -> String {
        match self.installer() {
            Some(installer) => format!("({installer}, {})", self.invocation()),
            None => self.invocation(),
        }
    }
}

impl<T: DeserializeOwned> Call<T> {
    /// Decode the JSON of the result (`javascriptcore::Value::to_json`); `None`
    /// (an `undefined` result) decodes as JSON `null`.
    pub fn decode(&self, json: Option<&str>) -> Result<T, DecodeError> {
        let json = json.unwrap_or("null");
        serde_json::from_str(json).map_err(|error| DecodeError {
            script: self.script,
            json: json.chars().take(400).collect(),
            error,
        })
    }

    /// Decode an already-parsed value.
    pub fn decode_value(&self, value: Value) -> Result<T, DecodeError> {
        let json = value.to_string();
        serde_json::from_value(value).map_err(|error| DecodeError {
            script: self.script,
            json: json.chars().take(400).collect(),
            error,
        })
    }
}

/// A user script ready for `WebKitUserScript::new` / `new_for_world`.
#[derive(Clone, Debug)]
pub struct UserScript {
    pub script: Script,
    /// A complete statement: `(<function>)(<args>);`.
    pub source: String,
    pub injection: Injection,
    pub frames: Frames,
    pub world: World,
}

impl UserScript {
    /// The user script for `script` with `args`. Panics if `script` is not a
    /// [`Kind::UserScript`] — a programming error, not a runtime condition.
    pub fn new(script: Script, args: impl Serialize) -> Self {
        let Kind::UserScript(injection, frames) = script.kind() else {
            panic!("{} is not a user script", script.file());
        };
        UserScript {
            script,
            source: format!("({})({});", script.function(), literal(&to_value(args))),
            injection,
            frames,
            world: script.world(),
        }
    }
}
