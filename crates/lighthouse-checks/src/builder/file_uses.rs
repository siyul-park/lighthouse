//! The facts of what a symbol is tied to: who calls it and what it calls or
//! references in its own file.

use lighthouse_model::ucm::{Symbol, SymbolId};
use serde_json::{Map, Value, json};

use super::Builder;
use crate::library::Needs;

impl Builder<'_> {
    /// `local_callers` and `file_uses`, each when `needs` asks for it.
    pub(super) fn tie_facts(&self, map: &mut Map<String, Value>, symbol: &Symbol, needs: &Needs) {
        if needs.field("local_callers") {
            map.insert("local_callers".to_owned(), self.local_callers(symbol));
        }
        if needs.field("file_uses") {
            map.insert("file_uses".to_owned(), self.file_uses(symbol));
        }
    }

    /// The ids of the symbols of the same file that `symbol` calls or
    /// references, tests aside, in id order: the edges within one file, which
    /// the neighbors digest of the file holds.
    fn file_uses(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        let mut ids: Vec<&SymbolId> = project
            .uses(&symbol.id)
            .iter()
            .filter(|id| **id != symbol.id && !project.in_test(id))
            .filter(|id| project.symbol(id).is_some_and(|s| s.file == symbol.file))
            .collect();
        ids.sort();
        json!(ids.into_iter().map(SymbolId::as_str).collect::<Vec<_>>())
    }
}
