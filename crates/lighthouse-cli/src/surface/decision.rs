//! `decision list|show|create|update|test|eval`.

use std::{
    env,
    fmt::Write,
    fs,
    io::{Read, stdin},
};

use lighthouse_model::Attribution;
use lighthouse_report::{Aim, CheckInfo, Descriptor, FixInfo};
use lighthouse_session::{
    Authored, CheckRequest, Checked, DecisionRow, DecisionTest, Fixing, Session, TestTimings,
    active_decisions,
    ask::{self, Answer, Delta, WhatIf},
    create_decision, decision_rows, explain,
    model::{self, EvalReport, EvalRequest, Side},
    replace_decision,
    signal::{self, Support},
    test_decisions,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use crate::surface::{
    Result,
    args::{
        DecisionAskArgs, DecisionEvalArgs, DecisionListArgs, DecisionShowArgs, DecisionSignalsArgs,
        DecisionTestArgs, SignalDismissArgs,
    },
    list::{Page, omitted_line, page},
    reply::{Context, FOUND, Format, Reply},
};

/// The `DecisionList` kind: the decisions with a rule, or every one.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct DecisionList {
    decisions: Vec<DecisionItem>,
    /// How many decisions `limit` left out.
    omitted: usize,
}

/// One decision of a listing.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct DecisionItem {
    #[serde(flatten)]
    row: DecisionRow,
    /// Whether this project's configuration enables it.
    enabled: bool,
    /// How its check runs here; absent for a decision without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    check: Option<CheckInfo>,
    /// How its fix runs here; absent for a decision without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<FixInfo>,
}

/// The `DecisionShow` kind.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct DecisionShow {
    id: String,
    /// Context, requirement, examples and status.
    markdown: String,
    /// How its check and fix run here; absent for a decision without a check.
    #[serde(flatten)]
    descriptor: Option<Descriptor>,
    /// The signals a proposal derives from; absent for a decision that names
    /// none.
    #[serde(skip_serializing_if = "Option::is_none")]
    support: Option<Support>,
}

/// The `DecisionTest` kind: the examples of the checked decisions, run.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct DecisionTested {
    /// False when any example fails.
    ok: bool,
    #[serde(flatten)]
    ran: DecisionTest,
    /// Where the time went; present only with `timings`.
    #[serde(skip_serializing_if = "Option::is_none")]
    timings: Option<TestTimings>,
}

/// `decision list`.
pub fn list(ctx: &Context, args: &DecisionListArgs, format: Format) -> Result<Reply> {
    let session = ctx.session()?;
    let registry = session.in_process_registry()?;
    let enabled = active_decisions(&session)?;
    let fixing = Fixing::of(&session)?;
    let rows = decision_rows(&session, &registry, args.all);
    let Page { items, omitted } = page(rows, args.limit);
    let decisions: Vec<DecisionItem> = items
        .into_iter()
        .map(|row| {
            let descriptor = session
                .catalog
                .decision(&row.id)
                .and_then(|decision| fixing.describe(decision, Aim::Decision(&row.id)));
            DecisionItem {
                enabled: enabled.contains(&row.id),
                check: descriptor.as_ref().map(|d| d.check),
                fix: descriptor.and_then(|d| d.fix),
                row,
            }
        })
        .collect();
    let list = DecisionList { decisions, omitted };
    Reply::data(format, &list, || {
        let mut out = String::new();
        for DecisionItem { row, .. } in &list.decisions {
            let _ = if args.all {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{}",
                    row.id,
                    row.status,
                    row.severity,
                    row.tier.as_ref().map_or("-", |t| t.tier.as_str()),
                    row.title
                )
            } else {
                writeln!(out, "{}\t{}\t{}", row.id, row.severity, row.title)
            };
        }
        out.push_str(&omitted_line(omitted));
        if args.all {
            out.push_str(&tier_line(&list.decisions));
        }
        out
    })
}

