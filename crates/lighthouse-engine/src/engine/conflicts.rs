//! The contradictions between the decisions of a project, worked out for a run
//! in which a rule reads `decision.conflicts` or `decision.candidates`: the
//! oracle searches the order decisions of each cell (an effective
//! configuration of a language) and the nogoods it finds are handed to the
//! decision documents that are members of them, as the document's `conflicts`
//! field. A run in which no rule reads either never starts the search. The
//! same search, [`Conflicts::search`], answers `spec validate` (the cells of a
//! project, [`Engine::contradictions`]) and `decision test` (the cells of a
//! catalog): there is no other path to a contradiction.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use lighthouse_graph::{Graph, Options as GraphOptions};
use lighthouse_model::{Document, Options};
use lighthouse_plugin::{Documents, Registry, Rule};
use lighthouse_spec::{Catalog, Decision, Rules};
use serde_json::{Map, Value, json};

use super::{Engine, Input};
use crate::{Basis, Cell, Error, Oracle, Run, Verdict, Witness};

/// The field of a document the search answers.
const FIELD: &str = "conflicts";

/// The field of a document that lists the conflicts its examples declare, the
/// ones that no search proved.
const CANDIDATES: &str = "candidates";

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
pub struct Found<'a> {
    pub nogoods: Vec<Nogood<'a>>,
    /// The decisions of nogoods that the catalog does not hold.
    pub lost: Vec<String>,
    /// Why the search could not be done in full; empty when it was.
    pub incomplete: Vec<String>,
}

/// A set of decisions that cannot hold together, as one of its members sees
/// it.
pub struct Nogood<'a> {
    pub members: Vec<&'a Decision>,
    pub kind: &'static str,
    pub witness: Value,
}

/// What the search finds over the files of a project.
#[derive(Debug, Default)]
pub struct Contradictions {
    pub found: Vec<Contradicted>,
    /// What could not be searched in full, or at all.
    pub notices: BTreeSet<String>,
}

/// A contradiction found among the decisions in force for a language.
#[derive(Debug, Clone, PartialEq)]
pub struct Contradicted {
    pub language: String,
    /// The decisions that cannot hold together, sorted; one for a decision
    /// that flags every order of some declarations alone.
    pub names: Vec<String>,
    pub kind: &'static str,
    pub witness: Value,
    /// A member supersedes another: the order of the decisions settles it.
    pub resolved: bool,
    /// Whether the same decisions run with their default options contradict
    /// too: then the options and overrides of the project do not make it.
    pub at_defaults: bool,
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
                .map(|(id, c)| (id.clone(), c.clone()))
                .collect();
            if !order.is_empty() {
                cells.push((file.lang.clone(), order));
            }
        }
        cells
    }

    /// The catalog the decisions are told by.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Whether the search holds `decision` to the others.
    pub fn orders(&self, decision: &Decision) -> bool {
        (self.orders)(decision)
    }

    /// What the search finds among `decisions`, the order decisions of one
    /// language with the options they run with, over the rules of `registry`
    /// and in `graph`.
    pub fn search(
        &self,
        registry: &Arc<Registry>,
        graph: &mut Graph,
        language: &str,
        decisions: Vec<(&Decision, Options)>,
    ) -> Found<'_> {
        let cell = Cell {
            language,
            decisions,
        };
        let oracle = Oracle::sharing(Arc::clone(registry), &self.catalog);
        let verdict = oracle.search(graph, &cell, BOUND).verdict;
        let incomplete = match &verdict.basis {
            Basis::Incomplete(reasons) => reasons
                .iter()
                .map(|reason| reason.told(&|key| self.id_of(key)))
                .collect(),
            Basis::Bounded(_) => Vec::new(),
        };
        let (nogoods, lost) = nogoods(&self.catalog, &verdict);
        Found {
            nogoods,
            lost,
            incomplete,
        }
    }

    /// Like [`search`](Self::search), in a graph that lives as long as the
    /// call: what a command that has no engine and no cache asks for.
    pub fn search_once(
        &self,
        registry: &Arc<Registry>,
        language: &str,
        decisions: Vec<(&Decision, Options)>,
    ) -> Found<'_> {
        match Graph::in_memory(GraphOptions::default()) {
            Ok(mut graph) => self.search(registry, &mut graph, language, decisions),
            Err(why) => Found {
                nogoods: Vec::new(),
                lost: Vec::new(),
                incomplete: vec![format!(
                    "the search has no graph to keep its flags in: {why}"
                )],
            },
        }
    }

    /// The id of the decision the search holds by `key`, as its findings
    /// show it; the key itself when the catalog has no such decision.
    fn id_of(&self, key: &str) -> String {
        find(&self.catalog, key).map_or_else(|| key.to_owned(), |d| d.id().to_owned())
    }

    /// What the search finds among the decisions of a cell.
    fn nogoods(
        &self,
        engine: &Engine,
        graph: &mut Graph,
        (language, rules): &(String, Rules),
    ) -> Found<'_> {
        let decisions = rules
            .iter()
            .filter_map(|(id, c)| Some((self.catalog.decision(id)?, c.options.clone())))
            .collect();
        self.search(&engine.registry, graph, language, decisions)
    }
}

