//! The `split` of a method declared away from its type's file: how far the
//! file it lives in is dedicated to the type, and how the type's methods are
//! spread.
//!
//! For a type `T` declared in `H`, a file `F` other than `H`, `M` the methods
//! of `T` in `F` and `L` the helpers local to `F` (private functions and
//! private types with their methods, used only from `F`):
//!
//! - `share`: (`|M|` plus the functions of the helpers only `M` reaches) over
//!   all functions of `F`;
//! - `home`: the file that declares `T`;
//! - `home_methods`: the methods of `T` declared in `H`;
//! - `methods`: all the methods of `T`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::ucm::{Model, Symbol, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::Builder;
use crate::fact::Fact;

impl Builder<'_> {
    /// The `split` of a method that lives away from the file of its owner
    /// type; `{}` for any other symbol.
    pub(super) fn split(&self, symbol: &Symbol) -> Value {
        self.indexes
            .split(&symbol.id)
            .map_or_else(|| json!({}), |split| split.json())
    }
}

/// What one file declares: its functions, and the helpers local to it, each a
/// unit of symbols that stands or falls together.
struct Local<'p> {
    file: &'p Path,
    functions: usize,
    units: Vec<Vec<&'p Symbol>>,
    unit_of: BTreeMap<&'p SymbolId, usize>,
}

impl<'p> Local<'p> {
    fn of(project: &'p dyn Model, file: &'p Path) -> Self {
        let declared: Vec<&Symbol> = project
            .symbols_in(file)
            .filter(|s| !project.in_test(&s.id))
            .collect();
        let functions = declared
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
            .count();
        let mut units: Vec<Vec<&Symbol>> = Vec::new();
        for symbol in declared
            .iter()
            .filter(|s| s.visibility == Visibility::Private)
        {
            match symbol.kind {
                SymbolKind::Function => units.push(vec![symbol]),
                SymbolKind::Type => {
                    let mut unit = vec![*symbol];
                    unit.extend(
                        project
                            .members(&symbol.id)
                            .iter()
                            .filter_map(|id| project.symbol(id))
                            .filter(|m| m.kind == SymbolKind::Method && m.file == file),
                    );
                    units.push(unit);
                }
                _ => {}
            }
        }
        let mut local = Self {
            file,
            functions,
            units,
            unit_of: BTreeMap::new(),
        };
        local.keep_local(project);
        local
    }

    /// Drops the units something outside the file uses, or nothing uses, and
    /// indexes the rest.
    fn keep_local(&mut self, project: &'p dyn Model) {
        let file = self.file;
        self.units.retain(|unit| {
            let users = users(project, unit);
            !users.is_empty()
                && users
                    .iter()
                    .all(|id| project.symbol(id).is_some_and(|s| s.file == file))
        });
        self.unit_of = self
            .units
            .iter()
            .enumerate()
            .flat_map(|(at, unit)| unit.iter().map(move |s| (&s.id, at)))
            .collect();
    }

    fn facts(&self, project: &dyn Model, methods: &[&Symbol], ty: &SymbolId, home: &Path) -> Value {
        let own: BTreeSet<&SymbolId> = methods.iter().map(|m| &m.id).collect();
        let reached = self.reached_only_from(project, &own);
        let helper_functions: usize = reached
            .iter()
            .flat_map(|&at| &self.units[at])
            .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
            .count();
        let share = (methods.len() + helper_functions) as f64 / self.functions.max(1) as f64;
        let all: Vec<&Symbol> = project
            .members(ty)
            .iter()
            .filter_map(|id| project.symbol(id))
            .filter(|m| m.kind == SymbolKind::Method && !project.in_test(&m.id))
            .collect();
        json!({
            "share": share,
            "home": home.to_string_lossy(),
            "home_methods": all.iter().filter(|m| m.file == home).count(),
            "methods": all.len(),
        })
    }

    /// The units whose every user is a method of `own` or another such unit,
    /// and which `own` reaches.
    fn reached_only_from(&self, project: &dyn Model, own: &BTreeSet<&SymbolId>) -> BTreeSet<usize> {
        let mut only: BTreeSet<usize> = (0..self.units.len()).collect();
        loop {
            let out: Vec<usize> = only
                .iter()
                .copied()
                .filter(|&at| {
                    !users(project, &self.units[at]).iter().all(|user| {
                        own.contains(user)
                            || self.unit_of.get(user).is_some_and(|u| only.contains(u))
                    })
                })
                .collect();
            if out.is_empty() {
                break;
            }
            for at in out {
                only.remove(&at);
            }
        }
        // A ring of helpers that nothing of `own` calls is not reached.
        let mut reached = BTreeSet::new();
        let mut frontier: Vec<&SymbolId> = own.iter().copied().collect();
        while let Some(id) = frontier.pop() {
            for used in project.uses(id) {
                if let Some(&at) = self.unit_of.get(used)
                    && only.contains(&at)
                    && reached.insert(at)
                {
                    frontier.extend(self.units[at].iter().map(|s| &s.id));
                }
            }
        }
        reached
    }
}

/// The `split` of every method away from its type's file.
pub fn splits(project: &dyn Model) -> BTreeMap<SymbolId, Fact> {
    let mut away: BTreeMap<(&SymbolId, &Path), Vec<&Symbol>> = BTreeMap::new();
    let types = project
        .symbols()
        .filter(|s| s.kind == SymbolKind::Type && !project.in_test(&s.id));
    for ty in types {
        for method in project
            .members(&ty.id)
            .iter()
            .filter_map(|id| project.symbol(id))
            .filter(|m| m.kind == SymbolKind::Method && m.file != ty.file)
            .filter(|m| !project.in_test(&m.id))
        {
            away.entry((&ty.id, method.file.as_path()))
                .or_default()
                .push(method);
        }
    }
    let mut files: BTreeMap<&Path, Local> = BTreeMap::new();
    let mut found = BTreeMap::new();
    for ((ty, file), methods) in away {
        let local = files
            .entry(file)
            .or_insert_with(|| Local::of(project, file));
        let Some(home) = project.symbol(ty).map(|t| t.file.as_path()) else {
            continue;
        };
        let value = Fact::from(&local.facts(project, &methods, ty, home));
        for method in methods {
            found.insert(method.id.clone(), value.clone());
        }
    }
    found
}

/// The distinct symbols that call or reference any symbol of the unit, the
/// unit's own symbols aside, test code aside.
fn users<'p>(project: &'p dyn Model, unit: &[&Symbol]) -> BTreeSet<&'p SymbolId> {
    unit.iter()
        .flat_map(|s| {
            project
                .callers(&s.id)
                .iter()
                .chain(project.references(&s.id))
        })
        .filter(|id| !unit.iter().any(|s| s.id == **id) && !project.in_test(id))
        .collect()
}
