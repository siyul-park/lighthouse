//! The facts about which type or module a function belongs with:
//! `receiver_affinity`, `owner_home` and `envy`. They share one way to find the
//! owner of a symbol that a function uses and the owner its callers have.

use std::collections::{BTreeMap, BTreeSet};

use lighthouse_model::{Project, Symbol, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::{Builder, effective::named_params};
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
            .is_some_and(|f| named_params(f).any(|t| t == bare));
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
        // A helper belongs to the type it works on, not to every type whose
        // methods happen to be its only callers.
        let owner = match symbol.kind {
            SymbolKind::Method => symbol.owner.clone(),
            _ => self
                .caller_owner(symbol)
                .filter(|owner| uses_owner(project, symbol, owner))
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
            "owner_module": owner.id.module(),
            "owner_tree": tree_of(&owner.file),
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
                && matches!(member.kind, SymbolKind::Field | SymbolKind::Method)
            {
                members.entry(&owner.id).or_default().insert(&member.name);
            }
            if id.module() == symbol.id.module() {
                own += 1;
            } else if !inside(symbol.id.module(), id.module()) {
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
        // Behavior ties a function to a module: calling its functions and
        // methods, reading its fields. Naming its constants and types does not.
        let behavior = |ids: &BTreeSet<&SymbolId>| -> Vec<&Symbol> {
            ids.iter()
                .filter_map(|id| project.symbol(id))
                .filter(|s| {
                    matches!(
                        s.kind,
                        SymbolKind::Function | SymbolKind::Method | SymbolKind::Field
                    )
                })
                .collect()
        };
        json!({
            "target": target.map_or("", |(id, _)| id.as_str()),
            "target_name": target.and_then(|(id, _)| project.symbol(id)).map_or("", |t| t.name.as_str()),
            "count": target.map_or(0, |(_, names)| names.len()),
            "member_names": target.map(|(_, names)| names.iter().collect::<Vec<_>>()).unwrap_or_default(),
            "others": members.keys().filter(|id| Some(**id) != target.map(|(t, _)| t)).map(|id| id.as_str()).collect::<Vec<_>>(),
            "same_module": target.is_some_and(|(id, _)| id.module() == symbol.id.module()),
            "module_target": outside.map_or("", |(module, _)| *module),
            "module_uses": outside.map(|(_, ids)| behavior(ids).iter().map(|s| s.id.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
            "module_use_names": outside.map(|(_, ids)| behavior(ids).iter().map(|s| s.name.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
            "own_uses": own,
            "entry": self.is_entry(symbol),
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

    /// Whether the symbol is where the program starts or is wired up: it lies
    /// in a Go `main` package or at the root of a Rust binary, or an `init`
    /// function or a package variable's initializer uses it. Moving such a
    /// function says nothing about its design.
    fn is_entry(&self, symbol: &Symbol) -> bool {
        let project = self.project;
        let in_binary = project.module(symbol.id.module()).is_some_and(|m| {
            m.name.as_deref() == Some("main") || (m.path.contains("[bin:") && !m.path.contains('/'))
        });
        in_binary
            || project
                .callers(&symbol.id)
                .iter()
                .chain(project.references(&symbol.id))
                .filter_map(|id| project.symbol(id))
                .any(|user| {
                    user.kind == SymbolKind::Var
                        || (user.kind == SymbolKind::Function && user.name == "init")
                })
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

/// The files of the module tree of a file: the directory a Rust module `m.rs`
/// keeps its child modules in (`m/`), or, for a crate root or a `mod.rs`, the
/// directory the file is in. A method's id names the module of its type, not
/// the module its `impl` is written in, so the place of the `impl` is its file.
fn tree_of(file: &std::path::Path) -> String {
    let dir = file.parent().map(|d| d.to_string_lossy().into_owned());
    let dir = dir.filter(|d| !d.is_empty());
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned());
    let root = matches!(stem.as_deref(), Some("lib" | "main" | "mod"));
    let path = match (dir, stem) {
        (Some(dir), _) if root || file.extension().is_none_or(|e| e != "rs") => dir,
        (Some(dir), Some(stem)) => format!("{dir}/{stem}"),
        (None, Some(stem)) if !root => stem,
        _ => String::new(),
    };
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
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

/// Whether `inner` lies inside `outer`: a submodule uses its parent's types
/// as a matter of course, so that use says nothing about where a function
/// belongs.
fn inside(inner: &str, outer: &str) -> bool {
    inner
        .strip_prefix(outer)
        .is_some_and(|rest| rest.starts_with('/'))
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
    let Some(owner_id) = SymbolId::parse(owner)
        .and_then(|id| project.symbol(&id))
        .map(|o| o.id.clone())
    else {
        return false;
    };
    project.uses(&symbol.id).iter().any(|used| {
        *used == owner_id
            || project
                .symbol(used)
                .is_some_and(|u| u.owner.as_ref() == Some(&owner_id))
    })
}
