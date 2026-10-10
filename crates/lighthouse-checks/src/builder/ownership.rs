//! The facts about which type or module a function belongs with:
//! `receiver_affinity`, `owner_home` and `envy`. They share one way to find the
//! owner of a symbol that a function uses and the owner its callers have.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::{Project, Symbol, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::Builder;
use crate::layout;

impl Builder<'_> {
    /// A private free function whose every production caller is a method of
    /// one owner type, with how it relates to that owner.
    pub(super) fn receiver_affinity(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        let Some(owner) = self.caller_owner(symbol) else {
            return json!({});
        };
        let bare = owner
            .rsplit_once('#')
            .map_or(owner.as_str(), |(head, _)| head);
        let takes_owner_param = project
            .function(&symbol.id)
            .is_some_and(|f| f.param_types.iter().any(|t| t == bare));
        let uses_owner = uses_owner(project, symbol, &owner);
        let owner_name = bare.rsplit("::").next().unwrap_or_default();
        json!({
            "owner": owner,
            "owner_name": owner_name,
            "callers": self.production_callers(symbol).iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            "takes_owner_param": takes_owner_param,
            "uses_owner": uses_owner,
        })
    }

    /// The type a method is declared with, or that all the methods calling a
    /// private free function belong to, and the file that type is declared in.
    pub(super) fn owner_home(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        let owner = match symbol.kind {
            SymbolKind::Method => symbol.owner.clone(),
            _ => self
                .caller_owner(symbol)
                .and_then(|owner| SymbolId::parse(&owner)),
        };
        let Some(owner) = owner
            .and_then(|id| project.symbol(&id))
            .filter(|o| o.kind == SymbolKind::Type)
        else {
            return json!({});
        };
        json!({
            "owner": owner.id.as_str(),
            "owner_name": owner.name,
            "owner_file": owner.file.to_string_lossy(),
        })
    }

    /// What a free function uses of the project, by type and by module: the
    /// members of each type it uses, and the modules its uses lie in.
    pub(super) fn envy(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        let used = project.uses(&symbol.id);
        if symbol.kind != SymbolKind::Function || used.is_empty() {
            return json!({});
        }
        let mut members: BTreeMap<&SymbolId, BTreeSet<&str>> = BTreeMap::new();
        let mut modules: BTreeMap<&str, BTreeSet<&SymbolId>> = BTreeMap::new();
        let mut own = 0;
        for id in used {
            if let Some(owner) = owner_type(project, id)
                && let Some(member) = project.symbol(id)
            {
                members.entry(&owner.id).or_default().insert(&member.name);
            }
            if id.module() == symbol.id.module() {
                own += 1;
            } else {
                modules.entry(id.module()).or_default().insert(id);
            }
        }
        let target = members
            .iter()
            .max_by_key(|(id, names)| (names.len(), std::cmp::Reverse(*id)))
            .map(|(id, names)| (*id, names));
        let outside = (modules.len() == 1)
            .then(|| modules.iter().next())
            .flatten();
        let names_of = |ids: &BTreeSet<&SymbolId>| -> Vec<&str> {
            ids.iter()
                .filter_map(|id| project.symbol(id))
                .map(|s| s.name.as_str())
                .collect()
        };
        json!({
            "target": target.map_or("", |(id, _)| id.as_str()),
            "target_name": target.and_then(|(id, _)| project.symbol(id)).map_or("", |t| t.name.as_str()),
            "count": target.map_or(0, |(_, names)| names.len()),
            "member_names": target.map(|(_, names)| names.iter().collect::<Vec<_>>()).unwrap_or_default(),
            "others": members.keys().filter(|id| Some(**id) != target.map(|(t, _)| t)).map(|id| id.as_str()).collect::<Vec<_>>(),
            "same_module": target.is_some_and(|(id, _)| id.module() == symbol.id.module()),
            // A type parameter bound to the target is not read from the code model.
            "generic": false,
            "module_target": outside.map_or("", |(module, _)| *module),
            "module_uses": outside.map(|(_, ids)| ids.iter().map(|id| id.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
            "module_use_names": outside.map(|(_, ids)| names_of(ids)).unwrap_or_default(),
            "own_uses": own,
        })
    }

    /// The owner every production caller of a private free function is a method
    /// of, as the owner key of the callers writes it.
    fn caller_owner(&self, symbol: &Symbol) -> Option<String> {
        if symbol.kind != SymbolKind::Function || symbol.visibility != Visibility::Private {
            return None;
        }
        sole_owner(&self.production_callers(symbol))
    }

    fn production_callers(&self, symbol: &Symbol) -> Vec<&Symbol> {
        let project = self.project;
        project
            .callers(&symbol.id)
            .iter()
            .filter(|id| !project.in_test(id))
            .filter_map(|id| project.symbol(id))
            .collect()
    }
}

/// The owner every caller is a method of, when there is at least one caller
/// and they all are methods of the same owner.
fn sole_owner(callers: &[&Symbol]) -> Option<String> {
    let mut owners = callers.iter().map(|c| {
        (c.kind == SymbolKind::Method)
            .then(|| layout::owner_key(c))
            .flatten()
    });
    let first = owners.next()??;
    owners
        .all(|o| o.as_deref() == Some(first.as_str()))
        .then_some(first)
}

/// The type whose member `id` is: not an interface, whose methods any type
/// may provide.
fn owner_type<'p>(project: &'p Project, id: &SymbolId) -> Option<&'p Symbol> {
    project
        .symbol(id)?
        .owner
        .as_ref()
        .and_then(|owner| project.symbol(owner))
        .filter(|owner| owner.kind == SymbolKind::Type)
}

/// Whether the function calls or references the owner type or one of its
/// members: without that, it has no more to do with the owner than with any
/// other type, however few callers it has.
fn uses_owner(project: &Project, symbol: &Symbol, owner: &str) -> bool {
    let owner_id = project
        .symbol(&SymbolId::parse(owner).unwrap_or_else(|| symbol.id.clone()))
        .map(|o| o.id.clone());
    let Some(owner_id) = owner_id else {
        return false;
    };
    project.uses(&symbol.id).iter().any(|used| {
        *used == owner_id
            || project
                .symbol(used)
                .is_some_and(|u| u.owner.as_ref() == Some(&owner_id))
    })
}
