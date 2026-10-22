//! The brief: what an agent is told to fix findings. One builder writes it for
//! every hand-over: the prompt of an agent run, and the text a
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

const LOCALIZED: &str = "The code above is what Lighthouse knows this fix needs: the code of the \
findings, the signatures of what it uses and, for scope `files`, every place that refers to what \
you may change. Start from it, and open further files only when you cannot finish without them. \
Open the specific file or symbol you need, not a listing or search of the whole directory.";

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

/// The code of one extent a finding is in, as the agent is given it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub path: String,
    pub language: String,
    /// The first line of `text`, from 1.
    pub first: u32,
    /// The lines of the extent.
    pub text: String,
}

/// What the engine knows from the code model that the fix needs, so that the
/// agent does not have to look for it: the code of the findings and, as far as
/// a budget allows, what surrounds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Localized {
    /// The code each finding is in, once, in the order of the lines.
    pub code: Vec<Code>,
    /// The declarations, with the first line of their doc, of the symbols of
    /// the project the code uses.
    pub uses: Vec<String>,
    /// What the file imports.
    pub imports: Vec<String>,
    /// The places that refer to the symbols the fix may change (scope
    /// `files`), each with the symbol that holds it.
    pub sites: Vec<String>,
    /// The files that hold those places, which the fix may change.
    pub files: Vec<String>,
    /// How many items did not fit the budget and are not here.
    pub left_out: usize,
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
/// the agent places code for. `localized` is the code around them, when the
/// engine has it.
pub fn brief(targets: &[Target<'_>], tools: &[&str], localized: Option<&Localized>) -> String {
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
    if let Some(localized) = localized {
        write_localized(&mut text, localized);
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

/// Appends what the engine knows around the findings, then what the agent
/// should do with it.
fn write_localized(text: &mut String, localized: &Localized) {
    for code in &localized.code {
        let last = code.first as usize + code.text.lines().count().saturating_sub(1);
        let _ = write!(
            text,
            "\nCode ({} lines {}-{last}):\n",
            code.path, code.first
        );
        fenced(text, &code.language, &numbered(code));
    }
    let lists = [
        ("Signatures of what it uses:", &localized.uses),
        ("The file imports:", &localized.imports),
        (
            "Places that refer to what you may change:",
            &localized.sites,
        ),
        ("Files you may have to change:", &localized.files),
    ];
    for (title, items) in lists {
        if items.is_empty() {
            continue;
        }
        let _ = write!(text, "\n{title}\n");
        for item in items {
            let _ = writeln!(text, "- {item}");
        }
    }
    if localized.left_out > 0 {
        let _ = write!(
            text,
            "\n{} more items were left out to keep this short.\n",
            localized.left_out
        );
    }
    let _ = write!(text, "\n{LOCALIZED}\n");
}

/// The lines of `code`, each after its number.
fn numbered(code: &Code) -> String {
    code.text
        .lines()
        .zip(code.first..)
        .map(|(line, number)| format!("{number:>4}  {line}\n"))
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
