//! The facts of the code model as queries of the graph: `aggregate` is an
//! index over the whole project, `index` its projection onto one key (what a
//! fact reads of it, so that an edit that leaves the entry as it was leaves the
//! fact green), and `fact` the values of one subject for what the rules read.
//! `reads` and `conventions` are the inputs they are made with, `Stable`.

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use lighthouse_graph::{Durability, Error, Graph, Input, Query};
use lighthouse_model::ucm::{Held, Model, SymbolId, View};
use lighthouse_plugin::{Pulling, Queries, Reads, Rule, TestNaming, Workspace, reads_of};
use serde::{Deserialize, Serialize};

use crate::layout::contains;
use crate::{
    builder::{
        Over,
        cohesion::types,
        cycle::cycles,
        effective::{Takers, takers},
        homonyms::names,
        imports::imports,
        owned::owned,
        parent::{Used, Uses, parents},
        split::splits,
        subject, tested,
    },
    fact::{Built, FACT, FUNCTION, Fact, SYMBOL, TEST, subject_of},
    indexes::{
        CREDITED, CYCLES, IMPORTS, NAMES, OWNED, PARENTS, Queried, REACH, Reached, SPLIT,
        STABILITY, TAKERS, TESTED, TYPES, USERS, credit_of,
    },
    metrics::propagation,
    naming::{Credited, credited},
};

/// What the rules of a run read of the subjects of one sort. `Stable`.
pub static READS: Input<String, Reads> = Input::new("reads");
/// How the languages name their tests and constructors. `Stable`.
pub static CONVENTIONS: Input<(), Conventions> = Input::new("conventions");

pub static AGGREGATE: Query<Sort, Aggregate> = Query::new("aggregate", aggregate).transient();
pub static INDEX: Query<(String, String), Entry> = Query::new("index", index).transient();
pub static SUBJECT: Query<(String, String), Built> = Query::new("fact", fact).transient();

/// The conventions of the languages the facts follow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conventions {
    pub test_naming: BTreeMap<String, TestNaming>,
    pub constructors: BTreeMap<String, Vec<String>>,
}

/// An index over the whole project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Aggregate {
    Takers(Takers),
    Owned(BTreeMap<SymbolId, Vec<SymbolId>>),
    Names(BTreeMap<String, Vec<SymbolId>>),
    Imports(BTreeMap<String, Vec<String>>),
    Cycles(BTreeMap<String, Vec<String>>),
    Types(BTreeMap<String, Vec<Fact>>),
    Parents(Uses),
    Splits(BTreeMap<SymbolId, Fact>),
    Modules(propagation::System),
    Symbols(propagation::System),
    Tested(BTreeSet<String>),
}

/// What an index says of one key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    Ids(Vec<SymbolId>),
    Strings(Vec<String>),
    Facts(Vec<Fact>),
    Fact(Option<Fact>),
    Used(Used),
    Reached(Option<Reached>),
    Bool(bool),
    Credited(Credited),
}

/// The sorts of aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sort {
    Takers,
    Owned,
    Names,
    Imports,
    Cycles,
    Types,
    Parents,
    Splits,
    Modules,
    Symbols,
    Tested,
}

impl Sort {
    /// The sort of aggregate the index `name` is a projection of.
    fn of(index: &str) -> Option<Self> {
        Some(match index {
            TAKERS | USERS => Self::Takers,
            OWNED => Self::Owned,
            NAMES => Self::Names,
            IMPORTS => Self::Imports,
            CYCLES => Self::Cycles,
            TYPES => Self::Types,
            PARENTS => Self::Parents,
            SPLIT => Self::Splits,
            STABILITY => Self::Modules,
            REACH => Self::Symbols,
            TESTED => Self::Tested,
            _ => return None,
        })
    }
}

/// The queries of a graph, answered by its kinds.
pub struct Pull<'g> {
    graph: &'g Graph,
}

impl<'g> Pull<'g> {
    /// The queries of `graph`.
    pub fn new(graph: &'g Graph) -> Self {
        Self { graph }
    }
}

impl Queries for Pull<'_> {
    fn pull(
        &self,
        kind: &str,
        key: &str,
    ) -> Result<Arc<dyn Any + Send + Sync>, lighthouse_plugin::Error> {
        let broken = |e: Error| lighthouse_plugin::Error::Failed(e.to_string());
        if kind == FACT {
            let (sort, id) = subject_of(key);
            let built = self
                .graph
                .query(&SUBJECT, &(sort.to_owned(), id.to_owned()))
                .map_err(broken)?;
            return Ok(built);
        }
        let entry = self
            .graph
            .query(&INDEX, &(kind.to_owned(), key.to_owned()))
            .map_err(broken)?;
        Ok(match entry.as_ref() {
            Entry::Ids(v) => Arc::new(v.clone()),
            Entry::Strings(v) => Arc::new(v.clone()),
            Entry::Facts(v) => Arc::new(v.clone()),
            Entry::Fact(v) => Arc::new(v.clone()),
            Entry::Used(v) => Arc::new(v.clone()),
            Entry::Reached(v) => Arc::new(v.clone()),
            Entry::Bool(v) => Arc::new(*v),
            Entry::Credited(v) => Arc::new(v.clone()),
        })
    }
}

/// What the engine needs of the facts: their inputs and the queries.
pub struct Pulls;

