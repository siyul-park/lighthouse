//! The contradictions between the decisions of a project, worked out for a run
//! in which a rule reads `decision.conflicts`: the oracle searches the order
//! decisions of each cell (an effective configuration of a language) and the
//! nogoods it finds are handed to the decision documents that are members of
//! them, as the document's `conflicts` field. A run in which no rule reads the
//! field never starts the search.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use lighthouse_graph::Graph;
use lighthouse_model::{Document, Options};
use lighthouse_plugin::{Documents, Rule};
use lighthouse_spec::{Catalog, Decision, Rules};
use serde_json::{Value, json};

use super::{Engine, Input};
use crate::{Basis, Cell, Oracle, Verdict, Witness};

/// The field of a document the search answers.
const FIELD: &str = "conflicts";

/// The bound of the search a run makes: shapes of up to this many
/// declarations. The gate and `make decisions` go further.
const BOUND: usize = 2;

/// What the search for contradictions needs besides the engine's own plugins
/// and configuration.
pub struct Conflicts {
    catalog: Arc<Catalog>,
    /// Whether a decision orders declarations: the ones the search can hold
    /// to each other.
    orders: Orders,
}

/// Which decisions order declarations.
pub type Orders = Arc<dyn Fn(&Decision) -> bool + Send + Sync>;

/// What the search of a cell found, and why it could not be done in full.
struct Found<'a> {
    nogoods: Vec<Nogood<'a>>,
    /// The decisions of nogoods that the catalog does not hold.
    lost: Vec<String>,
    incomplete: Vec<String>,
}

/// A set of decisions that cannot hold together, as one of its members sees
/// it.
struct Nogood<'a> {
    members: Vec<&'a Decision>,
    kind: &'static str,
    witness: Value,
}

impl Conflicts {
    /// The search over `catalog`, holding the decisions `orders` accepts.
    pub fn new(catalog: Arc<Catalog>, orders: Orders) -> Self {
        Self { catalog, orders }
    }

    /// The distinct effective configurations of the languages of the files:
    /// the order decisions in force for each, with their options.
    fn cells(
        &self,
        engine: &Engine,
        inputs: &[Input],
        notices: &mut BTreeSet<String>,
    ) -> Vec<(String, Rules)> {
        let mut seen = BTreeSet::new();
        let mut cells = Vec::new();
        for input in inputs {
            let file = &input.file;
            let rules = match engine
                .config
                .resolve(&file.path, &file.lang, &engine.projects)
            {
                Ok(rules) => rules,
                Err(why) => {
                    notices.insert(format!(
                        "the search for contradictions skips {}: its configuration does not resolve: {why}",
                        file.path.display()
                    ));
                    continue;
                }
            };
            if !seen.insert((file.lang.clone(), Arc::as_ptr(&rules))) {
                continue;
            }
            let order: Rules = rules
                .iter()
                .filter(|(id, c)| {
                    c.level.is_some()
                        && self.catalog.decision(id).is_some_and(|decision| {
                            decision.applies_to(&file.lang) && (self.orders)(decision)
                        })
                })
                .map(|(id, c)| {
                    let mut c = c.clone();
                    c.options =
                        super::config::options_at(&engine.inherited, &rules, id, &c.options)
                            .into_owned();
                    (id.clone(), c)
                })
                .collect();
            if !order.is_empty() {
                cells.push((file.lang.clone(), order));
            }
        }
        cells
    }

    /// What the search finds among the order decisions of the cell, in the
    /// graph of the engine.
    fn nogoods(
        &self,
        engine: &Engine,
        graph: &mut Graph,
        (language, rules): &(String, Rules),
    ) -> Found<'_> {
        let decisions: Vec<(&Decision, Options)> = rules
            .iter()
            .filter_map(|(id, c)| Some((self.catalog.decision(id)?, c.options.clone())))
            .collect();
        let cell = Cell {
            language,
            decisions,
        };
        let oracle = Oracle::sharing(Arc::clone(&engine.registry), &self.catalog);
        let verdict = oracle.search(graph, &cell, BOUND).verdict;
        let incomplete = match &verdict.basis {
            Basis::Incomplete(reasons) => reasons.clone(),
            Basis::Bounded(_) => Vec::new(),
        };
        let (nogoods, lost) = nogoods(&self.catalog, &verdict);
        Found {
            nogoods,
            lost,
            incomplete,
        }
    }
}

