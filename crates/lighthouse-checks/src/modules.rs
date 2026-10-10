//! What the checks over modules know about each one beyond its path: how much
//! it declares and who imports it.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::{EdgeKind, Node, Project, Symbol, SymbolKind, Target};

/// The measures of one module.
#[derive(Debug, Default)]
pub(crate) struct Stats {
    /// Lines from the first to the last declaration, summed over the module's
    /// files: file lengths are not part of the code model.
    pub lines: u32,
    /// The distinct production modules that import the module.
    pub dependents: BTreeSet<String>,
    /// The declarations of the module: types, interfaces, constants,
    /// variables and functions at module level, without `init`.
    pub declares: usize,
}

/// The stats of every module that declares a symbol.
pub(crate) fn stats(project: &Project) -> BTreeMap<String, Stats> {
    let mut stats: BTreeMap<String, Stats> = BTreeMap::new();
    let mut extents: BTreeMap<(&str, &std::path::Path), (u32, u32)> = BTreeMap::new();
    for symbol in &project.symbols {
        let module = symbol.id.module();
        let span = symbol.extent.unwrap_or(symbol.span);
        let (first, last) = extents
            .entry((module, symbol.file.as_path()))
            .or_insert((span.start.line, span.end.line));
        *first = (*first).min(span.start.line);
        *last = (*last).max(span.end.line);
        let entry = stats.entry(module.to_owned()).or_default();
        if declares(symbol) {
            entry.declares += 1;
        }
    }
    for ((module, _), (first, last)) in extents {
        if let Some(entry) = stats.get_mut(module) {
            entry.lines += last - first + 1;
        }
    }
    for edge in project.edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
        let (Node::Module(from), Target::Resolved(Node::Module(to))) = (&edge.from, &edge.to)
        else {
            continue;
        };
        let production = project.module(from).is_some_and(|m| m.test_of.is_none());
        if from != to
            && production
            && let Some(entry) = stats.get_mut(to.as_str())
        {
            entry.dependents.insert(from.clone());
        }
    }
    stats
}

fn declares(symbol: &Symbol) -> bool {
    symbol.owner.is_none()
        && symbol.name != "init"
        && matches!(
            symbol.kind,
            SymbolKind::Type
                | SymbolKind::Interface
                | SymbolKind::Const
                | SymbolKind::Var
                | SymbolKind::Function
        )
}
