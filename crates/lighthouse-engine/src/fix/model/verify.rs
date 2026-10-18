//! The verdict on a fix by a model. A fix of a rule is a derivation from the
//! rule; a model's rewrite is a guess, so it is kept only if the analysis of
//! the rewritten file agrees: the finding is gone, nothing of error or warn
//! severity is new in the file, and the file is still fully analyzed. An edit
//! of files by an agent is kept when at least one of the findings it was asked
//! about is gone; those still there are declined, one by one. Its files are
//! judged together, with the files that use a symbol whose signature it
//! changed: the edit stands or falls whole.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_model::{Diagnostic, Fingerprint, Severity};

use super::{Class, reach::reach};
use crate::{
    Run,
    fix::{
        FixedBy,
        propose::{Aim, Proposal},
    },
};

/// The reason a finding is declined as still there.
const UNRESOLVED: &str = "did not resolve the finding";

/// Why fixes by a model are not kept.
#[derive(Debug, Default)]
pub(in crate::fix) struct Verdict {
    /// Why the fix of each finding is not kept.
    pub(in crate::fix) own: BTreeMap<Fingerprint, (Class, String)>,
    /// Why each file is rolled back.
    pub(in crate::fix) files: BTreeMap<PathBuf, String>,
}

/// Judges the fixes by a model among `kept` by the check of the state `after`
/// them, which `before` them held the findings of. `merged` are the proposals
/// that have the edits of a kept one and share its fate.
pub(in crate::fix) fn judge(
    kept: &[Proposal],
    merged: &[(Proposal, Fingerprint)],
    before: &Run,
    after: &Run,
) -> Verdict {
    let known: BTreeSet<&Fingerprint> = before.diagnostics.iter().map(|d| &d.fingerprint).collect();
    let mut verdict = Verdict::default();
    for proposal in kept.iter().filter(|p| p.fixer == FixedBy::Model) {
        match proposal.aim {
            Aim::Finding => {
                if let Some((class, reason)) = flaw(proposal, (before, after), &known) {
                    reject(&mut verdict, &[proposal], class, reason);
                }
            }
            Aim::File => {
                let sharing = merged
                    .iter()
                    .filter(|(_, into)| *into == proposal.fingerprint)
                    .map(|(p, _)| p);
                let group: Vec<&Proposal> = std::iter::once(proposal).chain(sharing).collect();
                judge_file(&mut verdict, &group, (before, after), &known);
            }
        }
    }
    verdict
}

/// Rolls back the files of `group` for `reason`, and declines each of its
/// findings with it.
fn reject(verdict: &mut Verdict, group: &[&Proposal], class: Class, reason: String) {
    for proposal in group {
        for file in &proposal.files {
            verdict
                .files
                .entry(file.clone())
                .or_insert_with(|| reason.clone());
        }
        verdict
            .own
            .insert(proposal.fingerprint.clone(), (class, reason.clone()));
    }
}

/// The verdict on the edit of files that is for every finding of `group`.
fn judge_file(
    verdict: &mut Verdict,
    group: &[&Proposal],
    (before, after): (&Run, &Run),
    known: &BTreeSet<&Fingerprint>,
) {
    let reached = reach(&group[0].files, before, after);
    if let Some((class, reason)) = regression(&reached, (before, after), known) {
        return reject(verdict, group, class, reason);
    }
    let left = unresolved(group, after, known);
    if left.len() == group.len() {
        return reject(verdict, group, Class::Unresolved, UNRESOLVED.to_owned());
    }
    for proposal in left {
        verdict.own.insert(
            proposal.fingerprint.clone(),
            (Class::Unresolved, UNRESOLVED.to_owned()),
        );
    }
}

/// The proposals of `group` whose finding is still there after the check
/// `after`. A rewrite can change the text a fingerprint hashes, so a new
/// finding of a rule in the file stands for one of the old ones of that rule.
fn unresolved<'p>(
    group: &[&'p Proposal],
    after: &Run,
    known: &BTreeSet<&Fingerprint>,
) -> Vec<&'p Proposal> {
    let present: BTreeSet<&Fingerprint> =
        after.diagnostics.iter().map(|d| &d.fingerprint).collect();
    let mut refound: BTreeMap<&str, usize> = BTreeMap::new();
    for new in new_in(&group[0].files, after, known) {
        *refound.entry(new.rule_id.as_str()).or_default() += 1;
    }
    group
        .iter()
        .filter(|p| {
            if present.contains(&p.fingerprint) {
                return true;
            }
            match refound.get_mut(p.rule_id.as_str()) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    true
                }
                _ => false,
            }
        })
        .copied()
        .collect()
}

