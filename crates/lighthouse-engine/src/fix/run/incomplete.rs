//! What an incomplete analysis refuses. A fix is refused only when the
//! analysis it rests on has a gap: a file it edits, or a file the rule that
//! found its finding read, lies in the incomplete scope. A gap with a path
//! covers that file; a gap of a provider without one covers the files of that
//! provider; a gap of neither covers everything. A rule the graph holds no
//! reads of rests on the whole analysis.

use std::path::Path;

use lighthouse_model::{Incomplete, ucm::Model};

use super::{super::propose::Proposal, reads};
use crate::{Engine, Outcome};

/// Why the analysis of `outcome` is too incomplete for `proposal`, if it is.
pub(super) fn refusal(engine: &Engine, outcome: &Outcome, proposal: &Proposal) -> Option<String> {
    if outcome.incomplete.is_empty() {
        return None;
    }
    let Some(mut read) = reads::files(engine, (&proposal.rule_id, &proposal.file), None) else {
        return outcome.incomplete.first().map(describe);
    };
    read.extend(proposal.files.iter().cloned());
    outcome
        .incomplete
        .iter()
        .find(|gap| read.iter().any(|file| covers(engine, outcome, gap, file)))
        .map(describe)
}

fn covers(engine: &Engine, outcome: &Outcome, gap: &Incomplete, file: &Path) -> bool {
    if let Some(path) = &gap.path {
        return path == file;
    }
    let Some(provider) = &gap.provider else {
        return true;
    };
    outcome
        .project
        .file(file)
        .and_then(|f| engine.language(&f.lang))
        .is_some_and(|language| language.manifest().id == *provider)
}

fn describe(gap: &Incomplete) -> String {
    format!(
        "the analysis is incomplete ({}{}{}), so the fix was not made",
        gap.provider
            .as_ref()
            .map_or(String::new(), |p| format!("{p}: ")),
        gap.path
            .as_ref()
            .map_or(String::new(), |p| format!("{}: ", p.display())),
        gap.reason
    )
}
