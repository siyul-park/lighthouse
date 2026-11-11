//! The call-graph `height` of a function: how many calls deep the project goes
//! below it (NDepend's Level). A function that calls no project function has
//! height 0, any other is 1 plus the greatest height of its callees, and a
//! cycle of calls shares one height. A method an interface or trait declares
//! without a body stands for its implementations: it has the greatest height
//! among them.

use std::collections::BTreeMap;

use lighthouse_model::{
    graph::Graph,
    ucm::{Model, Symbol, SymbolId, SymbolKind},
};

use crate::fact::Fact;

/// How many embedded types deep a promoted method is looked for.
const EMBEDDING_DEPTH: usize = 8;

/// The height of every function and method of the project, as `Fact::Int`.
pub fn heights(project: &dyn Model) -> BTreeMap<SymbolId, Fact> {
    let mut ids: Vec<&SymbolId> = project
        .symbols()
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .map(|s| &s.id)
        .collect();
    ids.sort();
    let at = |id: &SymbolId| ids.binary_search(&id).ok().map(|at| at as u32);
    let mut edges = Vec::new();
    let mut transparent = vec![false; ids.len()];
    for (from, id) in (0u32..).zip(&ids) {
        let implementations = implementations(project, id);
        transparent[from as usize] = implementations.is_some();
        let targets = match &implementations {
            Some(found) => found.as_slice(),
            None => project.callees(id),
        };
        edges.extend(targets.iter().filter_map(at).map(|to| (from, to)));
    }
    let graph = Graph::new(ids.len(), edges);
    ids.iter()
        .zip(graph.heights(&transparent))
        .map(|(id, height)| ((*id).clone(), Fact::Int(i64::from(height))))
        .collect()
}

/// The methods that implement an abstract method: those a type that
/// implements its interface carries, a method an embedded field brings
/// included. A method that says which method it implements (a Rust trait
/// impl) counts for that one only; otherwise the name decides, and an inherent
/// method of the same name does not count. `None` for a method with a body or
/// one of no interface.
fn implementations(project: &dyn Model, id: &SymbolId) -> Option<Vec<SymbolId>> {
    let symbol = project.symbol(id)?;
    if symbol.kind != SymbolKind::Method || project.function(id).is_some() {
        return None;
    }
    let owner = project.symbol(symbol.owner.as_ref()?)?;
    if owner.kind != SymbolKind::Interface {
        return None;
    }
    Some(
        project
            .implementers(&owner.id)
            .iter()
            .flat_map(|ty| carried(project, ty, symbol, EMBEDDING_DEPTH))
            .collect(),
    )
}

/// The methods that implement `abstract_method` on type `ty`, or when it
/// declares none, on the types it embeds.
fn carried(
    project: &dyn Model,
    ty: &SymbolId,
    abstract_method: &Symbol,
    depth: usize,
) -> Vec<SymbolId> {
    let members = project.members(ty);
    let own: Vec<SymbolId> = members
        .iter()
        .filter_map(|m| project.symbol(m))
        .filter(|m| implements(project, m, abstract_method))
        .map(|m| m.id.clone())
        .collect();
    if !own.is_empty() || depth == 0 {
        return own;
    }
    members
        .iter()
        .filter_map(|m| embedded(project, project.symbol(m)?))
        .flat_map(|inner| carried(project, &inner, abstract_method, depth - 1))
        .collect()
}

/// Whether `method` implements `abstract_method`.
fn implements(project: &dyn Model, method: &Symbol, abstract_method: &Symbol) -> bool {
    if method.kind != SymbolKind::Method || !method.implementation {
        return false;
    }
    match project
        .function(&method.id)
        .and_then(|f| f.implements.as_deref())
    {
        Some(named) => named == abstract_method.id.as_str(),
        None => method.name == abstract_method.name,
    }
}

/// The type an embedded field holds.
fn embedded(project: &dyn Model, field: &Symbol) -> Option<SymbolId> {
    let held = field.type_ref.as_ref()?.symbol.as_ref()?;
    let ty = project.symbol(&SymbolId::parse(&format!("{held}#type"))?)?;
    (field.kind == SymbolKind::Field && field.embedded).then(|| ty.id.clone())
}
