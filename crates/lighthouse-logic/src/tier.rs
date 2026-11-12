//! The tier of a decision: how far it can be reasoned about without running
//! it. `F` is formal: a cel check the translator reads as a query. `O`
//! orders code: its fix moves or reorders declarations. `T` is the rest, with
//! the reason, so that what widening the fragment would buy can be counted.
//!
//! Known limit: a `move` to `before(x)` or `after(x)` is O even when `x` is in
//! another file; only the `start` and `end` anchors, which name a file, are
//! read as placement.

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

/// The operations of a fix that order declarations.
const ORDERING: [&str; 2] = ["move", "reorder"];

/// The anchors of `move` that name a file, so the declaration may leave its own.
const FILE_ANCHORS: [&str; 2] = ["start", "end"];

/// A tier and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Classified {
    pub tier: Tier,
    /// `translated` for F; `fix:move` or `fix:reorder` for O; for T one of
    /// `no-check`, `no-select`, `model`, `command`, `abstain`, `no-condition`,
    /// `placement` or `outside-fragment:<construct>`, the construct a stable token
    /// such as `ternary`, `index`, `flatten`, `two-fields`.
    pub tier_reason: String,
}

/// How far a decision can be reasoned about without running it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, JsonSchema)]
pub enum Tier {
    /// A formal check: a union of conjunctive queries with comparisons.
    F,
    /// An order check: its fix moves or reorders declarations.
    O,
    /// A check no formal engine reads: a model, a command, a move across
    /// files, or CEL outside the fragment.
    T,
}

impl Tier {
    /// The letter of the tier: `F`, `O` or `T`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::F => "F",
            Self::O => "O",
            Self::T => "T",
        }
    }
}

/// How a fix orders declarations.
enum Ordering {
    /// Within the file: `move` to a declaration's side, or `reorder`.
    Within(&'static str),
    /// A `move` to the start or end of a file, which may be another.
    Across,
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
    match orders(env, decision) {
        Some(Ordering::Across) => return classified(Tier::T, "placement"),
        Some(Ordering::Within(operation)) => {
            return classified(Tier::O, format!("fix:{operation}"));
        }
        None => {}
    }
    match translate(env, cel, select.variable()) {
        Ok(_) => classified(Tier::F, "translated"),
        Err(reject) => classified(Tier::T, format!("outside-fragment:{}", reject.construct())),
    }
}

fn classified(tier: Tier, reason: impl Into<String>) -> Classified {
    Classified {
        tier,
        tier_reason: reason.into(),
    }
}

/// Whether the `where` is the literal `false`: the check only asks a model.
fn never(env: &Env, cel: &CelCheck) -> bool {
    env.compile(&cel.condition).is_ok_and(|program| {
        matches!(&program.expression().expr,
            Expr::Literal(LiteralValue::Boolean(b)) if !*b.inner())
    })
}

/// The ordering the fix's edits call, if they call one.
fn orders(env: &Env, decision: &Decision) -> Option<Ordering> {
    let fix = decision.fix.as_ref()?;
    let FixKind::Cel { edits, .. } = &fix.kind else {
        return None;
    };
    let program: Program = env.compile(edits).ok()?;
    let root = program.expression();
    if visit(root, &|e| {
        matches!(&e.expr, Expr::Call(call) if call.func_name == "move"
        && call.args.get(1).is_some_and(|anchor| matches!(&anchor.expr,
            Expr::Call(a) if FILE_ANCHORS.contains(&a.func_name.as_str()))))
    }) {
        return Some(Ordering::Across);
    }
    ORDERING
        .into_iter()
        .find(|operation| {
            visit(
                root,
                &|e| matches!(&e.expr, Expr::Call(call) if call.func_name == *operation),
            )
        })
        .map(Ordering::Within)
}

/// Whether `found` holds of the expression or any part of it.
fn visit(expr: &IdedExpr, found: &dyn Fn(&IdedExpr) -> bool) -> bool {
    found(expr) || children(expr).into_iter().any(|child| visit(child, found))
}
