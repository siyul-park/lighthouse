//! The `homonyms` of a type: the exported types and interfaces of other
//! modules that carry its name.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{Project, Symbol, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::Builder;

impl Builder<'_> {
    /// The ids of the exported, non-test, non-generated types and interfaces
    /// of other modules with the name of `symbol`; empty for anything else.
    pub(super) fn homonyms(&self, symbol: &Symbol) -> Value {
        let named = self.indexes.names.get_or_init(|| index(self.project));
        let found = named.get(&symbol.name).filter(|_| exported_type(symbol));
        json!(
            found
                .into_iter()
                .flatten()
                .filter(|id| id.module() != symbol.id.module())
                .map(SymbolId::as_str)
                .collect::<Vec<_>>()
        )
    }
}

/// The exported types of the project by name.
pub(super) fn index(project: &Project) -> BTreeMap<String, Vec<SymbolId>> {
    let mut found: BTreeMap<String, Vec<SymbolId>> = BTreeMap::new();
    let types = project
        .symbols
        .iter()
        .filter(|s| exported_type(s))
        .filter(|s| !project.in_test(&s.id))
        .filter(|s| project.file(&s.file).is_some_and(|f| !f.generated));
    for symbol in types {
        found
            .entry(symbol.name.clone())
            .or_default()
            .push(symbol.id.clone());
    }
    found
}

fn exported_type(symbol: &Symbol) -> bool {
    matches!(symbol.kind, SymbolKind::Type | SymbolKind::Interface)
        && symbol.visibility == Visibility::Public
}
