mod agent;
mod evidence;
mod expected;
mod fix;
mod group;
mod sarif;

use std::fmt::Write;

use lighthouse_model::{Diagnostic, Incomplete, Severity};

pub use agent::{AgentReport, Briefing, Detail, agent_report, shown};
pub use fix::{FixEdit, FixFile, ProposedFix};
pub use group::{Entry, GroupOptions, Grouped};

/// Output format of [`render`]. The JSON report of a check is
/// [`agent_report`]; it is not rendered here.
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
/// notification per entry.
pub fn render(format: Format, diagnostics: &[Diagnostic], incomplete: &[Incomplete]) -> String {
    render_with(format, diagnostics, incomplete, &Briefing::default())
}

/// Like [`render`], with the catalog, subject facts and suppressed count the
/// agent formats draw on. The agent formats always end with a summary, a
/// clean run included, so that silence cannot be mistaken for success; the
/// other formats ignore the briefing.
pub fn render_with(
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
        Format::Agent => agent::text(diagnostics, incomplete, briefing),
    }
}

fn text(diagnostics: &[Diagnostic], incomplete: &[Incomplete], limit: Option<usize>) -> String {
    let mut out = String::new();
    let (shown, omitted) = agent::select(diagnostics, limit);
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
        match &item.path {
            Some(path) => {
                let _ = writeln!(out, "{}: incomplete: {}", path.display(), item.reason);
            }
            None => {
                let _ = writeln!(out, "incomplete: {}", item.reason);
            }
        }
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
