use lighthouse_model::{Diagnostic, Project, Symbol, SymbolId, SymbolKind, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;
use serde_json::json;

use crate::{
    finding, functions,
    layout::{has_word_prefix, owner_key},
    skipped,
};

const ID: &str = "design/receiver-owned-behavior";

#[derive(Deserialize)]
struct Options {
    constructor_prefixes: Vec<String>,
    require_owner_param: bool,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(DecisionRule::new(ID, &[], check))
}

/// A private free function whose every production caller is a method of one
/// owner type: its behavior belongs to that owner.
fn check(meta: &RuleManifest, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let mut found = Vec::new();
    for symbol in functions(ctx) {
        if symbol.kind != SymbolKind::Function
            || symbol.visibility != Visibility::Private
            || matches!(symbol.name.as_str(), "main" | "init")
            || is_constructor(symbol, &options)
            || !project.references(&symbol.id).is_empty()
        {
            continue;
        }
        let callers: Vec<&Symbol> = project
            .callers(&symbol.id)
            .iter()
            .filter(|id| !project.in_test(id))
            .filter_map(|id| project.symbol(id))
            .collect();
        let Some(owner) = sole_owner(&callers) else {
            continue;
        };
        let takes_owner_param = project.function(&symbol.id).is_some_and(|f| {
            let bare = owner
                .rsplit_once('#')
                .map_or(owner.as_str(), |(head, _)| head);
            f.param_types.iter().any(|t| t == bare)
        });
        let uses_owner = uses_owner(project, symbol, &owner);
        if !(takes_owner_param || uses_owner) || (options.require_owner_param && !takes_owner_param)
        {
            continue;
        }
        let owner_name = owner
            .rsplit_once('#')
            .map_or(owner.as_str(), |(head, _)| head)
            .rsplit("::")
            .next()
            .unwrap_or_default();
        found.push(finding(
            meta,
            symbol,
            format!(
                "private function {} is called only by methods of {owner_name}; consider making it a method of {owner_name}",
                symbol.name
            ),
            json!({
                "owner": owner,
                "callers": callers.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
                "takes_owner_param": takes_owner_param,
                "uses_owner": uses_owner,
            }),
        ));
    }
    Ok(found)
}

/// Whether the function calls or references the owner type or one of its
/// members: without that, it has no more to do with the owner than with any
/// other type, however few callers it has.
fn uses_owner(project: &Project, symbol: &Symbol, owner: &str) -> bool {
    let owner_id = project
        .symbol(&SymbolId::parse(owner).unwrap_or_else(|| symbol.id.clone()))
        .map(|o| o.id.clone());
    let Some(owner_id) = owner_id else {
        return false;
    };
    project.uses(&symbol.id).iter().any(|used| {
        *used == owner_id
            || project
                .symbol(used)
                .is_some_and(|u| u.owner.as_ref() == Some(&owner_id))
    })
}

fn is_constructor(symbol: &Symbol, options: &Options) -> bool {
    options
        .constructor_prefixes
        .iter()
        .any(|p| has_word_prefix(&symbol.name, p))
}

/// The owner every caller is a method of, when there is at least one caller
/// and they all are methods of the same owner.
fn sole_owner(callers: &[&Symbol]) -> Option<String> {
    let mut owners = callers.iter().map(|c| {
        (c.kind == SymbolKind::Method)
            .then(|| owner_key(c))
            .flatten()
    });
    let first = owners.next()??;
    owners
        .all(|o| o.as_deref() == Some(first.as_str()))
        .then_some(first)
}
