mod descriptor;
pub mod docs;
mod expected;
mod group;
mod report;
mod sarif;
mod text;

use std::fmt::Write;

use lighthouse_model::{Diagnostic, Incomplete, Severity};

pub use descriptor::{Aim, Apply, CheckBy, CheckInfo, Descriptor, FixBy, FixInfo, ToolCall};
pub use group::{Entry, GroupOptions, Grouped, Groups, Judgments, Omitted};
pub use report::{Briefing, Compact, Counts, Detail, Full, Gap, Report, Status, report};

/// Output format of [`render`]. The JSON report of a check is [`report`]; it
/// is not rendered here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Sarif,
    /// Findings grouped by decision for a coding agent: what a decision says
    /// once, a line per finding. One block per finding with
    /// [`Detail::Full`].
    Agent,
}

/// Renders diagnostics in the given order, then what the analysis could not
/// cover; the output ends with a newline unless it is empty. Text output ends
/// with a `summary:` line of counts whenever there is anything to report. An
/// incomplete analysis is stated in every format: text prints `incomplete`
/// lines and SARIF marks its invocation as unsuccessful with a tool
/// notification per entry. The briefing carries the catalog, subject facts and
/// suppressed count the agent formats draw on; they always end with a summary,
/// a clean run included, so that silence cannot be mistaken for success. The
/// other formats ignore what does not concern them.
pub fn render(
    format: Format,
    diagnostics: &[Diagnostic],
    incomplete: &[Incomplete],
    briefing: &Briefing,
) -> String {
    match format {
        Format::Text => text(diagnostics, incomplete, briefing.limit),
        Format::Sarif => sarif::render(
            diagnostics,
            incomplete,
            briefing.catalog,
            briefing.fixes,
            briefing.suppressions.unwrap_or_default(),
            briefing.sources,
        ),
        Format::Agent => report::text(diagnostics, incomplete, briefing),
    }
}

fn text(diagnostics: &[Diagnostic], incomplete: &[Incomplete], limit: Option<usize>) -> String {
    let mut out = String::new();
    let (shown, omitted) = report::select(diagnostics, limit);
    for d in shown {
        let _ = writeln!(
            out,
            "{}:{}:{}: {} {}: {}",
            d.file.display(),
            d.span.start.line,
            d.span.start.col,
            d.severity,
            d.rule_id,
            d.message,
        );
    }
    if omitted > 0 {
        let _ = writeln!(out, "... {omitted} more not shown (raise --limit)");
    }
    for item in incomplete {
        let _ = writeln!(out, "{}", report::incomplete_line(item));
    }
    if !diagnostics.is_empty() || !incomplete.is_empty() {
        let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
        let _ = writeln!(
            out,
            "summary: {} error, {} warn, {} info, {} incomplete",
            count(Severity::Error),
            count(Severity::Warn),
            count(Severity::Info),
            incomplete.len()
        );
    }
    out
}
