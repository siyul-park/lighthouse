//! The contradictions among decisions that `decision test` and `spec validate`
//! report: the same search a run makes for a rule that reads
//! `decision.conflicts` ([`lighthouse_engine::Conflicts::search`]), over the
//! decisions of a catalog under test or over the cells of a project. There is
//! no other path to one.

use std::{collections::BTreeSet, sync::Arc};

use lighthouse_engine::{Conflicts, Contradicted, Gaps, Nogood};
use lighthouse_model::Options;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Config, Decision};

use crate::Session;

/// What a search found, as lines: a contradiction no decision settles is a
/// failure; one a decision settles by superseding the other is a note; what
/// could not be searched in full is told, never left out.
#[derive(Debug, Default)]
pub(crate) struct Searched {
    pub(crate) failures: Vec<String>,
    pub(crate) settled: Vec<String>,
    pub(crate) unsearched: Vec<String>,
}

impl Searched {
    /// What a decision test reports of the search: its failures, the
    /// contradictions a decision settles as conflicts, and what it did not
    /// search as incomplete.
    pub(crate) fn report(self, found: &mut crate::examples::Found) {
        found.failures.extend(
            self.failures
                .into_iter()
                .map(|line| format!("contradiction in {line}")),
        );
        found.conflicts.extend(self.settled);
        found.incomplete.extend(self.unsearched);
    }

    fn add(&mut self, language: &str, nogood: &Nogood<'_>, catalog: &Catalog) {
        self.add_contradicted(&Contradicted {
            language: language.to_owned(),
            names: nogood.names(),
            kind: nogood.kind,
            witness: nogood.witness.clone(),
            resolved: nogood.resolved(catalog),
            at_defaults: true,
        });
    }

    fn add_contradicted(&mut self, c: &Contradicted) {
        let shape = c.witness["shape"].as_str().unwrap_or("some declarations");
        let names = c.names.join(", ");
        let line = format!(
            "{}: on code shaped like {shape}, no order satisfies {names}",
            c.language
        );
        if c.resolved {
            self.settled.push(format!("{line}; one supersedes another"));
        } else {
            self.failures.push(line);
        }
    }
}

/// The contradictions among the order decisions of `catalog`, language by
/// language, at their default options, that involve a decision of
/// `under_test`: a catalog is tested as a whole, and a decision of a pack that
/// ships is held to the others where the pack is authored.
pub(crate) fn in_catalog(
    session: &Session,
    catalog: &Catalog,
    registry: Registry,
    (languages, under_test): (&BTreeSet<String>, &[&Decision]),
) -> Searched {
    let mut searched = Searched::default();
    let conflicts = Conflicts::new(Arc::new(catalog.clone()), session.orders());
    let registry = Arc::new(registry);
    let mut gaps = Gaps::default();
    for language in languages.iter().filter(|language| {
        under_test
            .iter()
            .any(|d| d.applies_to(language) && conflicts.orders(d))
    }) {
        let decisions: Vec<(&Decision, Options)> = catalog
            .decisions()
            .filter(|d| d.automated() && d.applies_to(language) && conflicts.orders(d))
            .map(|d| (d, Options::new()))
            .collect();
        if decisions.is_empty() {
            continue;
        }
        let found = conflicts.search_once(&registry, language, decisions);
        searched.unsearched.extend(found.lost_notice(language));
        gaps.add(language, &found);
        for nogood in &found.nogoods {
            let involved = nogood
                .members
                .iter()
                .any(|m| under_test.iter().any(|t| t.key() == m.key()));
            if involved {
                searched.add(language, nogood, catalog);
            }
        }
    }
    searched.unsearched.extend(gaps.notice());
    searched
}

/// The contradictions the options and overrides of a project make: among its
/// cells (the effective decisions for each language of its files), those that
/// the same decisions at their default options do not make. A contradiction
/// that holds by default belongs to the packs, and is gated where they are
/// authored.
pub(crate) fn in_project(
    session: &Session,
    registry: &Arc<Registry>,
    config: Config,
    path: &str,
) -> Searched {
    let mut searched = Searched::default();
    let conflicts = Conflicts::new(Arc::clone(&session.catalog), session.orders());
    let found = session
        .engine_with(
            Arc::clone(registry),
            config,
            &session.catalog,
            &session.root,
        )
        .map(|engine| engine.with_conflicts(conflicts))
        .map_err(|e| e.to_string())
        .and_then(|engine| engine.contradictions().map_err(|e| e.to_string()));
    match found {
        Ok(found) => {
            searched.unsearched.extend(found.notices);
            for contradicted in found.found.iter().filter(|c| !c.at_defaults) {
                searched.add_contradicted(contradicted);
            }
        }
        Err(why) => searched.unsearched.push(format!(
            "{path}: the search for contradictions was not done: {why}"
        )),
    }
    searched
}
