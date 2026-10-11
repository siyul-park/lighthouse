//! The renderers of a run, one binary.

mod agent;
mod docs;
mod group;
mod render;

use lighthouse_model::{Diagnostic, Incomplete};
use lighthouse_report::{Briefing, agent_report};

/// The JSON report of a check on one line: what `check --format json` prints
/// before the frontend adds its summary.
fn agent_json(
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    let report = agent_report(diagnostics, incomplete, briefing);
    let mut line = serde_json::to_string(&report.fields).expect("a report serializes");
    line.push('\n');
    line
}
