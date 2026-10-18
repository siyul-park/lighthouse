//! The brief: what an agent is told to fix findings. One builder writes it for
//! every hand-over: the prompt of an agent that `edit` runs, and the text a
//! review task carries for whoever drives the session. The same inputs always
//! write the same text, so that a run can be kept under its digest.

use std::fmt::Write;

use lighthouse_model::Safety;
use lighthouse_plugin::FixDecision;
use lighthouse_spec::{Example, ExampleKind, FixScope};
use serde_json::Value;

use super::{FixPlan, FixedBy};
use crate::Needs;

/// How many examples of a decision the brief shows at most.
pub const MAX_EXAMPLES: usize = 2;

const SYSTEM: &str = "You are fixing findings of design rules in the files of this directory. \
Change only what the findings need, keep behaviour and style, stay within the scope of each \
finding, and do not run commands that change anything outside this directory.
Scope `symbol`: change only the symbol the finding is in (the whole file when it is short and no \
symbol holds it) and touch no other file. Scope `files`: change any file of this directory the \
fix needs, such as the callers of a signature you change.";

const TOOLS: &str = "The project's tools fix these after you; follow them when placing code:";

const CHECKED: &str = "Lighthouse checks the result by analyzing the files again: each finding \
must be gone, and nothing new of warning or error severity may appear in the files you change or \
in the files that use a symbol you change.";

const CLOSING: &str =
    "When done, reply with one line per finding: its number and `fixed` or `left` with a reason.";

/// A finding to fix, and what its decision tells.
pub struct Target<'a> {
    pub decision: &'a FixDecision,
    /// What the decision adds for its fix, such as how it is usually done.
    pub instruction: Option<&'a str>,
    /// Examples of the decision that show a fix, as [`examples_for`] chose them.
    pub examples: Vec<Example>,
    /// How far the change may go.
    pub scope: FixScope,
    pub message: &'a str,
    pub evidence: &'a Value,
    /// `path:line:col`.
    pub place: String,
}

/// The decisions whose safe fixes the project applies to what an agent
/// leaves in files of `language` (of every language when `None`), in the
/// order of their ids: the tools the agent places code for.
pub fn safe_fixes<'p>(plan: &'p FixPlan, language: Option<&str>) -> Vec<&'p str> {
    plan.bindings()
        .filter(|(_, b)| b.cap == Safety::Safe && matches!(b.fixer, FixedBy::Fixer(_)))
        .filter(|(_, b)| b.needs != Needs::Unserved)
        .filter(|(_, b)| language.is_none_or(|l| b.languages.iter().any(|x| x == l)))
        .map(|(rule, _)| rule)
        .collect()
}

/// The brief for `targets`, the findings of one file in the order of their
/// lines. The examples of a decision come with its first finding. `tools` are
/// the decisions whose safe fixes the project applies to the result, which
/// the agent places code for.
pub fn brief(targets: &[Target<'_>], tools: &[&str]) -> String {
    let mut text = format!("{SYSTEM}\n");
    for (n, target) in targets.iter().enumerate() {
        let first = !targets[..n]
            .iter()
            .any(|t| t.decision.id == target.decision.id);
        let _ = write!(text, "\nFinding {}\n", n + 1);
        finding(&mut text, target);
        if first {
            for (shown, example) in target.examples.iter().enumerate() {
                write_example(&mut text, shown + 1, example);
            }
        }
    }
    if !tools.is_empty() {
        let _ = write!(text, "\n{TOOLS} {}.\n", tools.join(", "));
    }
    let _ = write!(text, "\n{CHECKED}\n{CLOSING}\n");
    text
}

/// The examples of a decision that show a fix, at most [`MAX_EXAMPLES`], in
/// the order of the decision.
pub fn examples_for(examples: &[Example]) -> Vec<Example> {
    examples
        .iter()
        .filter(|e| e.kind == ExampleKind::Invalid && !e.fixed.is_empty())
        .take(MAX_EXAMPLES)
        .cloned()
        .collect()
}

fn finding(text: &mut String, target: &Target<'_>) {
    let decision = target.decision;
    let _ = writeln!(text, "Decision: {}", decision.id);
    let _ = writeln!(text, "Requirement: {}", decision.requirement);
    if !decision.context.is_empty() {
        let _ = writeln!(text, "Context: {}", decision.context);
    }
    if let Some(instruction) = target.instruction {
        let _ = writeln!(text, "Instruction: {instruction}");
    }
    let _ = writeln!(text, "Message: {}", target.message);
    if *target.evidence != Value::Null {
        let _ = writeln!(text, "Evidence: {}", target.evidence);
    }
    let _ = writeln!(text, "Where: {}", target.place);
    let _ = writeln!(text, "Scope: {}", target.scope);
}

/// Appends the fixes `example` shows, as numbered before and after pairs.
fn write_example(text: &mut String, number: usize, example: &Example) {
    for fixed in &example.fixed {
        let before = example
            .files
            .iter()
            .find(|file| file.path == fixed.path)
            .map_or("", |file| file.text());
        let _ = write!(text, "\nExample {number} ({}), before:\n", fixed.path);
        fenced(text, &example.language, before);
        let _ = write!(text, "\nExample {number} ({}), after:\n", fixed.path);
        fenced(text, &example.language, fixed.text());
    }
}

/// Appends `code` in a fenced block: the fence is longer than any run of
/// backticks in the code, so the block ends where it should.
fn fenced(text: &mut String, language: &str, code: &str) {
    let longest = code
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest.max(2) + 1);
    let _ = write!(text, "{fence}{language}\n{code}");
    if !code.ends_with('\n') {
        text.push('\n');
    }
    let _ = writeln!(text, "{fence}");
}
