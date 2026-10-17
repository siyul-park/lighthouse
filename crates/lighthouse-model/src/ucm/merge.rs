//! Merging fragments into a project: resolving the targets of edges, the
//! sites of references, and the union of the symbols of all files.

use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, HashSet},
    path::PathBuf,
};

use super::{EdgeKind, Fragment, Node, Site, Symbol, SymbolId, SymbolKind, Target};

/// A run of consecutive edges that came from one fragment: how many, and the
/// file of the fragment when it is about one.
type EdgeRun = (usize, Option<PathBuf>);

struct Resolver<'a> {
    symbols: BTreeSet<&'a str>,
    bare: BTreeMap<&'a str, Vec<(&'a SymbolId, SymbolKind)>>,
    modules: BTreeSet<&'a str>,
    ambiguous: Cell<usize>,
}

impl<'a> Resolver<'a> {
    fn new(all: &'a Fragment) -> Self {
        let mut bare: BTreeMap<&str, Vec<(&SymbolId, SymbolKind)>> = BTreeMap::new();
        for symbol in &all.symbols {
            let id = symbol.id.as_str();
            bare.entry(id.split_once('#').map_or(id, |(head, _)| head))
                .or_default()
                .push((&symbol.id, symbol.kind));
        }
        Self {
            symbols: all.symbols.iter().map(|s| s.id.as_str()).collect(),
            bare,
            modules: all.modules.iter().map(|m| m.path.as_str()).collect(),
            ambiguous: Cell::new(0),
        }
    }

    /// A kind-less path naming several symbols resolves only for a call that
    /// matches exactly one function or method; otherwise it stays a path.
    fn resolve(&self, target: &Target, call: bool) -> Target {
        let Target::Path(path) = target else {
            return target.clone();
        };
        let symbol = |id: &str| Target::Resolved(Node::Symbol(SymbolId(id.to_owned())));
        if self.symbols.contains(path.as_str()) {
            return symbol(path);
        }
        if self.modules.contains(path.as_str()) {
            return Target::Resolved(Node::Module(path.clone()));
        }
        let Some(found) = self.bare.get(path.as_str()) else {
            return target.clone();
        };
        if let [(only, _)] = found.as_slice() {
            return symbol(only.as_str());
        }
        if call {
            let callable: Vec<_> = found
                .iter()
                .filter(|(_, kind)| matches!(kind, SymbolKind::Function | SymbolKind::Method))
                .collect();
            if let [(only, _)] = callable.as_slice() {
                return symbol(only.as_str());
            }
        }
        self.ambiguous.set(self.ambiguous.get() + 1);
        target.clone()
    }
}

/// Resolves edge, test and forwarding targets in place and returns how many
/// targets stayed ambiguous.
pub(super) fn resolve_all(all: &mut Fragment) -> usize {
    let resolver = Resolver::new(all);
    let resolved: Vec<Target> = all
        .edges
        .iter()
        .map(|e| resolver.resolve(&e.to, e.kind == EdgeKind::Calls))
        .collect();
    let targets: Vec<Vec<Target>> = all
        .tests
        .iter()
        .map(|t| {
            t.targets
                .iter()
                .map(|x| resolver.resolve(x, false))
                .collect()
        })
        .collect();
    let forwards: Vec<Option<Target>> = all
        .functions
        .iter()
        .map(|f| f.forwards_to.as_ref().map(|t| resolver.resolve(t, true)))
        .collect();
    let ambiguous = resolver.ambiguous.get();
    for (edge, to) in all.edges.iter_mut().zip(resolved) {
        edge.to = to;
    }
    for (test, targets) in all.tests.iter_mut().zip(targets) {
        test.targets = targets;
    }
    for (function, to) in all.functions.iter_mut().zip(forwards) {
        function.forwards_to = to;
    }
    ambiguous
}

/// Drops duplicate edges: the same relation at several sites is one edge,
/// which keeps its first site. Returns, by position in the kept edges, the
/// file of the fragment of each edge that starts at a module.
pub(super) fn dedup_edges(all: &mut Fragment, runs: &[EdgeRun]) -> BTreeMap<usize, PathBuf> {
    let mut seen = HashSet::new();
    let mut files = runs
        .iter()
        .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
    let mut origins = BTreeMap::new();
    let mut kept = 0;
    let keep: Vec<bool> = all
        .edges
        .iter()
        .map(|e| {
            let file = files.next().flatten();
            let fresh = seen.insert((e.kind, &e.from, &e.to, e.resolution));
            if fresh {
                if let (Node::Module(_), Some(file)) = (&e.from, file) {
                    origins.insert(kept, file.clone());
                }
                kept += 1;
            }
            fresh
        })
        .collect();
    let mut keep = keep.into_iter();
    all.edges.retain(|_| keep.next().unwrap_or(true));
    origins
}

