//! What each step of a function is to the levels, and the external symbols
//! the steps call.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::ucm::{Model, Step, SymbolId};

use globset::GlobSet;

use super::{LevelOptions, Slot, groups::Groups};
use crate::builder::steps::resolved_calls;

/// The slots of every function that has steps, and the variable of each
/// external symbol by language and name (numbered after the groups).
pub(super) struct Slots {
    pub(super) of: BTreeMap<SymbolId, Vec<Slot>>,
    pub(super) externals: BTreeMap<(String, String), usize>,
}

/// Classifies the steps of every function of `groups`.
pub(super) fn classify(
    project: &dyn Model,
    groups: &Groups,
    options: &LevelOptions,
    neutral: &GlobSet,
) -> Slots {
    let mut slots = Slots {
        of: BTreeMap::new(),
        externals: BTreeMap::new(),
    };
    for id in &groups.ids {
        let Some(summary) = project.function(id).filter(|f| !f.steps.is_empty()) else {
            continue;
        };
        let language = project
            .symbol(id)
            .and_then(|s| project.language_of(s))
            .unwrap_or("");
        let resolved = resolved_calls(project, id);
        let own = groups.var(id);
        let found = summary
            .steps
            .iter()
            .map(|step| {
                if step.guard {
                    return Slot::Neutral;
                }
                let mut named = BTreeSet::new();
                for call in step
                    .calls
                    .iter()
                    .filter(|c| !c.name.is_empty() && !neutral.is_match(&c.name))
                {
                    let callee = resolved
                        .iter()
                        .find(|(site, _)| site.start == call.site.start)
                        .and_then(|(_, to)| groups.var(to));
                    named.insert(callee.unwrap_or_else(|| {
                        let next = groups.count + slots.externals.len();
                        *slots
                            .externals
                            .entry((language.to_owned(), call.name.clone()))
                            .or_insert(next)
                    }));
                }
                named.retain(|&v| Some(v) != own);
                slot(options, step, named)
            })
            .collect();
        slots.of.insert(id.clone(), found);
    }
    slots
}

fn slot(options: &LevelOptions, step: &Step, named: BTreeSet<usize>) -> Slot {
    let primitive = step.operators + step.indexing + step.literals;
    if !named.is_empty() {
        Slot::Named(named.into_iter().collect())
    } else if primitive >= options.min_ops.max(1) {
        Slot::Atom
    } else {
        Slot::Neutral
    }
}
