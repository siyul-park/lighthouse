//! The conflicts the examples of a project's own decisions declare: candidates,
//! each shown by an example and proved by no search.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::Document;
use lighthouse_plugin::Documents;
use serde_json::{Value, json};

use super::{Conflicts, supersedes};

/// The conflicts the examples of the project's own decisions declare, by
/// document: those that no search proved (a proven one is the error decision's
/// to report) and, for one a decision settles by superseding the other, told
/// `resolved`. Each is `empirical`: an example showed it, nothing proved it.
pub(super) fn candidates(
    conflicts: &Conflicts,
    documents: &[Document],
    proven: &BTreeMap<String, Vec<Value>>,
) -> BTreeMap<String, Vec<Value>> {
    let catalog = &conflicts.catalog;
    let mut found = BTreeMap::new();
    for document in documents.iter().filter(|d| d.kind == "Decision") {
        let Some(decision) = catalog.decision(&document.name) else {
            continue;
        };
        let key = Documents::key(document).to_owned();
        let proved: BTreeSet<&str> = proven
            .get(&key)
            .into_iter()
            .flatten()
            .filter(|entry| entry["basis"] == "proven")
            .filter_map(|entry| entry["with"].as_array())
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let mut list: Vec<Value> = Vec::new();
        for example in &decision.examples {
            for other in example
                .conflicts
                .iter()
                .filter(|o| !proved.contains(o.as_str()))
            {
                let settled = catalog.decision(other).is_some_and(|other| {
                    supersedes(catalog, decision, other) || supersedes(catalog, other, decision)
                });
                let entry = json!({
                    "with": [other],
                    "kind": "example",
                    "basis": "empirical",
                    "witness": { "example": example.name },
                    "resolved": settled,
                });
                if !list.contains(&entry) {
                    list.push(entry);
                }
            }
        }
        found.insert(key, list);
    }
    found
}
