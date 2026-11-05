//! The `homonyms` of a type: the exported types and interfaces of other
//! modules that carry its name.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{Model, Symbol, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::Builder;

impl Builder<'_> {
    /// The ids of the exported, non-test, non-generated types and interfaces
    /// of other modules with the name of `symbol`; empty for anything else.
    pub(super) fn homonyms(&self, symbol: &Symbol) -> Value {
        let found = if exported_type(symbol) {
            self.indexes.names(&symbol.name)
        } else {
            Vec::new()
        };
        json!(
            found
                .iter()
                .filter(|id| id.module() != symbol.id.module())
                .map(SymbolId::as_str)
                .collect::<Vec<_>>()
        )
    }
}

/// The exported types of the project by name.
pub fn names(project: &dyn Model) -> BTreeMap<String, Vec<SymbolId>> {
    let mut found: BTreeMap<String, Vec<SymbolId>> = BTreeMap::new();
    let types = project
        .symbols()
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
