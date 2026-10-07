use lighthouse_model::{Diagnostic, Symbol, SymbolKind, SymbolRole};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, generated};

const ID: &str = "testing/test-file-layout";

#[derive(Deserialize)]
struct NoOptions {}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[], check))
}

/// Fixtures declared after the file's first test, and helpers declared before
/// a test or helper of the same file that uses them. Only providers that tell
/// a declaration's role (`fixture`, `test-helper`) can be judged.
fn check(meta: &RuleManifest, ctx: &Ctx, _: NoOptions) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file else {
        return Ok(Vec::new());
    };
    if !file.test || generated(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let mut symbols: Vec<&Symbol> = project.symbols_in(&file.path).collect();
    symbols.sort_by_key(|s| (s.span.start, &s.id));
    let first_test = symbols.iter().find(|s| s.kind == SymbolKind::Test);
    let mut found = Vec::new();
    for symbol in &symbols {
        match symbol.role {
            Some(SymbolRole::Fixture) => {
                let Some(test) = first_test.filter(|t| t.span.start < symbol.span.start) else {
                    continue;
                };
                found.push(finding(
                    meta,
                    symbol,
                    format!(
                        "fixture {} must be declared above the tests, before {}",
                        symbol.name, test.name
                    ),
                    json!({ "symbol": symbol.id.as_str(), "role": "fixture", "before": test.id.as_str() }),
                ));
            }
            Some(SymbolRole::TestHelper) => {
                let user = project
                    .callers(&symbol.id)
                    .iter()
                    .chain(project.references(&symbol.id))
                    .filter_map(|id| project.symbol(id))
                    .filter(|user| user.file == symbol.file && user.span.start > symbol.span.start)
                    .min_by_key(|user| user.span.start);
                let Some(user) = user else {
                    continue;
                };
                found.push(finding(
                    meta,
                    symbol,
                    format!(
                        "test helper {} must be declared below {}, which uses it",
                        symbol.name, user.name
                    ),
                    json!({ "symbol": symbol.id.as_str(), "role": "test-helper", "after": user.id.as_str() }),
                ));
            }
            None => {}
        }
    }
    Ok(found)
}
