//! What the search finds among the order decisions in force for the files of a
//! project, told to the project's own decisions that are members of a
//! contradiction.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::Document;
use lighthouse_spec::Decision;
use serde_json::{Value, json};

use super::{Conflicts, Engine, Input};

/// What the search finds among the order decisions in force, told to the
/// project's own decisions that are members of it, by document.
pub(super) fn of(
    engine: &Engine,
    conflicts: &Conflicts,
    (inputs, documents): (&[Input], &[Document]),
    notices: &mut BTreeSet<String>,
) -> BTreeMap<String, Vec<Value>> {
    let mut entries: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let own = own_decisions(conflicts, documents);
    if own.is_empty() {
        return entries;
    }
    let catalog = &conflicts.catalog;
    let mut guard = engine.lock_graph();
    let graph = engine.graph_in(&mut guard, notices);
    for cell in conflicts.cells(engine, inputs, notices) {
        let found = conflicts.nogoods(engine, graph, &cell);
        let mut tell = |key: String, value: Value| {
            let known = entries.entry(key).or_default();
            if !known.contains(&value) {
                known.push(value);
            }
        };
        for nogood in &found.nogoods {
            for (key, value) in nogood.entries(catalog, &own) {
                tell(key, value);
            }
        }
        notices.extend(found.notices(&cell.0));
        if !found.incomplete.is_empty() {
            let members: Vec<&Decision> = cell
                .1
                .keys()
                .filter_map(|id| catalog.decision(id))
                .collect();
            for (key, value) in incomplete_entries(&members, &own, &found.incomplete) {
                tell(key, value);
            }
        }
    }
    entries
}

/// The entry of a cell whose search is incomplete, for each decision of the
/// project in it: nothing is claimed, and the reasons are told.
pub(super) fn incomplete_entries(
    members: &[&Decision],
    own: &BTreeSet<&str>,
    reasons: &[String],
) -> Vec<(String, Value)> {
    members
        .iter()
        .filter(|d| own.contains(d.key()))
        .map(|d| {
            let entry = json!({
                "with": [],
                "kind": "order",
                "basis": "incomplete",
                "witness": null,
                "resolved": false,
                "reasons": reasons,
            });
            (d.key().to_owned(), entry)
        })
        .collect()
}

/// The decisions of the catalog that the project's own documents are.
fn own_decisions<'c>(conflicts: &'c Conflicts, documents: &[Document]) -> BTreeSet<&'c str> {
    documents
        .iter()
        .filter(|d| d.kind == "Decision")
        .filter_map(|d| conflicts.catalog.decision(&d.name))
        .filter(|decision| (conflicts.orders)(decision))
        .map(Decision::key)
        .collect()
}
