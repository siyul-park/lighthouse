use lighthouse_model::{Diagnostic, SymbolKind, SymbolRole};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, generated};

const ID: &str = "testing/standard-assertions";

#[derive(Deserialize)]
struct NoOptions {}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(DecisionRule::new(ID, &[], check))
}

/// Functions of a test file that fail the test by hand after a comparison:
/// tests that could assert, and helpers that reimplement an assertion. Only
/// providers that count such checks (`manual_assertions`) can be judged.
fn check(meta: &RuleManifest, ctx: &Ctx, _: NoOptions) -> Result<Vec<Diagnostic>, Error> {
    let Some((file, _)) = ctx.file else {
        return Ok(Vec::new());
    };
    if !file.test || generated(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let mut found = Vec::new();
    for symbol in project.symbols_in(&file.path) {
        let Some(summary) = project.function(&symbol.id) else {
            continue;
        };
        if summary.manual_assertions == 0 {
            continue;
        }
        let what = match (symbol.kind, symbol.role) {
            (SymbolKind::Test, _) => "test",
            (_, Some(SymbolRole::TestHelper)) => "test helper",
            _ => "function",
        };
        found.push(finding(
            meta,
            symbol,
            format!(
                "{what} {} compares and fails by hand {} time(s); use the project's assertion library",
                symbol.name, summary.manual_assertions
            ),
            json!({
                "symbol": symbol.id.as_str(),
                "manual_assertions": summary.manual_assertions,
            }),
        ));
    }
    Ok(found)
}
