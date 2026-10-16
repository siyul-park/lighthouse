//! The renderers of a run, one binary.

mod agent;
mod docs;
mod group;
mod render;

use lighthouse_model::{Diagnostic, Incomplete};
use lighthouse_report::{Briefing, Format, render, report};

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
    let fields = report(diagnostics, incomplete, briefing).fields;
    let mut line = serde_json::to_string(&fields).expect("a report serializes");
    line.push('\n');
    line
}
