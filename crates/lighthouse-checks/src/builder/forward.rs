//! The `forwards_only` and `local_callers` facts: which wrapper only passes
//! its work on, and whether a call leads back.

use std::collections::BTreeSet;

use lighthouse_model::{Node, Project, Symbol, SymbolId, Target, Visibility};

use super::role::satisfies_interface;

/// The private target a private undocumented wrapper only forwards to. The
/// wrapper must have exactly one caller, test callers included, and no use as
/// a value; a method named like an interface method may be reached through
/// that interface, so it is left alone.
pub(super) fn forwarded<'p>(project: &'p Project, wrapper: &Symbol) -> Option<&'p SymbolId> {
    if wrapper.visibility != Visibility::Private
        || wrapper.doc.is_some()
        || project.callers(&wrapper.id).len() != 1
        || !project.references(&wrapper.id).is_empty()
        || satisfies_interface(project, wrapper)
    {
        return None;
    }
    let summary = project.function(&wrapper.id)?;
    let Some(Target::Resolved(Node::Symbol(target))) = &summary.forwards_to else {
        return None;
    };
    let callee = project.symbol(target)?;
    let eligible = callee.visibility == Visibility::Private
        && target.module() == wrapper.id.module()
        && callee.kind == wrapper.kind
        && project.callers(target).len() == 1
        && !reaches(project, target, &wrapper.id);
    eligible.then_some(target)
}

fn reaches(project: &Project, from: &SymbolId, goal: &SymbolId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(current) = stack.pop() {
        if current == goal {
            return true;
        }
        if seen.insert(current) {
            stack.extend(project.callees(current));
        }
    }
    false
}

/// Whether the callee reaches one of its callers: recursion through others.
pub(super) fn cyclic(project: &Project, callee: &Symbol, callers: &[&Symbol]) -> bool {
    let goals: BTreeSet<&SymbolId> = callers.iter().map(|s| &s.id).collect();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&SymbolId> = project.callees(&callee.id).iter().collect();
    while let Some(current) = stack.pop() {
        if goals.contains(current) {
            return true;
        }
        if seen.insert(current) {
            stack.extend(project.callees(current));
        }
    }
    false
}
