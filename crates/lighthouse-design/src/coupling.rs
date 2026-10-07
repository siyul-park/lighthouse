use lighthouse_metrics::{FAN, Fan, SIZE, Size, is_dispatcher, read};
use lighthouse_model::Diagnostic;
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use lighthouse_spec::PatternRule;
use serde::Deserialize;
use serde_json::json;

use crate::{finding, functions, skipped};

const ID: &str = "design/coupling-signal";

#[derive(Deserialize)]
struct Thresholds {
    hub_fan_in: u32,
    hub_fan_out: u32,
    hub_statements: u32,
    coordinator_fan_out: u32,
    coordinator_max_fan_in: u32,
    coordinator_statements: u32,
}

pub(crate) fn rule() -> Box<dyn Rule> {
    Box::new(PatternRule::new(ID, &[FAN, SIZE], check))
}

fn check(meta: &RuleManifest, ctx: &Ctx, t: Thresholds) -> Result<Vec<Diagnostic>, Error> {
    if skipped(ctx) {
        return Ok(Vec::new());
    }
    let fans = read::<Fan>(ctx, FAN)?;
    let sizes = read::<Size>(ctx, SIZE)?;
    let mut found = Vec::new();
    for symbol in functions(ctx) {
        if ctx.project.function(&symbol.id).is_none_or(is_dispatcher) {
            continue;
        }
        let (Some(fan), Some(size)) = (fans.get(&symbol.id), sizes.get(&symbol.id)) else {
            continue;
        };
        let statements = size.statements;
        let role = if fan.fan_in >= t.hub_fan_in
            && fan.fan_out >= t.hub_fan_out
            && statements >= t.hub_statements
        {
            "a dependency hub"
        } else if fan.fan_out >= t.coordinator_fan_out
            && fan.fan_in <= t.coordinator_max_fan_in
            && statements >= t.coordinator_statements
        {
            "a high fan-out coordinator"
        } else {
            continue;
        };
        found.push(finding(
            meta,
            symbol,
            format!(
                "function {} is {role}: fan-in={} fan-out={}",
                symbol.name, fan.fan_in, fan.fan_out
            ),
            json!({
                "fan_in": fan.fan_in,
                "fan_out": fan.fan_out,
                "statements": statements,
            }),
        ));
    }
    Ok(found)
}