/// `decision show`.
pub fn show(ctx: &Context, args: &DecisionShowArgs, format: Format) -> Result<Reply> {
    let session = ctx.session()?;
    let registry = session.in_process_registry()?;
    let mut markdown = explain(&session.catalog, &registry, &args.id)?;
    let descriptor = match session.catalog.decision(&args.id) {
        Some(decision) => Fixing::of(&session)?.describe(decision, Aim::Decision(&args.id)),
        None => None,
    };
    if let Some(descriptor) = &descriptor {
        let _ = writeln!(
            markdown,
            "check: {} ({})\n{}",
            descriptor.check.by.as_str(),
            if descriptor.check.runs {
                "runs"
            } else {
                "does not run here"
            },
            descriptor.line("<fingerprint>", false)
        );
    }
    let support = match session.catalog.decision(&args.id) {
        Some(decision) => signal::support(&session, decision)?,
        None => None,
    };
    if let Some(support) = &support {
        let _ = writeln!(markdown, "support: {}", support.line());
    }
    let shown = DecisionShow {
        id: args.id.clone(),
        markdown: markdown.clone(),
        descriptor,
        support,
    };
    Reply::data(format, &shown, || markdown)
}

/// `decision create`: `document` is YAML or JSON text.
pub fn create(ctx: &Context, document: &str, format: Format) -> Result<Reply> {
    write(ctx, document, format, create_decision)
}

/// `decision signals`.
pub fn signals(ctx: &Context, args: &DecisionSignalsArgs, format: Format) -> Result<Reply> {
    let report = signal::report(&ctx.session()?, args.all, args.limit)?;
    Reply::data(format, &report, || {
        let mut out = String::new();
        for s in &report.signals {
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}",
                s.id,
                s.class,
                s.status.name(),
                if s.rule.is_empty() { "-" } else { &s.rule },
                s.excerpt
            );
        }
        out.push_str(&omitted_line(report.omitted));
        let counts = &report.counts;
        let _ = writeln!(
            out,
            "{} signal(s): {}; {} abstained; by status: {}",
            counts.total,
            tally(&counts.by_class),
            counts.abstained,
            tally(&counts.by_status)
        );
        if !counts.by_rule.is_empty() {
            let _ = writeln!(out, "by rule: {}", tally(&counts.by_rule));
        }
        for (name, rate) in [
            ("capture", report.measures.capture),
            ("duplicate", report.measures.duplicate),
            ("ratification", report.measures.ratification),
        ] {
            if let Some(rate) = rate {
                let _ = writeln!(out, "{name}: {rate}");
            }
        }
        out
    })
}

/// `decision signal dismiss`.
pub fn dismiss(ctx: &Context, args: &SignalDismissArgs, format: Format) -> Result<Reply> {
    let by = ctx.attribution(args.attributed.attributed_to.as_deref())?;
    let dismissed = signal::dismiss(&ctx.session()?, &args.id, &args.reason, &by)?;
    Reply::data(format, &dismissed, || {
        format!("dismissed signal {}\n", dismissed.id)
    })
}

/// `decision update`: `document` is YAML or JSON text.
pub fn update(ctx: &Context, document: &str, format: Format) -> Result<Reply> {
    write(ctx, document, format, replace_decision)
}

/// The text of the document a command line names: a file, or `-` for stdin.
pub fn read_document(source: &str) -> Result<String> {
    if source == "-" {
        let mut text = String::new();
        stdin().read_to_string(&mut text)?;
        return Ok(text);
    }
    fs::read_to_string(source).map_err(|e| format!("{source}: {e}").into())
}

/// `decision ask`: `args.edit` is the text of the diff.
pub fn ask(ctx: &Context, args: &DecisionAskArgs, format: Format) -> Result<Reply> {
    let session = ctx.session()?;
    let question = WhatIf {
        edit: &args.edit,
        paths: &ctx.paths(&session.root, &args.paths)?,
        // The text names every finding; the detail is the JSON's.
        detail: match format {
            Format::Text => lighthouse_report::Detail::Full,
            Format::Json => args.detail.into(),
        },
    };
    let mut check = |request: CheckRequest| -> lighthouse_session::Result<Checked> {
        match ctx.kept() {
            Some(mut warm) => warm.check((&env::current_dir()?, ctx.config.as_deref()), |_| {
                Ok(request)
            }),
            None => lighthouse_session::check(&session, &request),
        }
    };
    let answer = ask::what_if(&session, &question, &mut check)?;
    let code = answer.code();
    Ok(Reply::data(format, &answer, || answered(&answer))?.with_code(code))
}

