//! The `owned` fact of a test: the symbol it is the owner test of.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{Symbol, SymbolId, SymbolKind};
use serde_json::Value;

use super::Builder;

impl Builder<'_> {
    /// The node of the symbol that `test` is credited to by the naming
    /// convention (`TestGet` owns `Get`), the one in the source file the test
    /// file is named after when there are several; null for a test that owns
    /// nothing. Only a test has it.
    pub(super) fn owned(&self, test: &Symbol) -> Value {
        let index = self.indexes.owned.get_or_init(|| self.owned_index());
        let Some(found) = index.get(&test.id) else {
            return Value::Null;
        };
        let stem = test.file.file_stem().and_then(|s| s.to_str());
        let stem = stem
            .and_then(|s| s.strip_suffix("_test"))
            .unwrap_or_default();
        let paired = found.iter().find(|id| {
            self.project
                .symbol(id)
                .is_some_and(|s| s.file.file_stem().and_then(|f| f.to_str()) == Some(stem))
        });
        paired
            .or(found.first())
            .and_then(|id| self.project.symbol(id))
            .map_or(Value::Null, |s| self.node(s))
    }

    /// Per test, the symbols it is credited to, in the order of the symbols.
    fn owned_index(&self) -> BTreeMap<SymbolId, Vec<SymbolId>> {
        let mut index: BTreeMap<SymbolId, Vec<SymbolId>> = BTreeMap::new();
        let owners = self.project.symbols.iter().filter(|s| {
            matches!(
                s.kind,
                SymbolKind::Function
                    | SymbolKind::Method
                    | SymbolKind::Type
                    | SymbolKind::Interface
            ) && !self.project.in_test(&s.id)
        });
        for symbol in owners {
            for (case, _) in self.naming.credited_tests(symbol) {
                index
                    .entry(case.symbol.clone())
                    .or_default()
                    .push(symbol.id.clone());
            }
        }
        index
    }
}
