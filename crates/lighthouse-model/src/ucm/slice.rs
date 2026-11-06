//! The model of a project as parts that a query graph keeps one by one: a
//! file's slice, the slice with its targets resolved, the reverse maps of all
//! the edges, and what a file sees of them. `Project::assemble` puts them
//! together, to the same result as `Project::merge` of the fragments.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::PathBuf,
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use super::{
    Edge, EdgeKind, Fragment, Node, Resolution, Site, SymbolId, Table, Target,
    merge::{EdgeRun, order_sites, resolve_in, site_of},
    qualify,
};

/// The modules a fragment declares for [`collisions`], as the slice of a file
/// would qualify them.
pub use qualify::{collisions, declared};

/// A slice with its edge, test and forwarding targets resolved against the
/// table, and how many targets stayed ambiguous.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    pub slice: Slice,
    pub ambiguous: usize,
}

impl Resolved {
    /// `slice` with its targets resolved against `table`.
    pub fn new(slice: &Slice, table: &Table) -> Self {
        let mut slice = slice.clone();
        let ambiguous = resolve_in(&mut slice.fragment, table);
        Self { slice, ambiguous }
    }
}

/// What a file contributes to the project: the fragments whose first file it is,
/// qualified and put one after the other in the order the host gave them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slice {
    pub path: PathBuf,
    pub fragment: Fragment,
    /// How many of the edges came from which fragment.
    pub runs: Vec<EdgeRun>,
}

impl Slice {
    /// The slice of `path` of these fragments; `collisions` are the module
    /// paths declared in several languages.
    pub fn new(path: PathBuf, parts: Vec<Fragment>, collisions: &BTreeSet<String>) -> Self {
        let mut slice = Self {
            path,
            ..Self::default()
        };
        for mut part in parts {
            qualify::qualify_part(&mut part, collisions);
            let file = match part.files.as_slice() {
                [only] => Some(only.path.clone()),
                _ => None,
            };
            slice.runs.push((part.edges.len(), file));
            let all = &mut slice.fragment;
            all.files.extend(part.files);
            all.modules.extend(part.modules);
            all.symbols.extend(part.symbols);
            all.places.extend(part.places);
            all.edges.extend(part.edges);
            all.functions.extend(part.functions);
            all.tests.extend(part.tests);
            all.comments.extend(part.comments);
        }
        slice
    }
}

/// What every edge of the project adds up to, apart from where they are: the
/// reverse maps, and the edges a later file repeats, which the project does
/// not keep.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edges {
    /// By file, the positions among the edges of its slice of those that
    /// repeat an earlier relation.
    pub dropped: BTreeMap<PathBuf, Vec<u32>>,
    pub reverse: Arc<Reverse>,
}

impl Edges {
    /// `resolved` are the slices in the order of their paths.
    pub fn new<'a>(table: &Table, resolved: impl IntoIterator<Item = &'a Resolved>) -> Self {
        let mut seen = HashSet::new();
        let mut dropped: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
        let mut kept: Vec<&Edge> = Vec::new();
        for resolved in resolved {
            let slice = &resolved.slice;
            let mut files = slice
                .runs
                .iter()
                .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
            for (at, edge) in slice.fragment.edges.iter().enumerate() {
                files.next();
                let key = (edge.kind, &edge.from, &edge.to, edge.resolution, edge.when);
                if seen.insert(key) {
                    kept.push(edge);
                } else {
                    dropped
                        .entry(slice.path.clone())
                        .or_default()
                        .push(at as u32);
                }
            }
        }
        Self {
            dropped,
            reverse: Arc::new(Reverse::link(table, kept.into_iter())),
        }
    }
}

/// The relations between symbols, by the symbol at the end that is asked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reverse {
    pub callers: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub callees: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub references: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub uses: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub members: BTreeMap<SymbolId, Vec<SymbolId>>,
    pub implementers: BTreeMap<SymbolId, Vec<SymbolId>>,
    /// Every edge that ends at a symbol: its kind, resolution and start.
    pub incoming: BTreeMap<SymbolId, Vec<(EdgeKind, Resolution, Node)>>,
}

