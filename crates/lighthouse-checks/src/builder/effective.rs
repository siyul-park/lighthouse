//! The `effective` fact: how many parameters a function really has once the
//! structs that exist only for it are counted by their fields.
//!
//! A struct that exists only for one function (nothing else takes it, nothing
//! else touches it or its fields but that function and the code that calls it)
//! is a parameter list in disguise. Counting the struct as one parameter would
//! let a long list pass by being wrapped.

use std::{collections::BTreeMap, sync::OnceLock};

use lighthouse_model::{FunctionSummary, Project, Symbol, SymbolId, SymbolKind};
use serde_json::{Value, json};

use super::Builder;

/// The run's index of who takes and touches which type, built by the first
/// check that asks and shared by the others.
#[derive(Default)]
pub(crate) struct Single(OnceLock<Index>);

#[derive(Default)]
struct Index {
    /// Functions that name a type in their parameters, by the type's
    /// kind-less id.
    takers: BTreeMap<String, Vec<SymbolId>>,
    /// Symbols that call or reference each symbol through a resolved edge.
    users: BTreeMap<SymbolId, Vec<SymbolId>>,
}

/// A struct that exists only for one function, and how many fields it counts.
struct Hidden {
    name: String,
    fields: usize,
}

impl Builder<'_> {
    /// `{params, hidden: [{struct, fields}]}` for a function: `params` is the
    /// declared count with each parameter that is a single-use struct (once
    /// per parameter) replaced by the fields that count.
    pub(super) fn effective(&self, function: &Symbol, summary: Option<&FunctionSummary>) -> Value {
        let Some(summary) = summary else {
            return json!({ "params": 0, "hidden": [] });
        };
        let index = self.single.0.get_or_init(|| index(self.project));
        let hidden: Vec<Hidden> = summary
            .param_types
            .iter()
            .filter_map(|ty| self.hidden(index, function, ty))
            .collect();
        let added: usize = hidden.iter().map(|h| h.fields).sum();
        let kept = (summary.params as usize).saturating_sub(hidden.len());
        json!({
            "params": kept + added,
            "hidden": hidden
                .iter()
                .map(|h| json!({ "struct": h.name, "fields": h.fields }))
                .collect::<Vec<_>>(),
        })
    }

    /// The struct `ty` names when it exists only for `function`.
    fn hidden(&self, index: &Index, function: &Symbol, ty: &str) -> Option<Hidden> {
        let project = self.project;
        let id = SymbolId::parse(&format!("{ty}#type"))?;
        let symbol = project.symbol(&id)?;
        let members = project.members(&id);
        let fields: Vec<&Symbol> = members
            .iter()
            .filter_map(|m| project.symbol(m))
            .filter(|m| m.kind == SymbolKind::Field)
            .collect();
        if fields.is_empty() || fields.len() != members.len() {
            return None;
        }
        if !matches!(index.takers.get(ty)?.as_slice(), [only] if *only == function.id) {
            return None;
        }
        let allowed = |user: &SymbolId| {
            *user == function.id
                || project.callers(&function.id).contains(user)
                || project.in_test(user)
        };
        let touched = std::iter::once(&id).chain(fields.iter().map(|f| &f.id));
        let others = touched
            .filter_map(|t| index.users.get(t))
            .flatten()
            .any(|user| !allowed(user));
        (!others).then(|| Hidden {
            name: symbol.name.clone(),
            fields: self.counted_fields(&symbol.name, &fields),
        })
    }

    /// All the fields of the struct; an options-like struct (its name ends in
    /// one of `allowSuffixes`) counts only the fields a caller must give.
    fn counted_fields(&self, name: &str, fields: &[&Symbol]) -> usize {
        let lenient = self.allow_suffixes.iter().any(|s| name.ends_with(s));
        fields.iter().filter(|f| !(lenient && f.optional)).count()
    }
}

fn index(project: &Project) -> Index {
    let mut index = Index::default();
    for summary in &project.functions {
        for ty in &summary.param_types {
            let takers: &mut Vec<SymbolId> = index.takers.entry(ty.clone()).or_default();
            if !takers.contains(&summary.symbol) {
                takers.push(summary.symbol.clone());
            }
        }
    }
    for symbol in &project.symbols {
        for used in project.uses(&symbol.id) {
            index
                .users
                .entry(used.clone())
                .or_default()
                .push(symbol.id.clone());
        }
    }
    index
}
