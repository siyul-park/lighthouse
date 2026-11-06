//! The `forwards_only` and `local_callers` facts: which wrapper only passes
//! its work on, and whether a call leads back.

use std::collections::BTreeSet;

use lighthouse_model::ucm::{Model, Node, Symbol, SymbolId, SymbolKind, Target, Visibility};

/// The private target a private undocumented wrapper only forwards to. The
/// wrapper must have exactly one caller, test callers included, and no use as
/// a value; a method that implements a trait or interface method may be
/// reached through it, so it is left alone. A variable is no callee: it is
/// forwarded when exactly one symbol reads it.
pub(super) fn forwarded<'p>(project: &'p dyn Model, wrapper: &Symbol) -> Option<&'p SymbolId> {
    if wrapper.visibility != Visibility::Private
        || project
            .place(&wrapper.id)
            .is_some_and(|place| place.doc.is_some())
        || uses(project, wrapper) != 1
        || (wrapper.kind != SymbolKind::Var && !project.references(&wrapper.id).is_empty())
    {
        return None;
    }
    if wrapper.implementation {
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
        && uses(project, callee) == 1
        && !reaches(project, target, &[&wrapper.id]);
    eligible.then_some(target)
}

/// Whether a call chain leads from `from`, itself included, to one of `goals`.
pub(super) fn reaches(project: &dyn Model, from: &SymbolId, goals: &[&SymbolId]) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![from];
    while let Some(current) = stack.pop() {
        if goals.contains(&current) {
            return true;
        }
        if seen.insert(current) {
            stack.extend(project.callees(current));
        }
    }
    false
}

/// How many symbols use `symbol`: the callers of a function, the readers of a
/// variable.
fn uses(project: &dyn Model, symbol: &Symbol) -> usize {
    if symbol.kind == SymbolKind::Var {
        project.references(&symbol.id).len()
    } else {
        project.callers(&symbol.id).len()
    }
}
