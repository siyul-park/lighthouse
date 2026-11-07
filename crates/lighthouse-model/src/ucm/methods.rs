//! Interfaces that types satisfy by structure, settled across the units of a
//! language.
//!
//! A provider of such a language states, for a unit, what each type provides,
//! what each interface requires and which methods it could not place; the keys
//! are opaque. `methods` is those facts of one unit, `implements` reads them
//! for every unit of the language and decides which type satisfies which
//! interface, and `settled` is the part of that which concerns one unit. An
//! edit that leaves the facts of a unit as they were stops at `methods`.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

use lighthouse_graph::{Error, Graph};
use serde::{Deserialize, Serialize};

use super::{
    Edge, EdgeKind, Fragment, Node, Resolution, SymbolId, Target, When,
    kinds::{IMPLEMENTS, MEMBERS, METHODS, PAYLOAD, Payload, SETTLED},
};

/// What the provider said of the method sets of a unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Methods {
    pub types: Vec<Provided>,
    pub interfaces: Vec<Required>,
    pub pending: Vec<Pending>,
}

impl Methods {
    /// Whether the provider stated no method set.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty() && self.interfaces.is_empty() && self.pending.is_empty()
    }
}

/// A type and the keys of the methods it provides, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provided {
    pub symbol: SymbolId,
    pub at: At,
    pub keys: Vec<String>,
}

/// An interface and the keys of the methods it requires, sorted. It has
/// `edges` when a type that satisfies it implements it as an edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Required {
    pub symbol: SymbolId,
    pub at: At,
    pub keys: Vec<String>,
    pub edges: bool,
}

/// A method that implements an interface if one requires its key and a type
/// that carries the method provides all that interface requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub method: SymbolId,
    pub key: String,
    pub carriers: Vec<SymbolId>,
}

/// Where a declaration starts: the order in which the edges of a file are
/// written.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct At {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// What the languages' types satisfy, by unit of the type or method.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implemented {
    units: BTreeMap<String, Settled>,
}

/// What settling decided for the types and methods of one unit: the
/// `implements` edges in the order of the types, then of the interfaces, and
/// the methods that implement an interface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settled {
    pub edges: Vec<(SymbolId, SymbolId)>,
    pub methods: BTreeSet<SymbolId>,
}

impl Settled {
    /// Adds to `part` the edges from the types it declares and marks the
    /// methods it declares that implement an interface.
    pub(super) fn apply(&self, part: &mut Fragment) {
        let declared: BTreeSet<&SymbolId> = part.symbols.iter().map(|s| &s.id).collect();
        let edges: Vec<Edge> = self
            .edges
            .iter()
            .filter(|(from, _)| declared.contains(from))
            .map(|(from, to)| edge(from, to))
            .collect();
        part.edges.extend(edges);
        for symbol in &mut part.symbols {
            if self.methods.contains(&symbol.id) {
                symbol.implementation = true;
            }
        }
    }
}

/// The method-set facts of the unit `(language, unit)`.
pub(super) fn methods(
    graph: &Graph,
    (language, unit): &(String, String),
) -> Result<Methods, Error> {
    Ok(graph
        .input(&PAYLOAD, &(language.clone(), unit.clone()))?
        .map(|payload| payload.methods.clone())
        .unwrap_or_default())
}

