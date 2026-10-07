use lighthouse_model::{Diagnostic, Project, Symbol, SymbolKind, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{
    finding, generated,
    naming::{Naming, has_tests},
};

const ID: &str = "testing/owner-test";

#[derive(Deserialize)]
struct Options {
    #[serde(flatten)]
    naming: Naming,
    kinds: Vec<String>,
    include_internal: bool,
    include_data_types: bool,
    exempt_methods: Vec<String>,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// A public symbol that no test touches, in a module that has tests: it has
/// no owner test by name and no test code calls or references it (or, for a
/// type, one of its members).
fn check(meta: &RuleManifest, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file else {
        return Ok(Vec::new());
    };
    let project = ctx.project;
    if file.test || generated(ctx) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for symbol in project.symbols_in(&file.path) {
        if !candidate(project, symbol, &options) || !has_tests(project, symbol.id.module()) {
            continue;
        }
        if !options.naming.credited_tests(project, symbol).is_empty() || touched(project, symbol) {
            continue;
        }
        found.push(finding(
            meta,
            symbol,
            format!(
                "public {} {} has no owner test; add one top-level test for it",
                symbol.kind.as_str(),
                symbol.name
            ),
            json!({ "symbol": symbol.id.as_str() }),
        ));
    }
    Ok(found)
}

fn candidate(project: &Project, symbol: &Symbol, options: &Options) -> bool {
    let exposed = |v: Visibility| {
        v == Visibility::Public || (options.include_internal && v == Visibility::Internal)
    };
    let owner = symbol.owner.as_ref().and_then(|o| project.symbol(o));
    let member = match (symbol.kind, owner) {
        (SymbolKind::Method, Some(o)) => o.kind == SymbolKind::Type && exposed(o.visibility),
        (SymbolKind::Method, None) => false,
        (_, owner) => owner.is_none(),
    };
    member
        && !implements_trait(symbol)
        && (options.include_data_types || !data_only(project, symbol))
        && exposed(symbol.visibility)
        && options.kinds.iter().any(|k| k == symbol.kind.as_str())
        && !(symbol.kind == SymbolKind::Method && options.exempt_methods.contains(&symbol.name))
        && !project.in_test(&symbol.id)
}

/// A method declared by an impl of a trait: its id carries the trait next to
/// the type (`m::Type::Trait::method`), and the trait is what it is tested
/// through.
fn implements_trait(symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Method && symbol.id.as_str().matches("::").count() > 2
}

/// A type that declares no method: it is specified by the code that builds and
/// reads it.
fn data_only(project: &Project, symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Type
        && !project.members(&symbol.id).iter().any(|id| {
            project
                .symbol(id)
                .is_some_and(|m| m.kind == SymbolKind::Method)
        })
}

/// Whether test code uses the symbol, or a member of it. Heuristic references
/// count: a call through a receiver of unknown type may reach the symbol, and
/// this rule only warns, so missing an owner test is the mistake to avoid.
fn touched(project: &Project, symbol: &Symbol) -> bool {
    let used_by_tests = |id| {
        project
            .callers(id)
            .iter()
            .chain(project.references(id))
            .any(|user| project.in_test(user))
    };
    used_by_tests(&symbol.id) || project.members(&symbol.id).iter().any(used_by_tests)
}