impl Engine {
    /// The facts of the decision documents that the engine works out, by
    /// document, when a selected rule reads them: every decision document has
    /// the field, empty unless it is a member of a contradiction.
    pub(super) fn document_facts(
        &self,
        (selected, inputs): (&[&dyn Rule], &[Input]),
        documents: &[Document],
        notices: &mut BTreeSet<String>,
    ) -> BTreeMap<String, Value> {
        let asked = selected
            .iter()
            .any(|rule| rule.reads_documents().iter().any(|field| field == FIELD));
        if !asked {
            return BTreeMap::new();
        }
        let mut facts: BTreeMap<String, Value> = documents
            .iter()
            .filter(|d| d.kind == "Decision")
            .map(|d| (Documents::key(d).to_owned(), json!({ FIELD: [] })))
            .collect();
        let Some(conflicts) = &self.conflicts else {
            return facts;
        };
        let own = own_decisions(conflicts, documents);
        if own.is_empty() {
            return facts;
        }
        let catalog = &conflicts.catalog;
        let mut entries: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut guard = self.lock_graph();
        let graph = self.graph_in(&mut guard, notices);
        for cell in conflicts.cells(self, inputs, notices) {
            let found = conflicts.nogoods(self, graph, &cell);
            let mut told = |key: String, value: Value| {
                let known = entries.entry(key).or_default();
                if !known.contains(&value) {
                    known.push(value);
                }
            };
            for nogood in &found.nogoods {
                for (key, value) in nogood.entries(catalog, &own) {
                    told(key, value);
                }
            }
            if !found.lost.is_empty() {
                notices.insert(format!(
                    "a contradiction among {} decisions names decisions the catalog does not hold, and is not told: {}",
                    cell.0,
                    found.lost.join("; ")
                ));
            }
            if !found.incomplete.is_empty() {
                notices.insert(format!(
                    "the search for contradictions among {} decisions is incomplete: {}",
                    cell.0,
                    found.incomplete.join("; ")
                ));
                let members: Vec<&Decision> = cell
                    .1
                    .keys()
                    .filter_map(|id| catalog.decision(id))
                    .collect();
                for (key, value) in incomplete_entries(&members, &own, &found.incomplete) {
                    told(key, value);
                }
            }
        }
        for (key, list) in entries {
            facts.insert(key, json!({ FIELD: list }));
        }
        facts
    }
}

impl Nogood<'_> {
    /// The entry of the nogood for each member that is a document of the
    /// project, by the key of its document. A nogood of pack decisions alone
    /// is gated where the packs are authored, not in a project that uses them.
    fn entries(&self, catalog: &Catalog, own: &BTreeSet<&str>) -> Vec<(String, Value)> {
        let mut uids: Vec<&str> = self.members.iter().map(|d| d.key()).collect();
        uids.sort_unstable();
        let mut names: Vec<&str> = self.members.iter().map(|d| d.id()).collect();
        names.sort_unstable();
        let resolved = resolved(catalog, &self.members);
        self.members
            .iter()
            .filter(|d| own.contains(d.key()))
            .map(|d| {
                let with: Vec<&str> = match self.members.len() {
                    1 => vec![d.id()],
                    _ => names.iter().copied().filter(|n| *n != d.id()).collect(),
                };
                let entry = json!({
                    "with": with,
                    "kind": self.kind,
                    "basis": "proven",
                    "witness": self.witness,
                    "resolved": resolved,
                    "names": names,
                    "uids": uids,
                });
                (d.key().to_owned(), entry)
            })
            .collect()
    }
}

/// The entry of a cell whose search is incomplete, for each decision of the
/// project in it: nothing is claimed, and the reasons are told.
fn incomplete_entries(
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

/// The nogoods a verdict holds, with the decisions they name.
/// The keys of decisions the catalog does not hold are told apart: a nogood
/// that names one cannot be told to a document.
fn nogoods<'c>(catalog: &'c Catalog, verdict: &Verdict) -> (Vec<Nogood<'c>>, Vec<String>) {
    let find = |key: &str| catalog.decisions().find(|d| d.key() == key);
    let mut found = Vec::new();
    let mut lost = Vec::new();
    for contradiction in &verdict.contradictions {
        let members: Vec<&Decision> = contradiction
            .decisions
            .iter()
            .filter_map(|key| find(key))
            .collect();
        if members.len() == contradiction.decisions.len() {
            found.push(Nogood {
                members,
                kind: "order",
                witness: witness(catalog, &contradiction.witness),
            });
        } else {
            lost.push(contradiction.decisions.join(", "));
        }
    }
    for single in &verdict.single {
        match find(&single.decision) {
            Some(decision) => found.push(Nogood {
                members: vec![decision],
                kind: "order",
                witness: json!({ "shape": single.shape.describe(), "arrangements": [], "forced": [] }),
            }),
            None => lost.push(single.decision.clone()),
        }
    }
    (found, lost)
}

/// The witness as a rule reads it: the shape in words, who flags each
/// arrangement, and the order each decision forces.
fn witness(catalog: &Catalog, witness: &Witness) -> Value {
    let name = |key: &String| {
        catalog
            .decisions()
            .find(|d| d.key() == key)
            .map_or_else(|| key.clone(), |d| d.id().to_owned())
    };
    let letter = |at: usize| char::from(b'A' + u8::try_from(at % 26).unwrap_or(0));
    json!({
        "shape": witness.shape.describe(),
        "arrangements": witness.arrangements.iter().map(|flagged| json!({
            "order": flagged.arrangement.source.iter().map(|&i| letter(i).to_string()).collect::<Vec<_>>().join(" "),
            "by": flagged.by.iter().map(name).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "forced": witness.forced.iter().map(|forced| json!({
            "first": letter(forced.first).to_string(),
            "second": letter(forced.second).to_string(),
            "by": forced.by.iter().map(name).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Whether a member supersedes another, through any chain: the contradiction
/// is then settled by the order of the decisions.
fn resolved(catalog: &Catalog, members: &[&Decision]) -> bool {
    members.iter().any(|newer| {
        members
            .iter()
            .any(|older| newer.key() != older.key() && supersedes(catalog, newer, older))
    })
}

fn supersedes(catalog: &Catalog, newer: &Decision, older: &Decision) -> bool {
    let mut pending: Vec<&str> = newer.supersedes.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if id == older.id() || Some(id) == older.uid() {
            return true;
        }
        if let Some(next) = catalog.decision(id) {
            pending.extend(next.supersedes.iter().map(String::as_str));
        }
    }
    false
}

#[cfg(test)]
mod tests;
