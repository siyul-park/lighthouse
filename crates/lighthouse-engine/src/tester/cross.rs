//! The cross-check of examples: what an example shows of one decision is held
//! to the others. A `valid` example, and the `fixed` form of an `invalid` one,
//! must not break another decision that applies to its language; a fix must not
//! introduce a finding of another decision on the text it edits. A finding
//! that cannot be avoided is declared on the example (`conflicts`), so it
//! stays tested.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Range,
    path::{Path, PathBuf},
};

use lighthouse_model::ExampleScope;
use lighthouse_spec::{Decision, Example};
use similar::TextDiff;

use super::{RuleTester, absent, rules_config};
use crate::{Engine, ModelOptions};

/// A finding of a decision other than the one the example is of.
pub(super) struct Hit {
    rule: String,
    file: PathBuf,
    line: u32,
    message: String,
    /// Whether the fix of the example introduced it, on the text it edited.
    introduced: bool,
}

impl RuleTester<'_> {
    /// The decisions that also judge the files of `example`: the enforced
    /// decisions with a program that apply to its language, except `decision`.
    /// A decision that only asks a model has no findings here, a command
    /// whose program is not installed cannot run, and one that leaves out
    /// examples (`scope.examples: exclude`) does not judge code that is shown.
    pub(super) fn others(&self, decision: &Decision, example: &Example) -> Vec<&Decision> {
        self.catalog
            .decisions()
            .filter(|d| {
                d.id() != decision.id()
                    && d.enforced()
                    && d.automated()
                    && d.severity.is_some()
                    && d.applies_to(&example.language)
                    && d.scope.examples != ExampleScope::Exclude
                    && !model_only(d)
                    && absent(d, example).is_none()
            })
            .collect()
    }

    /// An engine over the files in `dir` that checks `others` and nothing else.
    pub(super) fn crossing(&self, others: &[&Decision], dir: &Path) -> Result<Engine, String> {
        let registry = (self.registry)();
        let rules: Vec<(&Decision, Option<&Example>)> = others.iter().map(|d| (*d, None)).collect();
        let config = rules_config(&registry, &rules, &self.sources)?;
        Ok(Engine::new(registry, config, self.catalog, dir)
            .map_err(|e| e.to_string())?
            .with_trust(self.trusted)
            .with_models(ModelOptions::default()))
    }
}

/// The findings of `others` in the files of `example`, as `engine` sees them.
pub(super) fn cross(
    engine: &Engine,
    others: &[&Decision],
    example: &Example,
) -> Result<Vec<Hit>, String> {
    if others.is_empty() {
        return Ok(Vec::new());
    }
    let only: Vec<String> = others.iter().map(|d| d.id().to_owned()).collect();
    let run = engine.check(&[], &only).map_err(|e| e.to_string())?;
    let own: BTreeSet<&Path> = example.files.iter().map(|f| Path::new(&f.path)).collect();
    Ok(run
        .diagnostics
        .iter()
        .filter(|d| own.contains(d.file.as_path()))
        .map(|d| Hit {
            rule: d.rule_id.clone(),
            file: d.file.clone(),
            line: d.span.start.line,
            message: d.message.clone(),
            introduced: false,
        })
        .collect())
}

/// The findings of `after` that the fix of `example` introduced: those that
/// `before` did not have, on the lines it edited. They are marked; the others
/// are findings the example had already. `dir` holds the fixed files.
pub(super) fn introduced(
    example: &Example,
    dir: &Path,
    before: &[Hit],
    mut after: Vec<Hit>,
) -> Vec<Hit> {
    let known: BTreeSet<(&str, &str)> = before
        .iter()
        .map(|h| (h.rule.as_str(), h.message.as_str()))
        .collect();
    let edited: BTreeMap<&Path, Vec<Range<u32>>> = example
        .files
        .iter()
        .map(|f| {
            let now = fs::read_to_string(dir.join(&f.path)).unwrap_or_default();
            (Path::new(&f.path), edited_lines(f.text(), &now))
        })
        .collect();
    for hit in &mut after {
        hit.introduced = !known.contains(&(hit.rule.as_str(), hit.message.as_str()))
            && edited
                .get(hit.file.as_path())
                .is_some_and(|spans| spans.iter().any(|s| s.contains(&hit.line)));
    }
    after
}

/// Holds the `hits` to what `example` declares: a finding of a decision it does
/// not list is a failure, and so is a listed decision that has none. The lines
/// that report what is declared and still found.
pub(super) fn judge(example: &Example, hits: &[Hit]) -> Result<Vec<String>, String> {
    let mut found: BTreeMap<&str, Vec<&Hit>> = BTreeMap::new();
    for hit in hits {
        found.entry(&hit.rule).or_default().push(hit);
    }
    let declared: BTreeSet<&str> = example.conflicts.iter().map(String::as_str).collect();
    let mut wrong = Vec::new();
    let mut known = Vec::new();
    for (rule, group) in &found {
        let first = group[0];
        let how = if group.iter().any(|h| h.introduced) {
            "its fix introduces"
        } else {
            "it also breaks"
        };
        let places: Vec<String> = group
            .iter()
            .map(|h| format!("{}:{}", h.file.display(), h.line))
            .collect();
        let line = format!("{how} {rule} at {} ({})", places.join(", "), first.message);
        if declared.contains(rule) {
            known.push(line);
        } else {
            wrong.push(line);
        }
    }
    for rule in declared.iter().filter(|r| !found.contains_key(*r)) {
        wrong.push(format!(
            "declares a conflict with {rule}, which no longer fires"
        ));
    }
    if wrong.is_empty() {
        Ok(known)
    } else {
        Err(wrong.join("; "))
    }
}

/// Whether the decision's check never reports without a model.
pub(super) fn model_only(decision: &Decision) -> bool {
    use lighthouse_spec::CheckKind;
    decision.check.as_ref().is_some_and(|check| {
        matches!(
            &check.kind,
            CheckKind::Cel(cel) if cel.abstain.is_some() && cel.condition.trim() == "false"
        )
    })
}

/// The 1-based lines of `after` that differ from `before`; a removal counts the
/// lines around the place it was made.
fn edited_lines(before: &str, after: &str) -> Vec<Range<u32>> {
    let diff = TextDiff::from_lines(before, after);
    diff.ops()
        .iter()
        .filter(|op| !matches!(op, similar::DiffOp::Equal { .. }))
        .map(|op| {
            let range = op.new_range();
            let (start, end) = (range.start as u32, range.end as u32);
            if start == end {
                start.max(1)..start + 2
            } else {
                start + 1..end + 1
            }
        })
        .collect()
}