/// What is wrong with the fix after the check `after`, if anything. `known`
/// holds the fingerprints of the findings before it.
fn flaw(
    proposal: &Proposal,
    (before, after): (&Run, &Run),
    known: &BTreeSet<&Fingerprint>,
) -> Option<(Class, String)> {
    // A rewrite that keeps the problem can change the text a fingerprint
    // hashes, so a new finding of the rule in the file is the old one.
    if after
        .diagnostics
        .iter()
        .any(|d| d.fingerprint == proposal.fingerprint)
        || new_in(&proposal.files, after, known)
            .any(|d| d.rule_id == proposal.rule_id && d.file == proposal.file)
    {
        return Some((Class::Unresolved, UNRESOLVED.to_owned()));
    }
    regression(&proposal.files, (before, after), known)
}

/// The findings of the check `after` in `files` that were not there before.
fn new_in<'a>(
    files: &'a [PathBuf],
    after: &'a Run,
    known: &'a BTreeSet<&Fingerprint>,
) -> impl Iterator<Item = &'a Diagnostic> {
    after
        .diagnostics
        .iter()
        .filter(move |d| files.contains(&d.file) && !known.contains(&d.fingerprint))
}

/// A decision with more findings of error or warn severity in a file of
/// `files` after the check `after` than `before` it, or a gap in their
/// analysis. Findings are counted, not matched by fingerprint: a fingerprint
/// hashes the symbol path, so renaming a symbol would make all of its old
/// findings look new.
fn regression(
    files: &[PathBuf],
    (before, after): (&Run, &Run),
    known: &BTreeSet<&Fingerprint>,
) -> Option<(Class, String)> {
    if let Some(new) = introduced(files, (before, after), known) {
        return Some((
            Class::Introduced,
            format!("introduced {} at line {}", new.rule_id, new.span.start.line),
        ));
    }
    after
        .incomplete
        .iter()
        .find(|gap| gap.path.as_ref().is_some_and(|p| files.contains(p)))
        .map(|gap| {
            (
                Class::Incomplete,
                format!("analysis incomplete: {}", gap.reason),
            )
        })
}

/// The finding, of a decision that has more of error or warn severity in a
/// file of `files` after the check than before it, counted per (file, rule)
/// like `run::rollback`'s `worse`, that is not one of those of before when one
/// of them is not; else the first of them.
fn introduced<'a>(
    files: &[PathBuf],
    (before, after): (&Run, &'a Run),
    known: &BTreeSet<&Fingerprint>,
) -> Option<&'a Diagnostic> {
    let mut held: BTreeMap<(&Path, &str), usize> = BTreeMap::new();
    for d in loud_in(files, before) {
        *held
            .entry((d.file.as_path(), d.rule_id.as_str()))
            .or_default() += 1;
    }
    let mut now: BTreeMap<(&Path, &str), Vec<&'a Diagnostic>> = BTreeMap::new();
    for d in loud_in(files, after) {
        now.entry((d.file.as_path(), d.rule_id.as_str()))
            .or_default()
            .push(d);
    }
    now.into_iter()
        .filter(|(key, found)| found.len() > held.get(key).copied().unwrap_or(0))
        .find_map(|(_, found)| {
            found
                .iter()
                .find(|d| !known.contains(&d.fingerprint))
                .or(found.first())
                .copied()
        })
}

/// The findings of error or warn severity of the check `run` in `files`.
fn loud_in<'r>(files: &[PathBuf], run: &'r Run) -> Vec<&'r Diagnostic> {
    run.diagnostics
        .iter()
        .filter(|d| files.contains(&d.file))
        .filter(|d| matches!(d.severity, Severity::Error | Severity::Warn))
        .collect()
}
