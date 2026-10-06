use std::collections::BTreeSet;

use lighthouse_model::{Diagnostic, Project, Symbol, SymbolId, SymbolKind, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, functions, skipped};

const ID: &str = "design/callers-before-callees";

#[derive(Deserialize)]
struct Options {
    shared_after_last_caller: bool,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// A private function that only code of its own file uses is declared after
/// its first caller (after its last one with `shared_after_last_caller`).
fn check(meta: &RuleMeta, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let mut found = Vec::new();
    for callee in functions(ctx) {
        let Some(callers) = file_local_callers(project, callee) else {
            continue;
        };
        let Some(first) = callers.first().copied() else {
            continue;
        };
        let anchor = if options.shared_after_last_caller {
            callers.last().copied().unwrap_or(first)
        } else {
            first
        };
        if callee.span.start > anchor.span.start {
            continue;
        }
        found.push(finding(
            meta,
            callee,
            format!(
                "private {} {} is declared before its caller {}; callers come first",
                callee.kind.as_str(),
                callee.name,
                anchor.name
            ),
            json!({ "caller": anchor.id.as_str(), "callee": callee.id.as_str() }),
        ));
    }
    Ok(found)
}

/// The production callers of a private function in source order, when every
/// one of them lives in the callee's file, the function is never used as a
/// value, and no call leads back to it. `None` when the order cannot be judged
/// with confidence.
fn file_local_callers<'p>(project: &'p Project, callee: &Symbol) -> Option<Vec<&'p Symbol>> {
    let entry = matches!(callee.name.as_str(), "main" | "init");
    if callee.visibility != Visibility::Private
        || entry
        || !project.references(&callee.id).is_empty()
        || callee.kind == SymbolKind::Test
    {
        return None;
    }
    let mut callers: Vec<&Symbol> = project
        .callers(&callee.id)
        .iter()
        .filter(|id| !project.in_test(id))
        .filter_map(|id| project.symbol(id))
        .collect();
    let same_file = |s: &&Symbol| -> bool { s.file == callee.file };
    if callers.is_empty() || !callers.iter().all(same_file) || cyclic(project, callee, &callers) {
        return None;
    }
    callers.sort_by_key(|s| (s.span.start, &s.id));
    Some(callers)
}

/// Whether the callee reaches one of its callers: recursion through others.
fn cyclic(project: &Project, callee: &Symbol, callers: &[&Symbol]) -> bool {
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