impl Reverse {
    /// Links the edges, in the project's order: the implementers of
    /// interfaces, and the calls, uses and references between resolved
    /// symbols of project code; and the members of every owner.
    pub(super) fn link<'a>(table: &Table, edges: impl Iterator<Item = &'a Edge>) -> Self {
        let mut reverse = Self::default();
        let mut calls = BTreeSet::new();
        let mut references = BTreeSet::new();
        let mut uses = BTreeSet::new();
        for edge in edges {
            if let Target::Resolved(Node::Symbol(to)) = &edge.to {
                reverse.incoming.entry(to.clone()).or_default().push((
                    edge.kind,
                    edge.resolution,
                    edge.from.clone(),
                ));
            }
            let Node::Symbol(from) = &edge.from else {
                continue;
            };
            // An example uses nothing: its edges stay as sites of use, and are
            // not callers, references or implementers of what they name.
            let in_example_file = table
                .symbols
                .get(from)
                .is_some_and(|entry| table.examples.contains(&entry.file));
            if table.modules.get(from.module()).is_some_and(|m| m.example) || in_example_file {
                continue;
            }
            let Target::Resolved(Node::Symbol(to)) = &edge.to else {
                continue;
            };
            if edge.kind == EdgeKind::Implements {
                reverse
                    .implementers
                    .entry(to.clone())
                    .or_default()
                    .push(from.clone());
            }
            let precise = edge.resolution != Resolution::Heuristic;
            if from != to && precise && matches!(edge.kind, EdgeKind::Calls | EdgeKind::References)
            {
                uses.insert((from, to));
            }
            match edge.kind {
                EdgeKind::Calls if from != to && precise => calls.insert((from, to)),
                EdgeKind::References if from != to => references.insert((from, to)),
                _ => false,
            };
        }
        for (from, to) in calls {
            reverse
                .callees
                .entry(from.clone())
                .or_default()
                .push(to.clone());
            reverse
                .callers
                .entry(to.clone())
                .or_default()
                .push(from.clone());
        }
        for (from, to) in uses {
            reverse
                .uses
                .entry(from.clone())
                .or_default()
                .push(to.clone());
        }
        for (from, to) in references {
            reverse
                .references
                .entry(to.clone())
                .or_default()
                .push(from.clone());
        }
        for (id, entry) in &table.symbols {
            if let Some(owner) = &entry.owner {
                reverse
                    .members
                    .entry(owner.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        reverse
    }
}

/// Where the edges refer to a symbol from, by the symbol: the one thing of the
/// edges that an edit moving a line changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sites(pub BTreeMap<SymbolId, Vec<Site>>);

impl Sites {
    /// `resolved` are the slices in the order of their paths.
    pub fn new<'a>(table: &Table, resolved: impl IntoIterator<Item = &'a Resolved>) -> Self {
        let mut sites: BTreeMap<SymbolId, Vec<Site>> = BTreeMap::new();
        for resolved in resolved {
            let slice = &resolved.slice;
            let mut files = slice
                .runs
                .iter()
                .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
            for edge in &slice.fragment.edges {
                if let Some((to, site)) = site_of(edge, files.next().flatten(), table) {
                    sites.entry(to).or_default().push(site);
                }
            }
        }
        order_sites(&mut sites);
        Self(sites)
    }
}

/// What the rest of the project says of the symbols a file declares: who calls
/// and references them, who implements them, whose owner they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incoming {
    pub by_symbol: BTreeMap<SymbolId, Tie>,
}

/// The other ends of the relations that end at one symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tie {
    pub callers: Vec<SymbolId>,
    pub references: Vec<SymbolId>,
    pub implementers: Vec<SymbolId>,
    pub members: Vec<SymbolId>,
    /// The symbols it calls, which what other files repeat decides.
    pub callees: Vec<SymbolId>,
    /// The symbols it calls or references through a resolved edge.
    pub uses: Vec<SymbolId>,
    /// Every edge that ends at it.
    pub edges: Vec<(EdgeKind, Resolution, Node)>,
}

impl Incoming {
    /// The projection of the reverse maps onto the symbols declared in the
    /// file of `resolved`.
    pub fn new(resolved: &Resolved, table: &Table, reverse: &Reverse) -> Self {
        let path = &resolved.slice.path;
        let mut by_symbol = BTreeMap::new();
        for symbol in &resolved.slice.fragment.symbols {
            if table.entry(&symbol.id).is_none_or(|e| &e.file != path) {
                continue;
            }
            let of = |map: &BTreeMap<SymbolId, Vec<SymbolId>>| {
                map.get(&symbol.id).cloned().unwrap_or_default()
            };
            let tie = Tie {
                callers: of(&reverse.callers),
                references: of(&reverse.references),
                implementers: of(&reverse.implementers),
                members: of(&reverse.members),
                callees: of(&reverse.callees),
                uses: of(&reverse.uses),
                edges: reverse
                    .incoming
                    .get(&symbol.id)
                    .cloned()
                    .unwrap_or_default(),
            };
            if tie != Tie::default() {
                by_symbol.insert(symbol.id.clone(), tie);
            }
        }
        Self { by_symbol }
    }
}
