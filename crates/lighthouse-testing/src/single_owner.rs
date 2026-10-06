use lighthouse_model::{Diagnostic, Symbol, SymbolKind, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, generated, naming::Naming};

const ID: &str = "testing/single-owner-test";

#[derive(Deserialize)]
struct Options {
    #[serde(flatten)]
    naming: Naming,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// A public symbol that two or more top-level tests are named after.
fn check(meta: &RuleMeta, ctx: &Ctx, options: Options) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file else {
        return Ok(Vec::new());
    };
    let project = ctx.project;
    if file.test || generated(ctx) {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for symbol in project.symbols_in(&file.path) {
        if !nameable(symbol) || project.in_test(&symbol.id) {
            continue;
        }
        let owners = options.naming.owner_tests(project, symbol);
        if owners.len() < 2 {
            continue;
        }
        let tests: Vec<&str> = owners.iter().map(|(t, _)| t.symbol.as_str()).collect();
        found.push(finding(
            meta,
            symbol,
            format!(
                "public {} {} has {} owner tests; keep one top-level test and make the rest cases of it",
                symbol.kind.as_str(),
                symbol.name,
                tests.len()
            ),
            json!({ "symbol": symbol.id.as_str(), "owner_tests": tests }),
        ));
    }
    Ok(found)
}

fn nameable(symbol: &Symbol) -> bool {
    symbol.visibility == Visibility::Public
        && matches!(
            symbol.kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Type | SymbolKind::Interface
        )
}
