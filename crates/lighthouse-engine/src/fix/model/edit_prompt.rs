//! The prompt that asks an agent to fix the findings of one file by editing
//! the project. Plain text, written by one builder: the same inputs always
//! write the same prompt, so that a run can be kept under its digest.

use std::fmt::Write;

use lighthouse_model::Diagnostic;
use lighthouse_plugin::FixDecision;
use lighthouse_spec::Example;
use serde_json::Value;

use super::prompt::write_example;

/// How many findings of one file an agent is given at most.
pub(in crate::fix) const MAX_FINDINGS: usize = 20;

/// How many examples of a decision the prompt shows at most.
pub(super) const MAX_EXAMPLES: usize = 2;

const SYSTEM: &str = "You are fixing findings of design rules in the files of this directory. \
Change only what the findings need, keep behaviour and style, touch no other file, \
and do not run commands that change anything outside this directory.";

const CLOSING: &str =
    "When done, reply with one line per finding: its number and `fixed` or `left` with a reason.";

/// A finding the agent is asked to fix, and what its decision tells.
pub(in crate::fix) struct Target<'a> {
    pub(in crate::fix) finding: &'a Diagnostic,
    pub(in crate::fix) decision: &'a FixDecision,
    /// What the decision adds for its fix, such as how it is usually done.
    pub(in crate::fix) instruction: Option<&'a str>,
    /// Examples of the decision that show a fix, as the prompt shows them.
    pub(in crate::fix) examples: Vec<Example>,
}

/// The prompt for `targets`, the findings of one file in the order of their
/// lines. The examples of a decision come with its first finding.
pub(super) fn prompt(targets: &[Target<'_>]) -> String {
    let mut text = format!("{SYSTEM}\n");
    for (n, target) in targets.iter().enumerate() {
        let decision = target.decision;
        let first = !targets[..n].iter().any(|t| t.decision.id == decision.id);
        let _ = write!(text, "\nFinding {}\n", n + 1);
        finding(&mut text, target);
        if first {
            for (shown, example) in target.examples.iter().enumerate() {
                write_example(&mut text, shown + 1, example);
            }
        }
    }
    let _ = write!(text, "\n{CLOSING}\n");
    text
}

fn finding(text: &mut String, target: &Target<'_>) {
    let (decision, finding) = (target.decision, target.finding);
    let _ = writeln!(text, "Decision: {}", decision.id);
    let _ = writeln!(text, "Requirement: {}", decision.requirement);
    if !decision.context.is_empty() {
        let _ = writeln!(text, "Context: {}", decision.context);
    }
    if let Some(instruction) = target.instruction {
        let _ = writeln!(text, "Instruction: {instruction}");
    }
    let _ = writeln!(text, "Message: {}", finding.message);
    if finding.evidence != Value::Null {
        let _ = writeln!(text, "Evidence: {}", finding.evidence);
    }
    let _ = writeln!(
        text,
        "Where: {}:{}:{}",
        finding.file.display(),
        finding.span.start.line,
        finding.span.start.col
    );
}
