//! What a function uses of the project, the import graph, the helpers that move
//! with a function and what its signature says about its role.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::ucm::{EdgeKind, Model, Node, Symbol, SymbolId, SymbolKind, Target};

use crate::layout;

/// The project symbols a function uses, by module: those of test and
/// generated code and of an ancestor module (a submodule uses its parent's
/// types as a matter of course) are left out.
pub(super) struct Uses<'p> {
    pub(super) by_module: BTreeMap<String, Vec<&'p Symbol>>,
    pub(super) total: usize,
}

impl<'p> Uses<'p> {
    pub(super) fn of(project: &'p dyn Model, symbol: &Symbol, unit: &[&Symbol]) -> Self {
        let home = symbol.id.module();
        let mut by_module: BTreeMap<String, Vec<&Symbol>> = BTreeMap::new();
        let mut seen: BTreeSet<&SymbolId> = BTreeSet::new();
        let members: BTreeSet<&SymbolId> = unit.iter().map(|s| &s.id).collect();
        for id in unit.iter().flat_map(|s| project.uses(&s.id)) {
            if members.contains(id) || !seen.insert(id) {
                continue;
            }
            let Some(used) = project.symbol(id) else {
                continue;
            };
            let module = id.module();
            let skipped = project.in_test(id)
                || project.in_example(id)
                || project.file(&used.file).is_some_and(|f| f.generated)
                || (module != home && layout::contains(module, home));
            if !skipped {
                by_module.entry(module.to_owned()).or_default().push(used);
            }
        }
        let total = by_module.values().map(Vec::len).sum();
        Self { by_module, total }
    }

    /// The functions, methods and fields of `module` it uses: behavior ties a
    /// function to a module, naming its constants and types does not.
    pub(super) fn behavior(&self, module: &str) -> usize {
        self.by_module.get(module).map_or(0, |s| {
            s.iter()
                .filter(|u| {
                    matches!(
                        u.kind,
                        SymbolKind::Function | SymbolKind::Method | SymbolKind::Field
                    )
                })
                .count()
        })
    }

    pub(super) fn count(&self, module: &str) -> usize {
        self.by_module.get(module).map_or(0, Vec::len)
    }

    pub(super) fn own(&self, home: &str) -> usize {
        self.count(home)
    }
}

/// The import graph among the modules of the project.
pub(super) struct Imports {
    pub(super) edges: BTreeMap<String, BTreeSet<String>>,
}

impl Imports {
    pub(super) fn of(project: &dyn Model) -> Self {
        let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for edge in project.edges().filter(|e| e.kind == EdgeKind::Imports) {
            let from = match &edge.from {
                Node::Module(path) => path.clone(),
                Node::Symbol(id) => id.module().to_owned(),
            };
            let to = match &edge.to {
                Target::Resolved(Node::Module(path)) => path.clone(),
                Target::Resolved(Node::Symbol(id)) => id.module().to_owned(),
                Target::Path(path) => path.clone(),
            };
            if from != to {
                edges.entry(from).or_default().insert(to);
            }
        }
        Self { edges }
    }

    pub(super) fn direct(&self, from: &str, to: &str) -> bool {
        self.edges.get(from).is_some_and(|s| s.contains(to))
    }

    /// Whether `from` imports `to`, directly or through other modules.
    pub(super) fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(at) = stack.pop() {
            for next in self.edges.get(at).into_iter().flatten() {
                if next == to {
                    return true;
                }
                if seen.insert(next.as_str()) {
                    stack.push(next.as_str());
                }
            }
        }
        false
    }
}

pub(super) fn share(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// The function and the private functions only it and its other helpers use,
/// transitively: what has to move together. The function comes first.
pub(super) fn unit<'p>(project: &'p dyn Model, symbol: &'p Symbol) -> Vec<&'p Symbol> {
    let mut members: Vec<&Symbol> = vec![symbol];
    loop {
        let ids: BTreeSet<&SymbolId> = members.iter().map(|s| &s.id).collect();
        let next = members
            .iter()
            .flat_map(|m| project.uses(&m.id))
            .filter(|id| !ids.contains(id))
            .filter_map(|id| project.symbol(id))
            .find(|h| {
                h.kind == SymbolKind::Function
                    && h.owner.is_none()
                    && h.visibility == lighthouse_model::ucm::Visibility::Private
                    && h.id.module() == symbol.id.module()
                    && project.references(&h.id).is_empty()
                    && !project.in_test(&h.id)
                    && {
                        let callers: Vec<&SymbolId> = project
                            .callers(&h.id)
                            .iter()
                            .filter(|c| !project.in_test(c))
                            .collect();
                        !callers.is_empty() && callers.iter().all(|c| ids.contains(*c))
                    }
            });
        match next {
            Some(helper) => members.push(helper),
            None => return members,
        }
    }
}

/// The modules of the project types the signature of a function names, and
/// whether it names a type of the standard library or a dependency (`io.Writer`,
/// `std::fmt::Formatter`) or renders text: such a function is an adapter to
/// that library or a view.
pub(super) fn signature(project: &dyn Model, symbol: &Symbol) -> (BTreeSet<String>, bool) {
    let mut modules = BTreeSet::new();
    let mut external = false;
    let Some(summary) = project.function(&symbol.id) else {
        return (modules, external);
    };
    // A function that renders a foreign value as text, or writes text into a
    // buffer, is a view: it belongs to the layer that presents, not to the
    // module of what it presents.
    let text = |t: &str| {
        matches!(
            t.trim_start_matches(['*', '&']).trim_start_matches("mut "),
            "String" | "str" | "string"
        )
    };
    external |= summary.signature.results.iter().any(|r| text(&r.text))
        || summary
            .signature
            .params
            .iter()
            .any(|p| text(&p.text) && p.text.contains("mut"));
    for ty in summary
        .signature
        .params
        .iter()
        .chain(&summary.signature.results)
    {
        match ty.symbol.as_deref().and_then(|id| id.split_once("::")) {
            Some((module, _)) => {
                modules.insert(module.to_owned());
            }
            None => {
                let bare = ty
                    .text
                    .trim_start_matches(['*', '&'])
                    .trim_start_matches("mut ")
                    .trim_start_matches("...");
                let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | ':');
                external |= (bare.contains('.') || bare.contains("::")) && bare.chars().all(word);
            }
        }
    }
    (modules, external)
}
