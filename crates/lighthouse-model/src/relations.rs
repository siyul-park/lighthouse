//! Relations read off a merged fragment: the interface methods, the edges
//! that start at each symbol and the interfaces each type implements.

use std::collections::{BTreeMap, BTreeSet};

use super::{EdgeKind, Fragment, Node, Symbol, SymbolId, SymbolKind, Target};

/// `(module, name)` of every method an interface declares.
pub(super) fn interface_methods(
    all: &Fragment,
    positions: &BTreeMap<SymbolId, usize>,
) -> BTreeSet<(String, String)> {
    let declared_by_interface = |symbol: &Symbol| {
        symbol.kind == SymbolKind::Method
            && symbol
                .owner
                .as_ref()
                .and_then(|o| positions.get(o))
                .is_some_and(|at| all.symbols[*at].kind == SymbolKind::Interface)
    };
    all.symbols
        .iter()
        .filter(|s| declared_by_interface(s))
        .map(|s| (s.id.module().to_owned(), s.name.clone()))
        .collect()
}

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
    for edge in all.edges.iter().filter(|e| e.kind == EdgeKind::Implements) {
        if let (Node::Symbol(ty), Target::Resolved(Node::Symbol(interface))) =
            (&edge.from, &edge.to)
        {
            found.entry(ty.clone()).or_default().push(interface.clone());
        }
    }
    found
}
