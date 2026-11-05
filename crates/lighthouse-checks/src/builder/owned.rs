//! The `owned` fact of a test: the symbol it is the owner test of, by its name
//! and by what it uses.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{Model, Symbol, SymbolId, SymbolKind};
use lighthouse_plugin::Workspace;
use serde_json::Value;

use super::Builder;
use crate::{facts, naming::Tests};

impl Builder<'_> {
    /// The node of the symbol that `test` is credited to by the naming
    /// convention (`TestGet` owns `Get`) and that it uses, itself or through a
    /// member, when the symbol is not generated; the one in the source file the test
    /// file is named after when there are several; null for a test that owns
    /// nothing. Only a test has it.
    pub(super) fn owned(&self, test: &Symbol) -> Value {
        let found = self.indexes.owned(&test.id);
        if found.is_empty() {
            return Value::Null;
        }
        let stem = facts::file_stem(self.project, &test.file);
        let stem = stem.strip_suffix("_test").unwrap_or_default();
        let in_pair = |id: &SymbolId| {
            self.project
                .symbol(id)
                .is_some_and(|s| facts::file_stem(self.project, &s.file) == stem)
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
}

/// Per test, the symbols it is credited to, in the order of their ids. Each
/// owner is judged by the naming convention of its own language, not of the
/// file whose check built it first.
pub fn owned(model: &dyn Model, ws: &Workspace) -> BTreeMap<SymbolId, Vec<SymbolId>> {
    let mut conventions: BTreeMap<String, Tests<'_>> = BTreeMap::new();
    let mut index: BTreeMap<SymbolId, Vec<SymbolId>> = BTreeMap::new();
    let owners = model.symbols().filter(|s| {
        matches!(
            s.kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Type | SymbolKind::Interface
        ) && !model.in_test(&s.id)
    });
    for symbol in owners {
        let Some(file) = model.file(&symbol.file).filter(|f| !f.generated) else {
            continue;
        };
        let language = file.analysis_language();
        let naming = conventions
            .entry(language.to_owned())
            .or_insert_with(|| Tests::alone(ws.test_naming(language), model));
        for (case, _) in naming.credited_tests(symbol) {
            if !reaches(model, &case.symbol, symbol) {
                continue;
            }
            index
                .entry(case.symbol.clone())
                .or_default()
                .push(symbol.id.clone());
        }
    }
    // The symbols come in the order the providers answered in, which is
    // not the same from run to run; a test that is credited to several
    // takes the same one every time.
    index.values_mut().for_each(|owners| owners.sort());
    index
}

/// Whether the test uses the symbol, or a member of it when it is a type:
/// a name that matches is not enough to own it.
fn reaches(model: &dyn Model, test: &SymbolId, target: &Symbol) -> bool {
    let uses = model.uses(test);
    uses.contains(&target.id)
        || model
            .members(&target.id)
            .iter()
            .any(|member| uses.contains(member))
}
