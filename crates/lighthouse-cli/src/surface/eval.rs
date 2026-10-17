//! `decision eval`: a decision's check measured against its labels.

use std::{fmt::Write, path::PathBuf};

use lighthouse_session::{
    Session,
    model::{EvalReport, EvalRequest, Labelling, Rate, Repo, Side, eval},
};

use crate::surface::{
    Result,
    args::DecisionEvalArgs,
    reply::{Context, Output, Reply},
};

/// The repositories `--repo name=dir` names, inside the project when the
/// caller is confined.
pub fn repos_of(ctx: &Context, specs: &[String]) -> Result<Vec<Repo>> {
    let root = Session::load_or_default(ctx.config.as_deref())?.root;
    specs
        .iter()
        .map(|spec| {
            let (name, dir) = spec
                .split_once('=')
                .ok_or_else(|| format!("--repo `{spec}` is not name=dir"))?;
            let dir = ctx.paths(&root, &[PathBuf::from(dir)])?.remove(0);
            Ok(Repo {
                name: Some(name.to_owned()),
                dir,
            })
        })
        .collect()
}

/// What the labels come from: the repositories `--repo` names and the table
/// of reviewed findings.
pub fn labelling_of(ctx: &Context, repos: &[String], table: Option<PathBuf>) -> Result<Labelling> {
    Ok(Labelling {
        repos: repos_of(ctx, repos)?,
        table,
        config: ctx.config.clone(),
    })
}

/// `baseline/precision/<decision>.tsv` when it exists.
pub fn default_baseline(decision: &str) -> Option<PathBuf> {
    let path = PathBuf::from(format!("baseline/precision/{decision}.tsv"));
    path.is_file().then_some(path)
}

/// `decision eval`.
pub fn run(ctx: &Context, args: &DecisionEvalArgs, output: Output) -> Result<Reply> {
    let report = eval(&EvalRequest {
        decision: args.id.clone(),
        labelling: labelling_of(
            ctx,
            &args.repo,
            args.baseline.clone().or_else(|| default_baseline(&args.id)),
        )?,
        candidate: args.check.clone(),
        model: args.model.clone(),
        audit: args.audit,
    })?;
    Reply::data(output, &report, || text(&report))
}

pub(crate) fn rate_text(rate: Option<Rate>) -> String {
    rate.map_or_else(
        || "-".to_owned(),
        |r| {
            format!(
                "{:.3} [{:.3}, {:.3}] ({}/{})",
                r.value, r.low, r.high, r.hits, r.of
            )
        },
    )
}

fn text(report: &EvalReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}: {} labelled subject(s); target precision {}{}",
        report.decision,
        report.labelled,
        report.policy.precision,
        report
            .policy
            .recall
            .map_or_else(String::new, |r| format!(", recall {r}"))
    );
    side(&mut out, "current", &report.current);
    if let Some(candidate) = &report.candidate {
        side(&mut out, "candidate", candidate);
    }
    if let Some(rate) = &report.agreement {
        let _ = writeln!(
            out,
            "agreement of the candidate with the current check: {}",
            rate_text(Some(*rate))
        );
    }
    if let Some(meets) = report.meets_policy {
        let _ = writeln!(
            out,
            "verdict: {} the policy (precision {}) by the point estimate",
            if meets { "meets" } else { "does not meet" },
            report.policy.precision
        );
    }
    if let Some(proposal) = &report.proposal {
        let _ = writeln!(
            out,
            "proposal: detach {}: {}; run lighthouse model detach {}",
            proposal.model, proposal.reason, report.decision
        );
    }
    if let Some(path) = &report.audit {
        let _ = writeln!(
            out,
            "audit: label the rows of {} (verdict TP or FP)",
            path.display()
        );
    }
    out
}

fn side(out: &mut String, label: &str, side: &Side) {
    let m = &side.metrics;
    let _ = writeln!(out, "{label} ({}):", side.check);
    if let Some(layer) = &side.layer {
        let _ = writeln!(
            out,
            "  layer 1: {} ({} cross-fitted{})",
            layer.model,
            layer.cross_fitted,
            if layer.refits.is_empty() {
                String::new()
            } else {
                format!(", refits {}", layer.refits.join(", "))
            }
        );
    }
    for (name, rate) in [
        ("precision", m.precision),
        ("FPR", m.fpr),
        ("recall", m.recall),
        ("FNR", m.fnr),
        ("selector precision", m.selector_precision),
        ("coverage", m.coverage),
    ] {
        let _ = writeln!(out, "  {name}: {}", rate_text(rate));
    }
    let score = |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |v| format!("{v:.3}"));
    let _ = writeln!(
        out,
        "  Brier: {}, ECE: {}, AUROC: {} (answered {} of {} labelled)",
        score(m.brier),
        score(m.ece),
        score(m.auroc),
        m.answered,
        m.labelled
    );
    let _ = writeln!(
        out,
        "  decided by a model: {}, left to review: {}, questions asked: {} (cost {})",
        side.usage.model_decided, side.usage.reviews, side.usage.calls, side.usage.cost
    );
    for notice in &side.notices {
        let _ = writeln!(out, "  note: {notice}");
    }
}
