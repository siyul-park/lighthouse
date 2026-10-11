//! `fix`, and `check --fix`: apply the fixes the catalog names, verified.

use lighthouse_session::{AppliedRow, DeclinedRow, FixSelection, FixTargets, Fixed, Session};
use serde::Serialize;

use crate::surface::{
    Result,
    args::FixArgs,
    reply::{Context, Output, Reply},
};

/// The `FixReport` kind: what a fix run did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixReport {
    pub dry_run: bool,
    /// The unified diff of every changed file.
    pub diff: String,
    pub applied: Vec<AppliedRow>,
    pub declined: Vec<DeclinedRow>,
    /// How many times fixing was repeated until nothing was left.
    pub rounds: usize,
    pub messages: Vec<String>,
}

/// Fixes what `args` selects. `store` says whether the run is remembered.
pub fn apply(ctx: &Context, args: &FixArgs, store: bool) -> Result<Fixed> {
    if ctx.confine
        && args.fingerprints.is_empty()
        && args.paths.is_empty()
        && args.decision.is_empty()
    {
        return Err(
            "name what to fix: `fingerprints` of findings, `paths`, or `decision`; a fix never defaults to the whole project"
                .into(),
        );
    }
    let session = Session::load(ctx.config.as_deref())?;
    let request = FixSelection {
        targets: FixTargets {
            paths: ctx.paths(&session.root, &args.paths)?,
            fingerprints: args.fingerprints.clone(),
            rules: args.decision.clone(),
        },
        dry_run: args.options.dry_run,
        unsafe_fixes: args.options.unsafe_fixes,
        fixer: args.options.fixer.clone(),
        store,
    };
    lighthouse_session::fix(session, &request)
}

/// The report of a finished fix run.
pub fn report(fixed: &Fixed) -> FixReport {
    FixReport {
        dry_run: fixed.dry_run,
        diff: fixed.diff.clone(),
        applied: fixed.applied(),
        declined: fixed.declined(),
        rounds: fixed.report.rounds,
        messages: fixed.messages.clone(),
    }
}

/// What a person is told about a fix run: the lines for stderr.
pub fn notes(fixed: &Fixed) -> Vec<String> {
    let mut out: Vec<String> = fixed
        .messages
        .iter()
        .map(|m| format!("lighthouse: {m}"))
        .collect();
    let verb = if fixed.dry_run { "would fix" } else { "fixed" };
    for fix in fixed.applied() {
        out.push(format!(
            "{verb} {} [{}] {} ({}): {}",
            fix.rule,
            fix.safety,
            fix.files.join(", "),
            fix.fixer,
            fix.description
        ));
    }
    for fix in fixed.declined() {
        out.push(format!(
            "not fixed {}:{} {}: {}",
            fix.path, fix.line, fix.rule, fix.reason
        ));
    }
    out.push(format!(
        "lighthouse: {} fix(es) {}, {} left alone",
        fixed.applied().len(),
        if fixed.dry_run { "proposed" } else { "applied" },
        fixed.declined().len()
    ));
    out
}

/// The `fix` tool: fix, and say what happened.
pub fn run(ctx: &Context, args: &FixArgs, output: Output) -> Result<Reply> {
    let fixed = apply(ctx, args, true)?;
    let reply = Reply::data(output, &report(&fixed), || fixed.diff.clone())?;
    Ok(reply.with_notes(notes(&fixed)))
}
