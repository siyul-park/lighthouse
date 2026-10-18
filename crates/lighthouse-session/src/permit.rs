//! The permission to fix by a model: the decisions `[fix] decisions` lists
//! that an agent can be given.

use std::collections::BTreeSet;

use lighthouse_spec::{Catalog, Config, Decision, FixKind};

use crate::{Result, Selector};

/// Which decisions a model may fix.
pub(crate) struct Permitted {
    /// Decisions with a check and no fix of their own, or a fix by a model,
    /// that the project lists.
    pub(crate) ids: BTreeSet<String>,
    /// What is wrong with the list, one line each.
    pub(crate) warnings: Vec<String>,
}

/// The decisions of `catalog` that `config` lists for a model fix. A pattern
/// that matches no decision `active` accepts is a warning naming it.
pub(crate) fn permitted(
    config: &Config,
    catalog: &Catalog,
    active: &dyn Fn(&str) -> bool,
) -> Result<Permitted> {
    let selector = Selector::new(config.fix_decisions())?;
    let known = catalog
        .decisions()
        .map(Decision::id)
        .filter(|id| active(id));
    let (listed, unmatched) = selector.resolve(known);
    let fixable = |id: &String| {
        catalog.decision(id).is_some_and(|d| {
            d.check.is_some()
                && d.fix
                    .as_ref()
                    .is_none_or(|fix| matches!(fix.kind, FixKind::Model { .. }))
        })
    };
    Ok(Permitted {
        ids: listed.into_iter().filter(fixable).collect(),
        warnings: unmatched
            .into_iter()
            .map(|pattern| {
                format!("`[fix] decisions` lists `{pattern}`, which matches no active decision")
            })
            .collect(),
    })
}