impl Found<'_> {
    /// What a run says of a search that names decisions the catalog does not
    /// hold, for the language.
    pub fn lost_notice(&self, language: &str) -> Option<String> {
        (!self.lost.is_empty()).then(|| {
            format!(
                "a contradiction among {language} decisions names decisions the catalog does not hold, and is not told: {}",
                self.lost.join("; ")
            )
        })
    }
}

/// The searches of a run that could not be done in full, by language, told
/// once for all of them: a reason that several configurations of a language
/// share is told once.
#[derive(Debug, Default)]
pub struct Gaps(BTreeMap<String, BTreeSet<String>>);

impl Gaps {
    /// Keeps why the search of `language` was not done in full, if it was not.
    pub fn add(&mut self, language: &str, found: &Found<'_>) {
        if !found.incomplete.is_empty() {
            self.0
                .entry(language.to_owned())
                .or_default()
                .extend(found.incomplete.iter().cloned());
        }
    }

    /// The one notice of the run, naming each language and why.
    pub fn notice(&self) -> Option<String> {
        let told: Vec<String> = self
            .0
            .iter()
            .map(|(language, reasons)| {
                let reasons: Vec<&str> = reasons.iter().map(String::as_str).collect();
                format!("{language}: {}", reasons.join("; "))
            })
            .collect();
        (!told.is_empty()).then(|| {
            format!(
                "the search for contradictions is incomplete: {}",
                told.join(" | ")
            )
        })
    }
}

impl Nogood<'_> {
    /// Whether one of its decisions supersedes another, through any chain: the
    /// order of the decisions settles it.
    pub fn resolved(&self, catalog: &Catalog) -> bool {
        resolved(catalog, &self.members)
    }

    /// The ids of its decisions, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.members.iter().map(|d| d.id().to_owned()).collect();
        names.sort_unstable();
        names
    }

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

