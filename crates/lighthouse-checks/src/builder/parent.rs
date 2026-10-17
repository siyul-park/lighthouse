//! The `parent` of a module: how it is tied to the module that holds it, from
//! what its symbols use of each other.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::ucm::{Module, Project, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::Builder;

/// What the production symbols of the project use across the modules.
#[derive(Default)]
pub(super) struct Uses {
    /// The modules that declare a production symbol.
    production: BTreeSet<String>,
    /// The production modules that use a symbol of each module, itself left
    /// out.
    users: BTreeMap<String, BTreeSet<String>>,
    /// The private symbols of its parent that each module uses.
    reaches: BTreeMap<String, BTreeSet<SymbolId>>,
    /// The modules whose symbols are used by their parent.
    called: BTreeSet<String>,
}

impl Builder<'_> {
    /// `{ path, sole_user, reaches, called }`, or null when the module has no
    /// parent module in the project or is test code:
    /// - `sole_user`: the parent is the only production module that uses it;
    /// - `reaches`: the private functions, methods and fields of the parent it
    ///   uses, as nodes sorted by id;
    /// - `called`: the parent uses it.
    pub(super) fn module_parent(&self, module: &Module) -> Value {
        let uses = self.indexes.parents.get_or_init(|| uses(self.project));
        let path = module.path.as_str();
        let parent = parent_of(path).filter(|p| self.project.module(p).is_some());
        let Some(parent) = parent.filter(|_| uses.production.contains(path)) else {
            return Value::Null;
        };
        json!({
            "path": parent,
            "sole_user": uses.users.get(path).is_some_and(|u| u.len() == 1 && u.contains(parent)),
            "reaches": uses.reaches.get(path).into_iter().flatten()
                .filter_map(|id| self.project.symbol(id))
                .map(|s| self.node(s))
                .collect::<Vec<_>>(),
            "called": uses.called.contains(path),
        })
    }
}

/// The path of the module that holds `path`.
fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

fn uses(project: &Project) -> Uses {
    let mut found = Uses::default();
    for symbol in project.symbols.iter().filter(|s| !project.in_test(&s.id)) {
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

/// Whether `id` is a private function, method or field of a production
/// module.
fn is_private_member(project: &Project, id: &SymbolId) -> bool {
    !project.in_test(id)
        && project.symbol(id).is_some_and(|s| {
            s.visibility == Visibility::Private
                && matches!(
                    s.kind,
                    SymbolKind::Function | SymbolKind::Method | SymbolKind::Field
                )
        })
}
