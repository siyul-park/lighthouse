use std::collections::BTreeSet;

use lighthouse_model::{Diagnostic, Project, Symbol, SymbolId, SymbolKind, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, functions, order::group_index, skipped};

const ID: &str = "design/callers-before-callees";

#[derive(Deserialize)]
struct Options {
    shared_after_last_caller: bool,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(DecisionRule::new(ID, &[], check))
}

/// A private function that only code of its own file uses is declared after
/// its first caller (after its last one with `shared_after_last_caller`). A
/// callee whose declaration group the file order places before its caller's is
/// not judged: that order, not the reading order, fixes its position. Neither
/// is a method of an inherent impl called from a trait impl, which follows it.
fn check(meta: &RuleManifest, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let language = ctx.file.map(|(file, _)| file.lang.as_str());
    let group = group_index(language, &serde_json::Map::new())?;
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
        if group(callee).zip(group(anchor)).is_some_and(|(c, a)| c < a)
            || (inherent(callee) && via_trait(anchor))
        {
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

/// A method declared by an impl of a trait: its id carries the trait next to
/// the type (`m::Type::Trait::method`).
fn via_trait(symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Method && symbol.id.as_str().matches("::").count() > 2
}

/// A method declared by an inherent impl of its type.
fn inherent(symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Method && !via_trait(symbol)
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
