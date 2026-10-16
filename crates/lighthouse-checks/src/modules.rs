//! What the checks over modules know about each one beyond its path: where it
//! is declared, how much it declares and who imports it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::{EdgeKind, Node, Project, Symbol, SymbolKind, Target};

/// A module that declares a symbol.
pub(crate) struct Declared<'p> {
    /// The symbol that sorts first by file and position; the findings of a
    /// project decision about the module are reported there.
    pub anchor: &'p Symbol,
    /// The symbols it declares.
    pub symbols: usize,
    /// The files that declare a symbol of it.
    pub files: BTreeSet<&'p Path>,
    /// Lines from the first to the last declaration, summed over the module's
    /// files: file lengths are not part of the code model.
    pub lines: u32,
    /// The distinct production modules that import the module.
    pub dependents: BTreeSet<String>,
    /// The declarations of the module: types, interfaces, constants,
    /// variables and functions at module level, without `init`.
    pub declares: usize,
}

/// Every module that declares a symbol, by path, in one pass over the symbols.
pub(crate) fn index(project: &Project) -> BTreeMap<&str, Declared<'_>> {
    let mut modules: BTreeMap<&str, Declared> = BTreeMap::new();
    let mut extents: BTreeMap<(&str, &Path), (u32, u32)> = BTreeMap::new();
    for symbol in &project.symbols {
        let path = symbol.id.module();
        let span = symbol.extent.unwrap_or(symbol.span);
        let (first, last) = extents
            .entry((path, symbol.file.as_path()))
            .or_insert((span.start.line, span.end.line));
        *first = (*first).min(span.start.line);
        *last = (*last).max(span.end.line);
        let module = modules.entry(path).or_insert_with(|| Declared {
            anchor: symbol,
            symbols: 0,
            files: BTreeSet::new(),
            lines: 0,
            dependents: BTreeSet::new(),
            declares: 0,
        });
        module.symbols += 1;
        module.files.insert(symbol.file.as_path());
        if (&symbol.file, symbol.span.start) < (&module.anchor.file, module.anchor.span.start) {
            module.anchor = symbol;
        }
        module.declares += usize::from(declares(symbol));
    }
    for ((path, _), (first, last)) in extents {
        if let Some(module) = modules.get_mut(path) {
            module.lines += last - first + 1;
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
            && let Some(module) = modules.get_mut(to.as_str())
        {
            module.dependents.insert(from.clone());
        }
    }
    modules
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
