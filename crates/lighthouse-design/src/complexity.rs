use lighthouse_metrics::{COGNITIVE, CYCLOMATIC, NESTING, SIZE, Size, is_dispatcher, read};
use lighthouse_plugin::{Ctx, Error, Rule, RuleMeta};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, functions, skipped};

const ID: &str = "design/complexity-signal";

#[derive(Deserialize)]
struct Thresholds {
    cyclomatic: u32,
    statements: u32,
    structural_cyclomatic: u32,
    structural_statements: u32,
    structural_nesting: u32,
    cognitive: u32,
    cognitive_statements: u32,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(
        ID,
        &[SIZE, CYCLOMATIC, COGNITIVE, NESTING],
        check,
    ))
}

fn check(
    meta: &RuleMeta,
    ctx: &Ctx,
    t: Thresholds,
) -> Result<Vec<lighthouse_model::Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let sizes = read::<Size>(ctx, SIZE)?;
    let cyclomatic = read::<u32>(ctx, CYCLOMATIC)?;
    let cognitive = read::<u32>(ctx, COGNITIVE)?;
    let nesting = read::<u32>(ctx, NESTING)?;
    let mut found = Vec::new();
    for symbol in functions(ctx) {
        let summary = ctx.project.function(&symbol.id);
        if summary.is_none_or(is_dispatcher) {
            continue;
        }
        let (Some(size), Some(&cc), Some(&cog), Some(&depth)) = (
            sizes.get(&symbol.id),
            cyclomatic.get(&symbol.id),
            cognitive.get(&symbol.id),
            nesting.get(&symbol.id),
        ) else {
            continue;
        };
        let statements = size.statements;
        let verdict = if cc >= t.cyclomatic && statements >= t.statements {
            "high complexity"
        } else if cc >= t.structural_cyclomatic
            && statements >= t.structural_statements
            && depth >= t.structural_nesting
        {
            "high structural complexity"
        } else if cog >= t.cognitive && statements >= t.cognitive_statements {
            "high cognitive complexity"
        } else {
            continue;
        };
        found.push(finding(
            meta,
            symbol,
            format!(
                "function {} has {verdict}: cyclomatic={cc} cognitive={cog} statements={statements} nesting={depth}",
                symbol.name
            ),
            json!({
                "cyclomatic": cc,
                "cognitive": cog,
                "statements": statements,
                "nesting": depth,
            }),
        ));
    }
    Ok(found)
}
