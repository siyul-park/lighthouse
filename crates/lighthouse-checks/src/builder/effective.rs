//! The `effective` fact: how many parameters and results a function really
//! has once the structs that exist only for it are counted by their fields.
//!
//! A struct that exists only for one function (nothing else takes or returns
//! it, nothing else touches it or its fields but that function and the code
//! that calls it) is a parameter list or a result list in disguise. Counting
//! the struct as one parameter would let a long list pass by being wrapped.

use std::{collections::BTreeMap, sync::OnceLock};

use lighthouse_model::{FunctionSummary, Project, Symbol, SymbolId, SymbolKind};
use serde_json::{Value, json};

use super::Builder;

/// Which side of a signature a hidden struct is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Params,
    Results,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Params => "params",
            Self::Results => "results",
        }
    }
}

/// The run's index of who takes, returns and touches which type, built by the
/// first check that asks and shared by the others.
#[derive(Default)]
pub(crate) struct Single(OnceLock<Index>);

#[derive(Default)]
struct Index {
    /// Functions that name a type in their parameters or results, by the
    /// type's kind-less id.
    signatures: BTreeMap<String, Vec<SymbolId>>,
    /// Symbols that call or reference each symbol through a resolved edge.
    users: BTreeMap<SymbolId, Vec<SymbolId>>,
}

impl Builder<'_> {
    /// `{params, results, hidden: [{struct, fields, side}]}` for a function.
    pub(super) fn effective(&self, function: &Symbol, summary: Option<&FunctionSummary>) -> Value {
        let Some(summary) = summary else {
            return json!({ "params": 0, "results": 0, "hidden": [] });
        };
        let index = self.single.0.get_or_init(|| index(self.project));
        let mut hidden = Vec::new();
        for (side, types) in [
            (Side::Params, &summary.param_types),
            (Side::Results, &summary.result_types),
        ] {
            for ty in types {
                if let Some(found) = self.hidden(index, function, ty, side) {
                    hidden.push(found);
                }
            }
        }
        let total = |side: Side, base: u32| {
            let of_side = hidden.iter().filter(|h| h.side == side);
            let removed = of_side.clone().count();
            let added: usize = of_side.map(|h| h.fields).sum();
            let kept = (base as usize).saturating_sub(removed);
            kept + added
        };
        json!({
            "params": total(Side::Params, summary.params),
            "results": total(Side::Results, summary.returns),
            "hidden": hidden.iter().map(|h| json!({
                "struct": h.name,
                "fields": h.fields,
                "side": h.side.name(),
            })).collect::<Vec<_>>(),
        })
    }

    /// The struct `ty` names when it exists only for `function`.
    fn hidden(&self, index: &Index, function: &Symbol, ty: &str, side: Side) -> Option<Hidden> {
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
        let takers = index.signatures.get(ty)?;
        if takers.as_slice() != [function.id.clone()] {
            return None;
        }
        let allowed = |user: &SymbolId| {
            user == &function.id
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
            side,
        })
    }

    /// All the fields of the struct; an options-like struct (its name ends in
    /// one of `allowSuffixes`) counts only the fields a caller must give.
    fn counted_fields(&self, name: &str, fields: &[&Symbol]) -> usize {
        let lenient = self.allow_suffixes.iter().any(|s| name.ends_with(s));
        fields.iter().filter(|f| !(lenient && f.optional)).count()
    }
}

struct Hidden {
    name: String,
    fields: usize,
    side: Side,
}

fn index(project: &Project) -> Index {
    let mut index = Index::default();
    for summary in &project.functions {
        for ty in summary.param_types.iter().chain(&summary.result_types) {
            let takers: &mut Vec<SymbolId> = index.signatures.entry(ty.clone()).or_default();
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
