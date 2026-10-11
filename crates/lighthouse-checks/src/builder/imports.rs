//! The `module_imports` fact: what the module of a symbol imports, as the
//! import paths its files write.

use std::{collections::BTreeMap, sync::OnceLock};

use lighthouse_model::{EdgeKind, Node, Project, Symbol, Target};
use serde_json::{Value, json};

use super::Builder;

/// The import paths of every module, built by the first check that asks and
/// shared by the others.
#[derive(Default)]
pub(crate) struct Imports(OnceLock<BTreeMap<String, Vec<String>>>);

impl Builder<'_> {
    /// The distinct import paths of the files of the symbol's module, sorted.
    pub(super) fn module_imports(&self, symbol: &Symbol) -> Value {
        let imports = self.imports.0.get_or_init(|| index(self.project));
        json!(
            imports
                .get(symbol.id.module())
                .map_or(&[][..], Vec::as_slice)
        )
    }
}

fn index(project: &Project) -> BTreeMap<String, Vec<String>> {
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
