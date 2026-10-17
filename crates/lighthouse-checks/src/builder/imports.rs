//! The `module_imports` fact: what the module of a symbol imports, as the
//! import paths its files write.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{EdgeKind, Node, Project, Symbol, Target};
use serde_json::{Value, json};

use super::Builder;

impl Builder<'_> {
    /// The distinct import paths of the files of the symbol's module, sorted.
    pub(super) fn module_imports(&self, symbol: &Symbol) -> Value {
        let imports = self.indexes.imports.get_or_init(|| index(self.project));
        json!(
            imports
                .get(symbol.id.module())
                .map_or(&[][..], Vec::as_slice)
        )
    }
}

/// The import paths of every module.
pub(super) fn index(project: &Project) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in project.edges.iter().filter(|e| e.kind == EdgeKind::Imports) {
        let Node::Module(from) = &edge.from else {
            continue;
        };
        let to = match &edge.to {
            Target::Path(path) | Target::Resolved(Node::Module(path)) => path,
            Target::Resolved(Node::Symbol(_)) => continue,
        };
        found.entry(from.clone()).or_default().push(to.clone());
    }
    for paths in found.values_mut() {
        paths.sort();
        paths.dedup();
    }
    found
}
