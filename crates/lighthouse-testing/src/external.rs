use std::collections::BTreeSet;

use lighthouse_model::{Diagnostic, Symbol, SymbolId, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, generated};

const ID: &str = "testing/external-test-package";

#[derive(Deserialize)]
struct NoOptions {}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(DecisionRule::new(ID, &[], check))
}

/// A test file that lives in the package it tests, inside its private scope,
/// and reaches a private symbol of that package. A test file of a module that
/// tests another module from outside (`test_of`) cannot reach private symbols
/// and is fine; so is an internal test file that only uses public symbols,
/// which the language lets live outside as well but which breaks no boundary.
fn check(meta: &RuleManifest, ctx: &Ctx, _: NoOptions) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file else {
        return Ok(Vec::new());
    };
    let project = ctx.project;
    if !file.test || generated(ctx) {
        return Ok(Vec::new());
    }
    let mut symbols: Vec<&Symbol> = project.symbols_in(&file.path).collect();
    symbols.sort_by_key(|s| (s.span.start, &s.id));
    let Some(first) = symbols.first() else {
        return Ok(Vec::new());
    };
    let module = first.id.module();
    if project.module(module).is_none_or(|m| m.test_of.is_some()) {
        return Ok(Vec::new());
    }
    let mut reached: BTreeSet<&SymbolId> = BTreeSet::new();
    let mut at = None;
    for symbol in &symbols {
        let private: Vec<&SymbolId> = project
            .uses(&symbol.id)
            .iter()
            .filter(|id| id.module() == module)
            .filter(|id| !project.in_test(id))
            .filter(|id| {
                project
                    .symbol(id)
                    .is_some_and(|s| s.visibility == Visibility::Private)
            })
            .collect();
        if !private.is_empty() {
            at.get_or_insert(*symbol);
            reached.extend(private);
        }
    }
    let Some(symbol) = at else {
        return Ok(Vec::new());
    };
    let names: Vec<&str> = reached.iter().map(|id| id.as_str()).collect();
    Ok(vec![finding(
        meta,
        symbol,
        format!(
            "test file {} lives inside the package it tests and uses its private symbols ({}); test through the public boundary from outside the package",
            file.path.display(),
            names.len()
        ),
        json!({
            "test_file": file.path.to_string_lossy(),
            "package": module,
            "private_symbols": names,
        }),
    )])
}
