//! The facts of what a symbol is tied to: who calls it and what it calls or
//! references in its own file.

use std::rc::Rc;

use lighthouse_model::ucm::{LoadNeed, LoadPoint, Symbol, SymbolId};
use serde_json::{Map, Value, json};

use super::Builder;
use crate::library::Needs;

impl Builder<'_> {
    /// `local_callers` and `file_uses`, each when `needs` asks for it.
    pub(super) fn tie_facts(&self, map: &mut Map<String, Value>, symbol: &Symbol, needs: &Needs) {
        if needs.field("local_callers") {
            map.insert("local_callers".to_owned(), self.local_callers(symbol));
        }
        if needs.field("load_after") {
            map.insert("load_after".to_owned(), self.load_after(symbol));
        }
        if needs.field("load_before") {
            map.insert("load_before".to_owned(), self.load_before(symbol));
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

impl Builder<'_> {
    /// The declarations `symbol` must stay below, as `{id, pos}`: the bindings
    /// its code needs as the file loads.
    fn load_after(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        let mut found: Vec<Value> = self
            .loads(symbol)
            .iter()
            .filter(|n| n.user == LoadPoint::Declaration(symbol.id.clone()))
            .filter_map(|n| project.symbol(&n.binding))
            .map(|s| json!({"id": s.id.as_str(), "pos": crate::facts::pos(crate::facts::span_of(project, &s.id).start)}))
            .collect();
        found.sort_by_key(|v| v["pos"].as_u64());
        Value::Array(found)
    }

    /// The code that must stay below `symbol`, as `{id, pos}`: the declarations
    /// and the top-level code that need it as the file loads (`id` is empty
    /// for top-level code).
    fn load_before(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        let mut found: Vec<Value> = self
            .loads(symbol)
            .iter()
            .filter(|n| n.binding == symbol.id)
            .filter_map(|n| match &n.user {
                LoadPoint::Declaration(id) => project
                    .symbol(id)
                    .map(|s| (id.as_str(), crate::facts::span_of(project, &s.id).start)),
                LoadPoint::Site(span) => Some(("", span.start)),
            })
            .map(|(id, at)| json!({"id": id, "pos": crate::facts::pos(at)}))
            .collect();
        found.sort_by_key(|v| v["pos"].as_u64());
        Value::Array(found)
    }

    /// What the file of `symbol` needs of its declarations as it loads, in a
    /// language that evaluates them in order; nothing in another.
    fn loads(&self, symbol: &Symbol) -> Rc<Vec<LoadNeed>> {
        let mut loads = self.loads.borrow_mut();
        let found = loads
            .entry(symbol.file.clone())
            .or_insert_with(|| Rc::new(self.project.load_needs(&symbol.file)));
        Rc::clone(found)
    }
}
