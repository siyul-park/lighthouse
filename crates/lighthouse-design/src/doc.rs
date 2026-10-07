use std::collections::BTreeSet;

use lighthouse_model::{
    Diagnostic, EdgeKind, Node, Project, Symbol, SymbolId, SymbolKind, Target, Visibility,
};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, skipped};

const ID: &str = "design/exported-doc";

#[derive(Deserialize)]
struct Options {
    kinds: Vec<String>,
    include_internal: bool,
    exempt_methods: Vec<String>,
    exempt_interface_methods: bool,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

fn check(meta: &RuleManifest, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file.filter(|_| !skipped(ctx)) else {
        return Ok(Vec::new());
    };
    let exposed = |visibility: Visibility| {
        visibility == Visibility::Public
            || (options.include_internal && visibility == Visibility::Internal)
    };
    let documented_by_interface = options
        .exempt_interface_methods
        .then(|| interface_methods(ctx.project));
    let mut found = Vec::new();
    for symbol in ctx.project.symbols_in(&file.path) {
        let owner = symbol.owner.as_ref().and_then(|o| ctx.project.symbol(o));
        let named = options.kinds.iter().any(|k| k == symbol.kind.as_str());
        let reachable = exposed(symbol.visibility)
            && owner.is_none_or(|o| exposed(o.visibility) && o.kind != SymbolKind::Interface);
        let exempt = symbol.kind == SymbolKind::Method
            && (options.exempt_methods.contains(&symbol.name)
                || documented_by_interface
                    .as_ref()
                    .is_some_and(|known| implements_by_name(known, symbol)));
        if !named || !reachable || exempt || symbol.doc.is_some() || ctx.project.in_test(&symbol.id)
        {
            continue;
        }
        found.push(finding(
            meta,
            symbol,
            format!(
                "exported {} {} must have a doc comment",
                symbol.kind.as_str(),
                symbol.name
            ),
            json!({ "symbol": symbol.id.as_str() }),
        ));
    }
    Ok(found)
}

/// The documented methods of the project's interfaces and which types
/// implement them, as `(type, method name)` pairs: a method that implements a
/// documented interface method is documented where the interface declares it.
fn interface_methods(project: &Project) -> BTreeSet<(SymbolId, String)> {
    let declared: BTreeSet<(&SymbolId, &str)> = project
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Method && s.doc.is_some())
        .filter_map(|s| Some((s.owner.as_ref()?, s.name.as_str())))
        .collect();
    let mut known = BTreeSet::new();
    for edge in &project.edges {
        let (EdgeKind::Implements, Node::Symbol(ty), Target::Resolved(Node::Symbol(interface))) =
            (edge.kind, &edge.from, &edge.to)
        else {
            continue;
        };
        for (owner, name) in &declared {
            if *owner == interface {
                known.insert((ty.clone(), (*name).to_owned()));
            }
        }
    }
    known
}

fn implements_by_name(known: &BTreeSet<(SymbolId, String)>, method: &Symbol) -> bool {
    method
        .owner
        .as_ref()
        .is_some_and(|owner| known.contains(&(owner.clone(), method.name.clone())))
}
