//! The tier of a decision: how far it can be reasoned about without running
//! it. `F` is formal: a cel check the translator reads as a query. `O`
//! orders code: its fix moves or reorders declarations. `T` is the rest, with
//! the reason, so that what widening the fragment would buy can be counted.

#[cfg(test)]
mod tests;

use cel::{
    Program,
    common::ast::{Expr, IdedExpr, LiteralValue},
};
use lighthouse_spec::{CelCheck, CheckKind, Decision, Env, FixKind, children};
use schemars::JsonSchema;
use serde::Serialize;

use crate::translate;

/// The fields that say where code belongs: a check reading one judges the
/// placement of a declaration among files, which no single file shows.
const PLACEMENT: [&str; 4] = ["owner_home", "owner_file", "owned", "pair"];

/// The operations of a fix that order declarations.
const ORDERING: [&str; 2] = ["move", "reorder"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
pub enum Tier {
    /// A formal check: a union of conjunctive queries with comparisons.
    F,
    /// An order check: its fix moves or reorders declarations.
    O,
    /// A check no formal engine reads: a model, a command, a placement
    /// across files, or CEL outside the fragment.
    T,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F => "F",
            Self::O => "O",
            Self::T => "T",
        }
    }
}

/// A tier and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Classified {
    pub tier: Tier,
    /// `translated` for F; `fix:move` or `fix:reorder` for O; for T one of
    /// `no-check`, `no-select`, `model`, `command`, `abstain`, `no-condition`,
    /// `placement` or `outside-fragment:<construct>`.
    pub tier_reason: String,
}

fn classified(tier: Tier, reason: impl Into<String>) -> Classified {
    Classified {
        tier,
        tier_reason: reason.into(),
    }
}

/// The tier of `decision`, read from its document alone.
pub fn tier(env: &Env, decision: &Decision) -> Classified {
    let Some(check) = decision.check.as_ref() else {
        return classified(Tier::T, "no-check");
    };
    let cel = match &check.kind {
        CheckKind::Cel(cel) => cel,
        CheckKind::Command(_) => return classified(Tier::T, "command"),
        CheckKind::Model(_) => return classified(Tier::T, "model"),
    };
    let Some(select) = cel.selects(decision.scope.subject) else {
        return classified(Tier::T, "no-select");
    };
    if cel.abstain.is_some() {
        return classified(Tier::T, "abstain");
    }
    if never(env, cel) {
        return classified(Tier::T, "no-condition");
    }
    if let Some(operation) = orders(env, decision) {
        return classified(Tier::O, format!("fix:{operation}"));
    }
    if reads_placement(env, cel) {
        return classified(Tier::T, "placement");
    }
    match translate(env, cel, select.variable()) {
        Ok(_) => classified(Tier::F, "translated"),
        Err(reject) => classified(Tier::T, format!("outside-fragment:{}", reject.0)),
    }
}

/// Whether the `where` is the literal `false`: the check only asks a model.
fn never(env: &Env, cel: &CelCheck) -> bool {
    env.compile(&cel.condition).is_ok_and(|program| {
        matches!(&program.expression().expr,
            Expr::Literal(LiteralValue::Boolean(b)) if !*b.inner())
    })
}

/// Whether the variables or the `where` read a field of `PLACEMENT`.
fn reads_placement(env: &Env, cel: &CelCheck) -> bool {
    let sources = cel
        .variables
        .iter()
        .map(|v| v.expression.as_str())
        .chain([cel.condition.as_str()]);
    sources
        .filter_map(|source| env.compile(source).ok())
        .any(|program| {
            visit(
                program.expression(),
                &|e| matches!(&e.expr, Expr::Select(s) if PLACEMENT.contains(&s.field.as_str())),
            )
        })
}

/// The ordering operation the fix's edits call, if they call one.
fn orders(env: &Env, decision: &Decision) -> Option<&'static str> {
    let fix = decision.fix.as_ref()?;
    let FixKind::Cel { edits, .. } = &fix.kind else {
        return None;
    };
    let program: Program = env.compile(edits).ok()?;
    ORDERING.into_iter().find(|operation| {
        visit(
            program.expression(),
            &|e| matches!(&e.expr, Expr::Call(call) if call.func_name == *operation),
        )
    })
}

/// Whether `found` holds of the expression or any part of it.
fn visit(expr: &IdedExpr, found: &dyn Fn(&IdedExpr) -> bool) -> bool {
    found(expr) || children(expr).into_iter().any(|child| visit(child, found))
}
