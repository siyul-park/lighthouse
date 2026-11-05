//! Merging fragments into a project: resolving the targets of edges, the
//! sites of references, and the union of the symbols of all files.

use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use super::{
    Edge, EdgeKind, Fragment, Node, Resolution, Site, Symbol, SymbolId, SymbolKind, Table, Target,
};

/// A run of consecutive edges that came from one fragment: how many, and the
/// file of the fragment when it is about one.
pub type EdgeRun = (usize, Option<PathBuf>);

struct Resolver<'a> {
    table: &'a Table,
    ambiguous: Cell<usize>,
}

impl<'a> Resolver<'a> {
    fn new(table: &'a Table) -> Self {
        Self {
            table,
            ambiguous: Cell::new(0),
        }
    }

    /// A kind-less path naming several symbols resolves only for a call that
    /// matches exactly one function or method; otherwise it stays a path.
    fn resolve(&self, target: &Target, call: bool, by_name: bool) -> Target {
        let Target::Path(path) = target else {
            return target.clone();
        };
        let symbol = |id: &str| Target::Resolved(Node::Symbol(SymbolId(id.to_owned())));
        if self.table.symbols.contains_key(path.as_str()) {
            return symbol(path);
        }
        if self.table.modules.contains_key(path.as_str()) {
            return Target::Resolved(Node::Module(path.clone()));
        }
        let found = self.table.bare(path);
        if found.is_empty() {
            // An edge that names a symbol and no module is read by the name
            // alone: it is the symbol of the project that is called so, when
            // one is.
            if by_name
                && !path.contains("::")
                && let Some([only]) = self.table.named.get(path.as_str()).map(Vec::as_slice)
            {
                return symbol(only.as_str());
            }
            return target.clone();
        }
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
    let table = Table::of(all);
    resolve_in(all, &table)
}

/// Resolves the targets of one fragment against the table of the project.
pub(super) fn resolve_in(all: &mut Fragment, table: &Table) -> usize {
    let resolver = Resolver::new(table);
    let resolved: Vec<Target> = all
        .edges
        .iter()
        .map(|e| {
            resolver.resolve(
                &e.to,
                e.kind == EdgeKind::Calls,
                e.resolution == Resolution::Syntactic,
            )
        })
        .collect();
    let targets: Vec<Vec<Target>> = all
        .tests
        .iter()
        .map(|t| {
            t.targets
                .iter()
                .map(|x| resolver.resolve(x, false, false))
                .collect()
        })
        .collect();
    let forwards: Vec<Option<Target>> = all
        .functions
        .iter()
        .map(|f| {
            f.forwards_to
                .as_ref()
                .map(|t| resolver.resolve(t, true, false))
        })
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
            let fresh = seen.insert((e.kind, &e.from, &e.to, e.resolution, e.when));
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

/// The place an edge refers to a symbol from, when it has a site and ends at
/// a symbol: the symbol, and the place. `run` is the file of the fragment the
/// edge came from, when it is about one; an edge of another starts at the
/// symbol, whose file the table knows.
pub(super) fn site_of(
    edge: &Edge,
    run: Option<&PathBuf>,
    table: &Table,
) -> Option<(SymbolId, Site)> {
    let (Target::Resolved(Node::Symbol(to)), Some(span)) = (&edge.to, edge.site) else {
        return None;
    };
    let file = run.cloned().or_else(|| match &edge.from {
        Node::Symbol(id) => table.symbols.get(id).map(|e| e.file.clone()),
        Node::Module(_) => None,
    })?;
    Some((
        to.clone(),
        Site {
            file,
            from: edge.from.clone(),
            kind: edge.kind,
            resolution: edge.resolution,
            span,
        },
    ))
}

/// Puts the sites of one target in file and position order, without repeats.
pub(super) fn order_sites(sites: &mut BTreeMap<SymbolId, Vec<Site>>) {
    for list in sites.values_mut() {
        list.sort_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start)));
        list.dedup();
    }
}

/// The sited edges that end at a symbol, grouped by target, ordered by file
/// and position. `runs` says how many of `all.edges`, in order, came from the
/// fragment of which file.
pub(super) fn sites_of(
    all: &Fragment,
    table: &Table,
    runs: &[EdgeRun],
) -> BTreeMap<SymbolId, Vec<Site>> {
    let mut files = runs
        .iter()
        .flat_map(|(len, file)| std::iter::repeat_n(file.as_ref(), *len));
    let mut sites: BTreeMap<SymbolId, Vec<Site>> = BTreeMap::new();
    for edge in &all.edges {
        let file = files.next().flatten();
        if let Some((to, site)) = site_of(edge, file, table) {
            sites.entry(to).or_default().push(site);
        }
    }
    order_sites(&mut sites);
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
    super::qualify::qualify(&mut parts);
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
    let notices = settle(&mut all).into_iter().collect();
    (all, notices, edge_files)
}

/// Sorts and deduplicates what was concatenated by identity, and says when a
/// symbol id is declared in several files.
pub(super) fn settle(all: &mut Fragment) -> Option<String> {
    all.files.sort_by(|a, b| a.path.cmp(&b.path));
    // What the providers that read the regions of a host file state of it
    // wins over what the host states: they come after it.
    all.files.dedup_by(|later, kept| {
        let same = later.path == kept.path;
        if same {
            kept.facts.append(&mut later.facts);
        }
        same
    });
    all.modules.sort_by(|a, b| a.path.cmp(&b.path));
    all.modules.dedup_by(|later, kept| {
        let same = later.path == kept.path;
        if same {
            super::table::reconcile(kept, later);
        }
        same
    });
    all.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    let notice = duplicate_notice(&all.symbols);
    all.symbols.dedup_by(|a, b| a.id == b.id);
    all.functions.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.functions.dedup_by(|a, b| a.symbol == b.symbol);
    all.tests.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    all.tests.dedup_by(|a, b| a.symbol == b.symbol);
    all.comments
        .sort_by(|a, b| (&a.file, a.span.start).cmp(&(&b.file, b.span.start)));
    all.comments.dedup();
    notice
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
