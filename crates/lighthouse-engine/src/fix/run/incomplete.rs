//! What an incomplete analysis refuses. A fix is refused only when the
//! analysis it rests on has a gap: a file it edits, or a file its rule reads
//! by reach (`local`: the file itself, `neighbors`: the files around it,
//! `global`: the project), lies in the incomplete scope. A gap with a path
//! covers that file; a gap of a provider without one covers the files of that
//! provider; a gap of neither covers everything.

use std::path::Path;

use lighthouse_model::{Incomplete, Reach};

use super::super::{model::reach, propose::Proposal};
use crate::{Engine, Run};

/// Why the analysis of `outcome` is too incomplete for `proposal`, if it is.
pub(super) fn refusal(engine: &Engine, outcome: &Run, proposal: &Proposal) -> Option<String> {
    if outcome.incomplete.is_empty() {
        return None;
    }
    let mut edited = proposal.files.clone();
    edited.push(proposal.file.clone());
    let read: Vec<_> = match reach_of(engine, outcome, proposal) {
        Reach::Local => edited.clone(),
        Reach::Neighbors => reach(&edited, outcome, outcome),
        Reach::Global => return outcome.incomplete.first().map(describe),
    };
    outcome
        .incomplete
        .iter()
        .find(|gap| read.iter().any(|file| covers(engine, outcome, gap, file)))
        .map(describe)
}

/// How far the rule of `proposal` reads for the file of the proposal.
fn reach_of(engine: &Engine, outcome: &Run, proposal: &Proposal) -> Reach {
    let Some(rule) = engine.registry.rule(&proposal.rule_id) else {
        return Reach::Global;
    };
    let language = outcome
        .project
        .file(&proposal.file)
        .map_or("", |file| file.lang.as_str());
    rule.reach(language)
}

fn covers(engine: &Engine, outcome: &Run, gap: &Incomplete, file: &Path) -> bool {
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
