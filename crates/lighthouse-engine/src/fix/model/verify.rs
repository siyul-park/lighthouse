//! The verdict on a fix by a model. A fix of a rule is a derivation from the
//! rule; a model's rewrite is a guess, so it is kept only if the analysis of
//! the rewritten file agrees: the finding is gone, nothing of error or warn
//! severity is new in the file, and the file is still fully analyzed.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Fingerprint, Severity};

use super::Class;
use crate::{
    Run,
    fix::{FixedBy, propose::Proposal},
};

/// Why fixes by a model are not kept.
#[derive(Debug, Default)]
pub(in crate::fix) struct Verdict {
    /// Why the fix of each finding is not kept.
    pub(in crate::fix) own: BTreeMap<Fingerprint, (Class, String)>,
    /// Why each file is rolled back.
    pub(in crate::fix) files: BTreeMap<PathBuf, String>,
}

/// Judges the fixes by a model among `kept` by the check of the state `after`
/// them, which `before` them held the findings of.
pub(in crate::fix) fn judge(kept: &[Proposal], before: &Run, after: &Run) -> Verdict {
    let known: BTreeSet<&Fingerprint> = before.diagnostics.iter().map(|d| &d.fingerprint).collect();
    let mut verdict = Verdict::default();
    for proposal in kept.iter().filter(|p| p.fixer == FixedBy::Model) {
        let Some((class, reason)) = flaw(proposal, after, &known) else {
            continue;
        };
        for file in &proposal.files {
            verdict
                .files
                .entry(file.clone())
                .or_insert_with(|| reason.clone());
        }
        verdict
            .own
            .insert(proposal.fingerprint.clone(), (class, reason));
    }
    verdict
}

/// What is wrong with the fix after the check `after`, if anything. `known`
/// holds the fingerprints of the findings before it.
fn flaw(
    proposal: &Proposal,
    after: &Run,
    known: &BTreeSet<&Fingerprint>,
) -> Option<(Class, String)> {
    let in_files = || {
        after
            .diagnostics
            .iter()
            .filter(|d| proposal.files.contains(&d.file) && !known.contains(&d.fingerprint))
    };
    // A rewrite that keeps the problem can change the text a fingerprint
    // hashes, so a new finding of the rule in the file is the old one.
    if after
        .diagnostics
        .iter()
        .any(|d| d.fingerprint == proposal.fingerprint)
        || in_files().any(|d| d.rule_id == proposal.rule_id && d.file == proposal.file)
    {
        return Some((Class::Unresolved, "did not resolve the finding".to_owned()));
    }
    if let Some(new) = in_files().find(|d| matches!(d.severity, Severity::Error | Severity::Warn)) {
        return Some((
            Class::Introduced,
            format!("introduced {} at line {}", new.rule_id, new.span.start.line),
        ));
    }
    after
        .incomplete
        .iter()
        .find(|gap| {
            gap.path
                .as_ref()
                .is_some_and(|p| proposal.files.contains(p))
        })
        .map(|gap| {
            (
                Class::Incomplete,
                format!("analysis incomplete: {}", gap.reason),
            )
        })
}
