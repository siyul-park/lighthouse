//! The `unit` of a function: its own measures plus those of the private
//! functions that exist only to serve it, which can be neither reused nor read
//! apart from it.

use std::collections::BTreeSet;

use lighthouse_model::{
    EdgeKind, FunctionSummary, Node, Project, Symbol, SymbolId, SymbolKind, Target, Visibility,
};
use serde_json::{Value, json};

use super::Builder;
use crate::metrics::own;

impl Builder<'_> {
    /// `{ lines, statements, cyclomatic, cognitive, helpers, caller }`: the
    /// measures of the function and of its single-use helpers, the helpers
    /// themselves and, for a function that is one, its caller; null for what
    /// is no function.
    pub(super) fn unit(&self, symbol: &Symbol, summary: Option<&FunctionSummary>) -> Value {
        let Some(summary) = summary else {
            return Value::Null;
        };
        let project = self.project;
        let helpers = helpers_of(project, &symbol.id);
        let mut total = own(project, symbol, summary);
        for helper in &helpers {
            let Some(summary) = project.function(&helper.id) else {
                continue;
            };
            let part = own(project, helper, summary);
            total.lines += part.lines;
            total.statements += part.statements;
            total.cognitive += part.cognitive;
            total.cyclomatic += part.cyclomatic.saturating_sub(1);
        }
        json!({
            "lines": total.lines,
            "statements": total.statements,
            "cyclomatic": total.cyclomatic,
            "cognitive": total.cognitive,
            "helpers": helpers.iter().map(|h| named(h)).collect::<Vec<_>>(),
            "caller": sole_caller(project, symbol).map(named),
        })
    }
}

fn named(symbol: &Symbol) -> Value {
    json!({ "id": symbol.id.as_str(), "name": symbol.name })
}

/// The single-use helpers of `root`, then theirs: breadth first, in the order
/// of the callees.
fn helpers_of<'p>(project: &'p Project, root: &'p SymbolId) -> Vec<&'p Symbol> {
    let mut seen: BTreeSet<&SymbolId> = BTreeSet::from([root]);
    let mut order = vec![root];
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(&caller) = order.get(at) {
        at += 1;
        for callee in project.callees(caller) {
            let helper = project
                .symbol(callee)
                .filter(|h| sole_caller(project, h).is_some_and(|c| c.id == *caller));
            if let Some(helper) = helper.filter(|_| seen.insert(callee)) {
                order.push(&helper.id);
                found.push(helper);
            }
        }
    }
    found
}

/// The one production function that calls the private function `callee`,
/// when nothing else uses it: not a test, not a value use, not a call to
/// itself, not a function that implements an interface.
fn sole_caller<'p>(project: &'p Project, callee: &Symbol) -> Option<&'p Symbol> {
    let serves = callee.visibility == Visibility::Private
        && matches!(callee.kind, SymbolKind::Function | SymbolKind::Method)
        && project
            .function(&callee.id)
            .is_some_and(|f| !f.implementation)
        && project.references(&callee.id).is_empty()
        && !project.in_test(&callee.id)
        && !calls_itself(project, &callee.id);
    if !serves {
        return None;
    }
    let mut callers = project
        .callers(&callee.id)
        .iter()
        .filter(|id| !project.in_test(id));
    let caller = callers.next()?;
    if callers.next().is_some() {
        return None;
    }
    project
        .symbol(caller)
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
}

/// `callers` hides the edge of a function to itself, so it is read here.
fn calls_itself(project: &Project, id: &SymbolId) -> bool {
    project.edges_from(id).any(|edge| {
        edge.kind == EdgeKind::Calls
            && matches!(&edge.to, Target::Resolved(Node::Symbol(to)) if to == id)
    })
}
