//! The `gap` of a method: what separates it from the previous method of its
//! owner in the file.

use lighthouse_model::ucm::{Symbol, SymbolKind};
use serde_json::{Value, json};

use super::Builder;
use crate::layout::{declarations_in, owner_key};

impl Builder<'_> {
    /// `{ previous, between }`: the id of the previous method of the same
    /// owner among the declarations of the file (null for the first method of
    /// an owner and for what is no method), and the declarations strictly
    /// between the two.
    pub(super) fn gap(&self, symbol: &Symbol) -> Value {
        let none = json!({ "previous": null, "between": [] });
        let owner = owner_key(symbol).filter(|_| symbol.kind == SymbolKind::Method);
        let Some(file) = owner.as_ref().and_then(|_| self.project.file(&symbol.file)) else {
            return none;
        };
        let modules = declarations_in(self.project, file);
        let Some(module) = modules
            .iter()
            .find(|module| module.iter().any(|s| s.id == symbol.id))
        else {
            return none;
        };
        let at = module.iter().position(|s| s.id == symbol.id).unwrap_or(0);
        let Some(before) = module[..at]
            .iter()
            .rposition(|s| s.kind == SymbolKind::Method && owner_key(s) == owner)
        else {
            return none;
        };
        json!({
            "previous": module[before].id.as_str(),
            "between": module[before + 1..at].iter().map(|s| self.node(s)).collect::<Vec<_>>(),
        })
    }
}
