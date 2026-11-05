//! The `parent` of a module: how it is tied to the module that holds it, from
//! what its symbols use of each other.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::ucm::{Model, Module, SymbolId, SymbolKind, Visibility};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;

/// What the production symbols of the project use across the modules.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    /// The modules that declare a production symbol.
    pub production: BTreeSet<String>,
    /// The production modules that use a symbol of each module, itself left
    /// out.
    pub users: BTreeMap<String, BTreeSet<String>>,
    /// The private symbols of its parent that each module uses.
    pub reaches: BTreeMap<String, BTreeSet<SymbolId>>,
    /// The modules whose symbols are used by their parent.
    pub called: BTreeSet<String>,
}

/// What the production symbols of the project use of one module, and of its
/// parent.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Used {
    pub production: bool,
    pub users: BTreeSet<String>,
    pub reaches: BTreeSet<SymbolId>,
    pub called: bool,
}

impl Uses {
    /// What the project uses of `module`.
    pub fn of(&self, module: &str) -> Used {
        Used {
            production: self.production.contains(module),
            users: self.users.get(module).cloned().unwrap_or_default(),
            reaches: self.reaches.get(module).cloned().unwrap_or_default(),
            called: self.called.contains(module),
        }
    }
}

impl Builder<'_> {
    /// `{ path, sole_user, reaches, called }`, or null when the module has no
    /// parent module in the project or is test code:
    /// - `sole_user`: the parent is the only production module that uses it;
    /// - `reaches`: the private functions, methods and fields of the parent it
    ///   uses, as nodes sorted by id;
    /// - `called`: the parent uses it.
    pub(super) fn module_parent(&self, module: &Module) -> Value {
        let path = module.path.as_str();
        let used = self.indexes.parent(path);
        let parent = parent_of(path).filter(|p| self.project.module(p).is_some());
        let Some(parent) = parent.filter(|_| used.production) else {
            return Value::Null;
        };
        json!({
            "path": parent,
            "sole_user": used.users.len() == 1 && used.users.contains(parent),
            "reaches": used.reaches.iter()
                .filter_map(|id| self.project.symbol(id))
                .map(|s| self.node(s))
                .collect::<Vec<_>>(),
            "called": used.called,
        })
    }
}

/// What the production symbols of the project use across the modules.
pub fn parents(project: &dyn Model) -> Uses {
    let mut found = Uses::default();
    for symbol in project.symbols().filter(|s| !project.in_test(&s.id)) {
        let from = symbol.id.module();
        found.production.insert(from.to_owned());
        for used in project.uses(&symbol.id) {
            let to = used.module();
            if to == from {
                continue;
            }
            found
                .users
                .entry(to.to_owned())
                .or_default()
                .insert(from.to_owned());
            if parent_of(to) == Some(from) {
                found.called.insert(to.to_owned());
            }
            if parent_of(from) == Some(to) && is_private_member(project, used) {
                found
                    .reaches
                    .entry(from.to_owned())
                    .or_default()
                    .insert(used.clone());
            }
        }
    }
    found
}

/// The path of the module that holds `path`.
fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

/// Whether `id` is a private function, method or field of a production
/// module.
fn is_private_member(project: &dyn Model, id: &SymbolId) -> bool {
    !project.in_test(id)
        && project.symbol(id).is_some_and(|s| {
            s.visibility == Visibility::Private
                && matches!(
                    s.kind,
                    SymbolKind::Function | SymbolKind::Method | SymbolKind::Field
                )
        })
}
