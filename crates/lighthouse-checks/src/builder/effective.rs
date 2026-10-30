//! The `param_structs` fact: the structs that exist only for a function and
//! that its parameters name, with whether each of their fields is optional.
//!
//! A struct that exists only for one function (nothing else takes it, nothing
//! else touches it or its fields but that function and the code that calls it)
//! is a parameter list in disguise. Counting the struct as one parameter would
//! let a long list pass by being wrapped; a decision counts its fields instead.

use std::collections::BTreeMap;

use lighthouse_model::ucm::{FunctionSummary, Project, Symbol, SymbolId, SymbolKind};
use serde_json::{Value, json};

use super::Builder;

/// Who takes and touches which type.
#[derive(Default)]
pub(super) struct Index {
    /// Functions that name a type in their parameters, by the type's
    /// kind-less id.
    takers: BTreeMap<String, Vec<SymbolId>>,
    /// Symbols that call or reference each symbol through a resolved edge.
    users: BTreeMap<SymbolId, Vec<SymbolId>>,
}

/// A struct that exists only for one function: its name, and for each field
/// whether a caller may leave it out.
struct Hidden {
    name: String,
    optional: Vec<bool>,
}

impl Builder<'_> {
    /// `[{struct, fields: [optional]}]` for a function: one entry per
    /// parameter that is a single-use struct (once per parameter).
    pub(super) fn param_structs(
        &self,
        function: &Symbol,
        summary: Option<&FunctionSummary>,
    ) -> Value {
        let Some(summary) = summary else {
            return json!([]);
        };
        let index = self.indexes.single.get_or_init(|| index(self.project));
        Value::Array(
            named_params(summary)
                .filter_map(|ty| self.hidden(index, function, ty))
                .map(|h| json!({ "struct": h.name, "fields": h.optional }))
                .collect(),
        )
    }

    /// The struct `ty` names when it exists only for `function`.
    fn hidden(&self, index: &Index, function: &Symbol, ty: &str) -> Option<Hidden> {
        let project = self.project;
        // A struct is a `type`; where an interface lists only fields (a
        // TypeScript options interface) it is the same parameter list.
        let (id, symbol) = ["type", "interface"].into_iter().find_map(|kind| {
            let id = SymbolId::parse(&format!("{ty}#{kind}"))?;
            project.symbol(&id).map(|symbol| (id.clone(), symbol))
        })?;
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
            optional: fields.iter().map(|f| f.optional).collect(),
        })
    }
}

/// The project types the parameters name, as kind-less ids, one per parameter
/// in order (a type taken twice appears twice).
pub(super) fn named_params(summary: &FunctionSummary) -> impl Iterator<Item = &str> {
    summary
        .signature
        .params
        .iter()
        .filter_map(|p| p.symbol.as_deref())
}

fn index(project: &Project) -> Index {
    let mut index = Index::default();
    for summary in &project.functions {
        for ty in named_params(summary) {
            let takers: &mut Vec<SymbolId> = index.takers.entry(ty.to_owned()).or_default();
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
