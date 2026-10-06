use std::collections::BTreeSet;

use lighthouse_model::{
    Diagnostic, Node, Project, Symbol, SymbolId, SymbolKind, Target, Visibility,
};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, functions, skipped};

const ID: &str = "design/single-use-wrapper";

#[derive(Deserialize)]
struct NoOptions {}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// The private target a private undocumented wrapper only forwards to. The
/// wrapper must have exactly one caller, test callers included, and no use as
/// a value; a method named like an interface method may be reached through
/// that interface, so it is left alone.
pub(crate) fn forwarded<'p>(project: &'p Project, wrapper: &Symbol) -> Option<&'p SymbolId> {
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

pub(crate) fn satisfies_interface(project: &Project, wrapper: &Symbol) -> bool {
    wrapper.kind == SymbolKind::Method
        && project.symbols.iter().any(|s| {
            s.kind == SymbolKind::Method
                && s.name == wrapper.name
                && s.id.module() == wrapper.id.module()
                && s.owner
                    .as_ref()
                    .and_then(|o| project.symbol(o))
                    .is_some_and(|o| o.kind == SymbolKind::Interface)
        })
}

fn check(meta: &RuleMeta, ctx: &Ctx, _: NoOptions) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let mut found = Vec::new();
    for symbol in functions(ctx) {
        let Some(target) = forwarded(project, symbol) else {
            continue;
        };
        found.push(finding(
            meta,
            symbol,
            format!(
                "private helper {} is a single-use forwarding wrapper; inline it",
                symbol.name
            ),
            json!({ "callee": target.as_str(), "callers": project.callers(&symbol.id).len() }),
        ));
    }
    Ok(found)
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