/// `decision test`.
pub fn test(ctx: &Context, args: &DecisionTestArgs, format: Format) -> Result<Reply> {
    let session = ctx.session()?;
    let ran = test_decisions(
        &session,
        &args.decision,
        args.language.as_deref(),
        args.jobs,
    )?;
    let report = DecisionTested {
        ok: ran.failures.is_empty(),
        timings: args.timings.then(|| ran.timings.clone()),
        ran,
    };
    let code = if report.ok { 0 } else { FOUND };
    let reply = Reply::data(format, &report, || {
        let mut out = String::new();
        for failure in &report.ran.failures {
            let _ = writeln!(out, "FAIL {failure}");
        }
        for skipped in &report.ran.skipped {
            let _ = writeln!(out, "SKIP {skipped}");
        }
        for conflict in &report.ran.conflicts {
            let _ = writeln!(out, "CONFLICT {conflict}");
        }
        let _ = writeln!(
            out,
            "decision test: {} decision(s) in {} language(s), {} run(s), {} failure(s), {} skipped, {} conflict(s)",
            report.ran.decisions,
            report.ran.languages,
            report.ran.runs,
            report.ran.failures.len(),
            report.ran.skipped.len(),
            report.ran.conflicts.len()
        );
        out
    })?;
    let mut reply = reply.with_code(code);
    if let (Some(timings), Format::Text) = (&report.timings, format) {
        reply.notes.extend(timings.lines());
    }
    Ok(reply)
}

/// `decision eval`.
pub fn eval(ctx: &Context, args: &DecisionEvalArgs, format: Format) -> Result<Reply> {
    let report = model::eval(&EvalRequest {
        decision: args.id.clone(),
        labelling: args.labels.resolve(ctx, Some(&args.id))?,
        candidate: args.check.clone(),
        model: args.model.clone(),
        audit: args.audit,
    })?;
    Reply::data(format, &report, || eval_text(&report))
}

/// How many of the listed decisions are in each tier.
fn tier_line(items: &[DecisionItem]) -> String {
    let count = |wanted: &str| {
        items
            .iter()
            .filter(|i| {
                i.row
                    .tier
                    .as_ref()
                    .is_some_and(|t| t.tier.as_str() == wanted)
            })
            .count()
    };
    format!(
        "tiers: F {}, O {}, T {}\n",
        count("F"),
        count("O"),
        count("T")
    )
}

/// The answer as text: the verdict, then a line for each finding the edit
/// adds or removes.
fn answered(answer: &Answer) -> String {
    let mut out = format!("{}\n", answer.verdict);
    for (label, delta) in [("fired", &answer.fired), ("cleared", &answer.cleared)] {
        let Delta::Full(items) = delta else { continue };
        for item in items {
            let fix = item.fix.as_ref().map_or(String::new(), |fix| {
                format!(" (fix: {}, {})", fix.safety, fix.by.as_str())
            });
            let _ = writeln!(
                out,
                "{label} {}:{}:{}: {} {}: {}{fix}",
                item.path, item.line, item.col, item.severity, item.decision, item.message
            );
        }
    }
    let _ = writeln!(out, "unchanged: {}", answer.unchanged);
    if !answer.gaps.is_empty() {
        let _ = writeln!(
            out,
            "undecided: {} enabled decision(s) a model decides",
            answer.gaps.len()
        );
    }
    for gap in &answer.incomplete {
        let path = gap
            .path
            .as_ref()
            .map_or("-".to_owned(), |p| p.display().to_string());
        let _ = writeln!(out, "incomplete: {path}: {}", gap.reason);
    }
    out
}

fn tally(counts: &std::collections::BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "-".to_owned();
    }
    counts
        .iter()
        .map(|(name, n)| format!("{name} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Writes a decision with `make` once its document parses.
fn write(
    ctx: &Context,
    text: &str,
    format: Format,
    make: fn(&Session, Value, &Attribution) -> Result<Authored>,
) -> Result<Reply> {
    let document: Value = serde_norway::from_str(text)
        .map_err(|e| format!("the document is not valid YAML or JSON: {e}"))?;
    let done = make(
        &Session::load(ctx.config.as_deref())?,
        document,
        &ctx.attribution(None)?,
    )?;
    let verb = if done.created { "created" } else { "updated" };
    Reply::data(format, &done, || {
        format!(
            "{verb} {} at {}\n{}\n",
            done.id,
            done.path.display(),
            done.note
        )
    })
}

fn eval_text(report: &EvalReport) -> String {
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
            "agreement of the candidate with the current check: {rate}"
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
        let _ = writeln!(
            out,
            "  {name}: {}",
            rate.map_or("-".to_owned(), |r| r.to_string())
        );
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
        side.usage.model_decided, side.usage.reviews, side.usage.usage.calls, side.usage.usage.cost
    );
    for notice in &side.notices {
        let _ = writeln!(out, "  note: {notice}");
    }
}