/// The sited edges that end at a symbol, grouped by target, ordered by file
/// and position. `runs` says how many of `all.edges`, in order, came from the
/// fragment of which file.
pub(super) fn sites_of(all: &Fragment, runs: &[EdgeRun]) -> BTreeMap<SymbolId, Vec<Site>> {
    let file_of = |from: &Node| match from {
        Node::Symbol(id) => all
            .symbols
            .binary_search_by(|s| s.id.cmp(id))
            .ok()
            .map(|at| all.symbols[at].file.clone()),
        Node::Module(_) => None,
    };
    let mut files = runs
        .iter()
        .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
    let mut sites: BTreeMap<SymbolId, Vec<Site>> = BTreeMap::new();
    for edge in &all.edges {
        let file = files.next().flatten();
        let (Target::Resolved(Node::Symbol(to)), Some(span)) = (&edge.to, edge.site) else {
            continue;
        };
        let Some(file) = file.cloned().or_else(|| file_of(&edge.from)) else {
            continue;
        };
        sites.entry(to.clone()).or_default().push(Site {
            file,
            from: edge.from.clone(),
            kind: edge.kind,
            resolution: edge.resolution,
            span,
        });
    }
    for list in sites.values_mut() {
        list.sort_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start)));
        list.dedup();
    }
    sites
}

/// The fragments concatenated, sorted and deduplicated by identity, with a
/// notice when a symbol id is declared in several files, and for each
/// fragment how many edges it had and the file of the fragment when it is
/// about one.
pub(super) fn sorted_union(
    parts: impl IntoIterator<Item = Fragment>,
) -> (Fragment, Vec<String>, Vec<EdgeRun>) {
    let mut all = Fragment::default();
    let mut edge_files = Vec::new();
    // By file, so that the edge that stands for several, and its site, do not
    // depend on the order the providers answered in.
    let mut parts: Vec<Fragment> = parts.into_iter().collect();
    let first = |f: &Fragment| f.files.first().map(|file| file.path.clone());
    parts.sort_by_cached_key(first);
    for part in parts {
        let file = match part.files.as_slice() {
            [only] => Some(only.path.clone()),
            _ => None,
        };
        edge_files.push((part.edges.len(), file));
        all.files.extend(part.files);
        all.modules.extend(part.modules);
        all.symbols.extend(part.symbols);
        all.edges.extend(part.edges);
        all.functions.extend(part.functions);
        all.tests.extend(part.tests);
        all.comments.extend(part.comments);
    }
    all.files.sort_by(|a, b| a.path.cmp(&b.path));
    all.files.dedup_by(|a, b| a.path == b.path);
    all.modules.sort_by(|a, b| a.path.cmp(&b.path));
    all.modules.dedup_by(|a, b| a.path == b.path);
    all.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    let notices = duplicate_notice(&all.symbols).into_iter().collect();
    all.symbols.dedup_by(|a, b| a.id == b.id);
    all.functions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.functions.dedup_by(|a, b| a.symbol == b.symbol);
    all.tests.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.tests.dedup_by(|a, b| a.symbol == b.symbol);
    all.comments
        .sort_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start)));
    all.comments.dedup();
    (all, notices, edge_files)
}

fn duplicate_notice(sorted: &[Symbol]) -> Option<String> {
    let mut groups: Vec<(&SymbolId, Vec<String>)> = Vec::new();
    for pair in sorted.windows(2) {
        if pair[0].id != pair[1].id || pair[0].file == pair[1].file {
            continue;
        }
        let (a, b) = (
            pair[0].file.display().to_string(),
            pair[1].file.display().to_string(),
        );
        match groups.last_mut() {
            Some((id, files)) if **id == pair[0].id => files.push(b),
            _ => groups.push((&pair[0].id, vec![a, b])),
        }
    }
    if groups.is_empty() {
        return None;
    }
    let examples: Vec<String> = groups
        .iter()
        .take(3)
        .map(|(id, files)| format!("{} ({})", id.as_str(), files.join(", ")))
        .collect();
    Some(format!(
        "{} symbol id(s) are declared in several files and only the first is analyzed: {}",
        groups.len(),
        examples.join("; ")
    ))
}
