//! Relations read off a merged fragment: the edges that start at each symbol,
//! the interfaces each type implements, and the types each interface has.

use std::collections::BTreeMap;

use super::{EdgeKind, Fragment, Node, SymbolId, Target};

/// Positions in `all.edges` of the edges that start at each symbol.
pub(super) fn edges_from(all: &Fragment) -> BTreeMap<SymbolId, Vec<usize>> {
    let mut by_source: BTreeMap<SymbolId, Vec<usize>> = BTreeMap::new();
    for (at, edge) in all.edges.iter().enumerate() {
        if let Node::Symbol(from) = &edge.from {
            by_source.entry(from.clone()).or_default().push(at);
        }
    }
    by_source
}

/// The interfaces each type implements, by resolved `implements` edges.
pub(super) fn implements(all: &Fragment) -> BTreeMap<SymbolId, Vec<SymbolId>> {
    let mut found: BTreeMap<SymbolId, Vec<SymbolId>> = BTreeMap::new();
    for (ty, interface) in implementations(all) {
        found.entry(ty.clone()).or_default().push(interface.clone());
    }
    found
}

/// The types that implement each interface: the reverse of [`implements`].
pub(super) fn implementers(all: &Fragment) -> BTreeMap<SymbolId, Vec<SymbolId>> {
    let mut found: BTreeMap<SymbolId, Vec<SymbolId>> = BTreeMap::new();
    for (ty, interface) in implementations(all) {
        found.entry(interface.clone()).or_default().push(ty.clone());
    }
    found
}

/// The resolved `implements` edges, as (type, interface).
fn implementations(all: &Fragment) -> impl Iterator<Item = (&SymbolId, &SymbolId)> {
    all.edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Implements)
        .filter_map(|edge| match (&edge.from, &edge.to) {
            (Node::Symbol(ty), Target::Resolved(Node::Symbol(interface))) => Some((ty, interface)),
            _ => None,
        })
}
