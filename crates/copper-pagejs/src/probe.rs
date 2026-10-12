//! `browser_perf_probe` (Fork/MCP/Probe.swift): sample the page for N seconds
//! and answer a Markdown report or the raw JSON.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::call::Call;
use crate::script::Script;

/// What the probe answers: Markdown (`format: "text"`) or the report object.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum PerfOutput {
    Text(String),
    Json(Value),
}

/// One heuristic finding; severity 0 means "looks fine".
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: f64,
    pub text: String,
}

/// The top of the JSON report; the sections stay as JSON (`frameLoop`,
/// `domChurn`, `animations`, `filters`, `canvases`, `timers`, `resources`, `page`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfReport {
    pub window_seconds: f64,
    pub heuristics: Vec<Finding>,
    #[serde(flatten)]
    pub sections: serde_json::Map<String, Value>,
}

/// The Mac's clamping of the tool's arguments (`Probe.run`).
pub fn clamp(
    seconds: Option<f64>,
    top: Option<f64>,
    json_format: bool,
) -> (f64, u32, &'static str) {
    let seconds = seconds.unwrap_or(3.0).clamp(1.0, 10.0);
    let top = (top.map(|t| t as i64).unwrap_or(8)).clamp(1, 100) as u32;
    (seconds, top, if json_format { "json" } else { "text" })
}

/// The probe. Callers time out after `seconds + 5` s, as the Mac does.
pub fn perf(seconds: f64, top: u32, format: &str) -> Call<PerfOutput> {
    Call::new(
        Script::PerfProbe,
        json!({ "probeSeconds": seconds, "probeTop": top, "probeFormat": format }),
    )
}
