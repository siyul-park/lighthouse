//! The renderers of a run, one binary.

mod agent;
mod docs;
mod group;
mod render;

use lighthouse_model::{Diagnostic, Incomplete};
use lighthouse_report::{Briefing, Format, render, report};
use serde::Serialize;
use serde_json::Value;

/// `data` as the JSON object a frontend prints.
fn json_of<T: Serialize>(data: &T) -> Value {
    serde_json::to_value(data).expect("a report serializes")
}

/// [`render`] with nothing known beyond the diagnostics.
fn plain(format: Format, diagnostics: &[Diagnostic], incomplete: &[Incomplete]) -> String {
    render(format, diagnostics, incomplete, &Briefing::default())
}

/// The JSON report of a check on one line: what `check --format json` prints
/// before the frontend adds its summary.
fn agent_json(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let mut line = json_of(&report(diagnostics, incomplete, briefing)).to_string();
    line.push('\n');
    line
}

/// The CEL environment is installed before any test reads a decision.
#[ctor::ctor(unsafe)]
fn compose() {
    lighthouse_test_support::cel();
}