impl Pulling for Pulls {
    fn inputs(&self, graph: &mut Graph, rules: &[&dyn Rule], ws: &Workspace) -> Result<(), Error> {
        let reads = reads_of(rules.iter().copied());
        for kind in [SYMBOL, FUNCTION, TEST] {
            let reads = reads.get(kind).cloned().unwrap_or_default();
            graph.set_input(&READS, &kind.to_owned(), reads, Durability::Stable)?;
        }
        let conventions = Conventions {
            test_naming: ws.test_naming.clone(),
            constructors: ws.constructors.clone(),
        };
        graph.set_input(&CONVENTIONS, &(), conventions, Durability::Stable)
    }

    fn queries<'g>(&self, graph: &'g Graph) -> Box<dyn Queries + 'g> {
        Box::new(Pull::new(graph))
    }
}

fn aggregate(graph: &Graph, sort: &Sort) -> Result<Aggregate, Error> {
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let model: &dyn Model = &view;
    let found = match sort {
        Sort::Takers => Aggregate::Takers(takers(model)),
        Sort::Owned => Aggregate::Owned(owned(model, &workspace(graph)?)),
        Sort::Names => Aggregate::Names(names(model)),
        Sort::Imports => Aggregate::Imports(imports(model)),
        Sort::Cycles => Aggregate::Cycles(cycles(model)),
        Sort::Types => Aggregate::Types(types(model)),
        Sort::Parents => Aggregate::Parents(parents(model)),
        Sort::Splits => Aggregate::Splits(splits(model)),
        Sort::Modules => Aggregate::Modules(propagation::modules(model)),
        Sort::Symbols => Aggregate::Symbols(propagation::symbols(model)),
        Sort::Tested => Aggregate::Tested(tested(model)),
    };
    view.failure().map_or(Ok(found), Err)
}

fn index(graph: &Graph, (name, key): &(String, String)) -> Result<Entry, Error> {
    if name == CREDITED {
        let held = Held::default();
        let view = View::new(graph, &held)?;
        let (prefix, modules) = credit_of(key);
        let found = credited(&view, prefix, &modules);
        return view.failure().map_or(Ok(Entry::Credited(found)), Err);
    }
    let sort = Sort::of(name).ok_or_else(|| failed(format!("`{name}` is no index")))?;
    let aggregate = graph.query(&AGGREGATE, &sort)?;
    let id = || SymbolId::parse(key);
    let ids = |map: &BTreeMap<SymbolId, Vec<SymbolId>>| {
        Entry::Ids(
            id().and_then(|id| map.get(&id).cloned())
                .unwrap_or_default(),
        )
    };
    let strings = |map: &BTreeMap<String, Vec<String>>| {
        Entry::Strings(map.get(key.as_str()).cloned().unwrap_or_default())
    };
    Ok(match (name.as_str(), aggregate.as_ref()) {
        (TAKERS, Aggregate::Takers(t)) => {
            Entry::Ids(t.takers.get(key.as_str()).cloned().unwrap_or_default())
        }
        (USERS, Aggregate::Takers(t)) => ids(&t.users),
        (OWNED, Aggregate::Owned(map)) => ids(map),
        (NAMES, Aggregate::Names(map)) => {
            Entry::Ids(map.get(key.as_str()).cloned().unwrap_or_default())
        }
        (IMPORTS, Aggregate::Imports(map)) | (CYCLES, Aggregate::Cycles(map)) => strings(map),
        (TYPES, Aggregate::Types(map)) => {
            Entry::Facts(map.get(key.as_str()).cloned().unwrap_or_default())
        }
        (PARENTS, Aggregate::Parents(uses)) => Entry::Used(uses.of(key)),
        (SPLIT, Aggregate::Splits(map)) => Entry::Fact(id().and_then(|id| map.get(&id).cloned())),
        (STABILITY, Aggregate::Modules(system)) => Entry::Reached(Reached::of(system, key)),
        (REACH, Aggregate::Symbols(system)) => Entry::Reached(Reached::of(system, key)),
        (TESTED, Aggregate::Tested(roots)) => {
            Entry::Bool(roots.iter().any(|root| contains(root, key)))
        }
        _ => return Err(failed(format!("`{name}` is no index"))),
    })
}

fn fact(graph: &Graph, (kind, id): &(String, String)) -> Result<Built, Error> {
    let reads = graph
        .input(&READS, kind)?
        .map(|reads| reads.as_ref().clone())
        .unwrap_or_default();
    let ws = workspace(graph)?;
    let held = Held::default();
    let view = View::new(graph, &held)?;
    let pull = Pull::new(graph);
    let indexes = Queried::new(&pull);
    let found = SymbolId::parse(id).and_then(|id| view.symbol(&id));
    let fact = match found {
        Some(symbol) => {
            let over = Over {
                model: &view,
                indexes: &indexes,
                ws: &ws,
            };
            subject(kind, over, &reads, symbol).map_err(failed)?
        }
        None => None,
    };
    if let Some(error) = view.failure() {
        return Err(error);
    }
    Ok(Built::of(fact))
}

fn workspace(graph: &Graph) -> Result<Workspace, Error> {
    let conventions = graph
        .input(&CONVENTIONS, &())?
        .map(|c| c.as_ref().clone())
        .unwrap_or_default();
    let mut ws = Workspace::new("");
    ws.test_naming = conventions.test_naming;
    ws.constructors = conventions.constructors;
    Ok(ws)
}

fn failed(error: impl std::fmt::Display) -> Error {
    Error::Encode(error.to_string())
}
