use lighthouse_model::{Diagnostic, Visibility};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::DecisionRule;
use serde::Deserialize;
use serde_json::json;

use crate::{
    finding, functions, skipped,
    wrapper::{forwarded, satisfies_interface},
};

const ID: &str = "design/private-helper-callers";

#[derive(Deserialize)]
struct NoOptions {}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(DecisionRule::new(ID, &[], check))
}

/// A private function or method with exactly one production caller, never
/// used as a value. A forwarding wrapper is left to `single-use-wrapper`.
fn check(meta: &RuleManifest, ctx: &Ctx, _: NoOptions) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let project = ctx.project;
    let mut found = Vec::new();
    for symbol in functions(ctx) {
        if symbol.visibility != Visibility::Private
            || matches!(symbol.name.as_str(), "main" | "init")
            || !project.references(&symbol.id).is_empty()
            || satisfies_interface(project, symbol)
            || forwarded(project, symbol).is_some()
        {
            continue;
        }
        let callers: Vec<_> = project
            .callers(&symbol.id)
            .iter()
            .filter(|id| !project.in_test(id))
            .collect();
        let [caller] = callers.as_slice() else {
            continue;
        };
        let statements = project.function(&symbol.id).map_or(0, |f| f.statements);
        found.push(finding(
            meta,
            symbol,
            format!(
                "private {} {} has one caller ({}); review whether it is part of that caller or names a real policy",
                symbol.kind.as_str(),
                symbol.name,
                project.symbol(caller).map_or("?", |c| c.name.as_str())
            ),
            json!({ "caller": caller.as_str(), "callers": 1, "statements": statements }),
        ));
    }
    Ok(found)
}