/// Which type satisfies which interface and which methods implement one, over
/// the units of a language.
pub(super) fn implements(graph: &Graph, language: &String) -> Result<Implemented, Error> {
    let members = graph.input(&MEMBERS, language)?.unwrap_or_default();
    let mut found: Vec<(&String, Arc<Methods>)> = Vec::new();
    for unit in members.iter() {
        let methods = graph.query(&METHODS, &(language.clone(), unit.clone()))?;
        if !methods.is_empty() {
            found.push((unit, methods));
        }
    }
    let mut types: Vec<(&String, &Provided)> = found
        .iter()
        .flat_map(|(unit, methods)| methods.types.iter().map(move |t| (*unit, t)))
        .collect();
    types.sort_by(|a, b| (&a.1.at, &a.1.symbol).cmp(&(&b.1.at, &b.1.symbol)));
    let mut interfaces: Vec<&Required> = found
        .iter()
        .flat_map(|(_, methods)| &methods.interfaces)
        .filter(|iface| !iface.keys.is_empty())
        .collect();
    interfaces.sort_by(|a, b| (&a.at, &a.symbol).cmp(&(&b.at, &b.symbol)));
    let mut wanting: HashMap<&str, Vec<usize>> = HashMap::new();
    for (at, iface) in interfaces.iter().enumerate() {
        for key in &iface.keys {
            wanting.entry(key).or_default().push(at);
        }
    }
    let mut out = Implemented::default();
    for (unit, provided) in &types {
        let satisfied = satisfied(&provided.keys, &interfaces, &wanting);
        let edges = satisfied.into_iter().filter(|at| interfaces[*at].edges);
        out.units
            .entry((*unit).clone())
            .or_default()
            .edges
            .extend(edges.map(|at| (provided.symbol.clone(), interfaces[at].symbol.clone())));
    }
    let mut carried: HashMap<&SymbolId, &Provided> = HashMap::new();
    for (_, provided) in &types {
        carried.entry(&provided.symbol).or_insert(provided);
    }
    for (unit, methods) in &found {
        let settled = methods
            .pending
            .iter()
            .filter(|pending| carried_by_some(pending, &wanting, &interfaces, &carried))
            .map(|pending| pending.method.clone());
        out.units
            .entry((*unit).clone())
            .or_default()
            .methods
            .extend(settled);
    }
    Ok(out)
}

/// Adds to the fragments of a unit what settling its method sets decided: the
/// edges from their types and the methods that implement an interface.
pub(super) fn settle(
    graph: &Graph,
    at: &(String, String),
    payload: &Payload,
    parts: &mut [Fragment],
) -> Result<(), Error> {
    if payload.methods.is_empty() {
        return Ok(());
    }
    let settled = graph.query(&SETTLED, at)?;
    parts.iter_mut().for_each(|part| settled.apply(part));
    Ok(())
}

/// The part of [`implements`] that concerns one unit.
pub(super) fn settled(
    graph: &Graph,
    (language, unit): &(String, String),
) -> Result<Settled, Error> {
    let implemented = graph.query(&IMPLEMENTS, language)?;
    Ok(implemented.units.get(unit).cloned().unwrap_or_default())
}

/// Whether an interface requires the key of the method and a carrier of it
/// provides all that interface requires.
fn carried_by_some(
    pending: &Pending,
    wanting: &HashMap<&str, Vec<usize>>,
    interfaces: &[&Required],
    carried: &HashMap<&SymbolId, &Provided>,
) -> bool {
    let mut wanted = wanting.get(pending.key.as_str()).into_iter().flatten();
    wanted.any(|at| {
        pending.carriers.iter().any(|carrier| {
            carried
                .get(carrier)
                .is_some_and(|t| includes(&t.keys, &interfaces[*at].keys))
        })
    })
}

/// The interfaces, by position in `interfaces`, whose every key is among
/// `keys`.
fn satisfied(
    keys: &[String],
    interfaces: &[&Required],
    wanting: &HashMap<&str, Vec<usize>>,
) -> Vec<usize> {
    let mut hits: HashMap<usize, usize> = HashMap::new();
    for key in keys {
        for at in wanting.get(key.as_str()).into_iter().flatten() {
            *hits.entry(*at).or_default() += 1;
        }
    }
    let mut all: Vec<usize> = hits
        .into_iter()
        .filter(|(at, count)| *count == interfaces[*at].keys.len())
        .map(|(at, _)| at)
        .collect();
    all.sort_unstable();
    all
}

/// Whether the sorted `have` holds every one of the sorted `need`.
fn includes(have: &[String], need: &[String]) -> bool {
    let mut have = have.iter();
    need.iter().all(|key| have.any(|k| k == key))
}

/// The `implements` edge from a type to an interface.
fn edge(from: &SymbolId, to: &SymbolId) -> Edge {
    let target = to
        .as_str()
        .rsplit_once('#')
        .map_or(to.as_str(), |(id, _)| id);
    Edge {
        kind: EdgeKind::Implements,
        from: Node::Symbol(from.clone()),
        to: Target::Path(target.to_owned()),
        resolution: Resolution::Semantic,
        site: None,
        when: When::Call,
    }
}
