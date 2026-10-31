//! The `owned` fact of a test: the symbol it is the owner test of, by its name
//! and by what it uses.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{Symbol, SymbolId, SymbolKind};
use serde_json::Value;

use super::Builder;

impl Builder<'_> {
    /// The node of the symbol that `test` is credited to by the naming
    /// convention (`TestGet` owns `Get`) and that it uses, itself or through a
    /// member, when the symbol is not generated; the one in the source file the test
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
        let in_pair = |id: &SymbolId| {
            self.project
                .symbol(id)
                .is_some_and(|s| s.file.file_stem().and_then(|f| f.to_str()) == Some(stem))
        };
        let paired = found.iter().find(|id| in_pair(id));
        // A test that uses something its own file's source declares exercises
        // that file; the name that matches elsewhere does not move it.
        if paired.is_none() && self.project.uses(&test.id).iter().any(in_pair) {
            return Value::Null;
        }
        paired
            .or(found.first())
            .and_then(|id| self.project.symbol(id))
            .map_or(Value::Null, |s| self.node(s))
    }

    /// Whether the test uses the symbol, or a member of it when it is a type:
    /// a name that matches is not enough to own it.
    fn reaches(&self, test: &SymbolId, target: &Symbol) -> bool {
        let uses = self.project.uses(test);
        uses.contains(&target.id)
            || self
                .project
                .members(&target.id)
                .iter()
                .any(|member| uses.contains(member))
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
            if self.project.file(&symbol.file).is_none_or(|f| f.generated) {
                continue;
            }
            for (case, _) in self.naming.credited_tests(symbol) {
                if !self.reaches(&case.symbol, symbol) {
                    continue;
                }
                index
                    .entry(case.symbol.clone())
                    .or_default()
                    .push(symbol.id.clone());
            }
        }
        index
    }
}