impl Engine {
    /// The facts of the decision documents that the engine works out, by
    /// document, when a selected rule reads them: every decision document has
    /// the fields that are read, empty unless it is a member of a
    /// contradiction (`conflicts`) or an example of it declares a conflict
    /// (`candidates`).
    pub(super) fn document_facts(
        &self,
        (selected, inputs): (&[&dyn Rule], &[Input]),
        documents: &[Document],
        notices: &mut BTreeSet<String>,
    ) -> BTreeMap<String, Value> {
        let reads = |field: &str| {
            selected
                .iter()
                .any(|rule| rule.reads_documents().iter().any(|f| f == field))
        };
        let (proving, declared) = (reads(FIELD), reads(CANDIDATES));
        if !proving && !declared {
            return BTreeMap::new();
        }
        let asked: Vec<&str> = [(FIELD, proving), (CANDIDATES, declared)]
            .into_iter()
            .filter_map(|(field, asked)| asked.then_some(field))
            .collect();
        let blank: Map<String, Value> = asked
            .iter()
            .map(|field| ((*field).to_owned(), json!([])))
            .collect();
        let mut facts: BTreeMap<String, Map<String, Value>> = documents
            .iter()
            .filter(|d| d.kind == "Decision")
            .map(|d| (Documents::key(d).to_owned(), blank.clone()))
            .collect();
        if let Some(conflicts) = &self.conflicts {
            let entries = proven::of(self, conflicts, (inputs, documents), notices);
            if declared {
                for (key, list) in declared::candidates(conflicts, documents, &entries) {
                    facts
                        .entry(key)
                        .or_default()
                        .insert(CANDIDATES.to_owned(), json!(list));
                }
            }
            for (key, list) in entries.into_iter().filter(|_| proving) {
                facts
                    .entry(key)
                    .or_default()
                    .insert(FIELD.to_owned(), json!(list));
            }
        }
        facts
            .into_iter()
            .map(|(key, fields)| (key, Value::Object(fields)))
            .collect()
    }

    /// The contradictions among the order decisions in force for the files of
    /// the project, under its configuration, packs, options and overrides:
    /// the cells of the project, searched as a run reads them.
    pub fn contradictions(&self) -> Result<Contradictions, Error> {
        let mut found = Contradictions::default();
        let Some(conflicts) = &self.conflicts else {
            return Ok(found);
        };
        let overlays = super::Overlays::new();
        let detected = self.detect(&overlays, |rel| self.claimed(rel))?;
        let mut run = Run::default();
        let inputs = self.read(&detected, &overlays, &mut run);
        found.notices.extend(run.notices);
        let mut guard = self.lock_graph();
        let graph = self.graph_in(&mut guard, &mut found.notices);
        let mut gaps = Gaps::default();
        for cell in conflicts.cells(self, &inputs, &mut found.notices) {
            let (language, rules) = &cell;
            let effective = conflicts.nogoods(self, graph, &cell);
            let defaults = rules
                .keys()
                .filter_map(|id| Some((conflicts.catalog.decision(id)?, Options::new())))
                .collect();
            let baseline = conflicts.search(&self.registry, graph, language, defaults);
            let at_defaults: BTreeSet<Vec<String>> =
                baseline.nogoods.iter().map(|n| n.names()).collect();
            found.notices.extend(effective.lost_notice(language));
            gaps.add(language, &effective);
            for nogood in &effective.nogoods {
                let names = nogood.names();
                found.found.push(Contradicted {
                    language: language.clone(),
                    kind: nogood.kind,
                    witness: nogood.witness.clone(),
                    resolved: resolved(&conflicts.catalog, &nogood.members),
                    at_defaults: at_defaults.contains(&names),
                    names,
                });
            }
        }
        found.notices.extend(gaps.notice());
        Ok(found)
    }
}

/// The decision of the catalog the search holds by `key`.
fn find<'c>(catalog: &'c Catalog, key: &str) -> Option<&'c Decision> {
    catalog.decisions().find(|d| d.key() == key)
}

/// The nogoods a verdict holds, with the decisions they name.
/// The keys of decisions the catalog does not hold are told apart: a nogood
/// that names one cannot be told to a document.
fn nogoods<'c>(catalog: &'c Catalog, verdict: &Verdict) -> (Vec<Nogood<'c>>, Vec<String>) {
    let mut found = Vec::new();
    let mut lost = Vec::new();
    for contradiction in &verdict.contradictions {
        let members: Vec<&Decision> = contradiction
            .decisions
            .iter()
            .filter_map(|key| find(catalog, key))
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
        match find(catalog, &single.decision) {
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

mod declared;
mod proven;

#[cfg(test)]
mod tests;
