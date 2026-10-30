//! `model list|train|attach|detach|rebuild`.

use std::fmt::Write;

use lighthouse_session::model::{
    self, Attachment, BoundModel, Detachment, EvalRequest, RebuildRequest, Rebuilt, TrainRequest,
    Trained,
};
use schemars::JsonSchema;
use serde::Serialize;

use crate::surface::{
    Result,
    args::{ModelAttachArgs, ModelDetachArgs, ModelListArgs, ModelRebuildArgs, ModelTrainArgs},
    list::{omitted_line, page},
    reply::{Context, FOUND, Format, Reply},
};

/// The `ModelList` kind.
#[derive(Debug, Serialize, JsonSchema)]
pub(super) struct ModelList {
    models: Vec<BoundModel>,
    /// How many models `limit` left out.
    omitted: usize,
}

/// `model list`.
pub fn list(ctx: &Context, args: &ModelListArgs, format: Format) -> Result<Reply> {
    let session = ctx.session()?;
    let shown = page(model::bound_models(&session, &args.decision)?, args.limit);
    let list = ModelList {
        models: shown.items,
        omitted: shown.omitted,
    };
    Reply::data(format, &list, || {
        let mut out = String::new();
        for m in &list.models {
            let _ = writeln!(
                out,
                "{}\t{}\t{:?}\t{}",
                m.decision.as_deref().unwrap_or("-"),
                m.model,
                m.binding,
                if m.remote { "remote" } else { "local" }
            );
        }
        out.push_str(&omitted_line(list.omitted));
        out
    })
}

/// `model train`: trains and reports what the trainer measured.
pub fn train(ctx: &Context, args: &ModelTrainArgs, format: Format) -> Result<Reply> {
    let trained = model::train(&TrainRequest {
        decision: args.id.clone(),
        model: args.model.clone(),
        from_model: args.from_model.clone(),
        labelling: args.labels.resolve(ctx, Some(&args.id))?,
    })?;
    Reply::data(format, &trained, || train_text(&trained))
}

/// `model attach`: exits 1 and writes nothing when the evaluation does not
/// allow the model.
pub fn attach(ctx: &Context, args: &ModelAttachArgs, format: Format) -> Result<Reply> {
    let attachment = model::attach(
        &EvalRequest {
            decision: args.id.clone(),
            labelling: args.labels.resolve(ctx, Some(&args.id))?,
            candidate: None,
            model: Some(args.model.clone()),
            audit: None,
        },
        ctx.attribution(args.attributed.attributed_to.as_deref())?,
    )?;
    let reply = Reply::data(format, &attachment, || attached_text(&attachment))?;
    Ok(if attachment.revision.is_some() {
        reply
    } else {
        reply.with_code(FOUND)
    })
}

/// `model detach`.
pub fn detach(ctx: &Context, args: &ModelDetachArgs, format: Format) -> Result<Reply> {
    let approval = ctx.attribution(args.attributed.attributed_to.as_deref())?;
    let detached = model::detach(ctx.config.as_deref(), &args.id, approval)?;
    Reply::data(format, &detached, || detached_text(&detached))
}

/// `model rebuild`: exits 1 unless every model is present or made again.
pub fn rebuild(ctx: &Context, args: &ModelRebuildArgs, format: Format) -> Result<Reply> {
    let one = match args.decision.as_slice() {
        [one] => Some(one.as_str()),
        _ => None,
    };
    let rows = model::rebuild(&RebuildRequest {
        decisions: args.decision.clone(),
        labelling: args.labels.resolve(ctx, one)?,
    })?;
    let holds = rows.iter().all(|row| row.status.holds());
    let reply = Reply::data(format, &rows, || rebuilt_text(&rows))?;
    Ok(if holds { reply } else { reply.with_code(FOUND) })
}

fn train_text(trained: &Trained) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} subject(s) for {} ({} label(s) found no subject)",
        trained.subjects, trained.decision, trained.unresolved
    );
    for (source, n) in &trained.by_source {
        let _ = writeln!(out, "  {source}: {n}");
    }
    let _ = writeln!(out, "dataset: {}", trained.training_file);
    match &trained.result {
        Some(job) => {
            let _ = writeln!(
                out,
                "trained {}: {} ({:?})",
                trained.model,
                job.fine_tuned_model.as_deref().unwrap_or("-"),
                job.status
            );
        }
        None => out.push_str("no model that learns is bound: nothing was trained\n"),
    }
    if let Some(card) = &trained.card {
        let _ = writeln!(out, "{}", card_line(card));
    }
    out
}

/// What the trainer measured, in a line: the candidate it chose, its Brier
/// score out of fold and the abstain band.
fn card_line(card: &serde_json::Value) -> String {
    let number = |pointer: &str, places: usize| {
        card.pointer(pointer)
            .and_then(serde_json::Value::as_f64)
            .map_or_else(|| "-".to_owned(), |n| format!("{n:.places$}"))
    };
    format!(
        "chosen {} (brier {}), band upper {} lower {}",
        card.pointer("/chosen")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("-"),
        number("/metrics/brier", 4),
        number("/band/upper", 4),
        number("/band/lower", 4),
    )
}

fn attached_text(attachment: &Attachment) -> String {
    let mut out = String::new();
    let metrics = attachment
        .report
        .candidate
        .as_ref()
        .map(|side| &side.metrics);
    match &attachment.revision {
        Some(revision) => {
            let _ = writeln!(
                out,
                "attached {} to {} as revision {revision}",
                attachment.model, attachment.decision
            );
        }
        None => {
            let _ = writeln!(
                out,
                "refused: {} is not attached to {}",
                attachment.model, attachment.decision
            );
            for reason in &attachment.reasons {
                let _ = writeln!(out, "  - {reason}");
            }
        }
    }
    let _ = writeln!(
        out,
        "evaluation on {} labelled subject(s), {} decided that the check left for review:",
        attachment.report.labelled, attachment.report.decided
    );
    if let Some(metrics) = metrics {
        for (name, rate) in [
            ("precision", metrics.precision),
            ("recall", metrics.recall),
            ("coverage", metrics.coverage),
        ] {
            let _ = writeln!(
                out,
                "  {name}: {}",
                rate.map_or("-".to_owned(), |r| r.to_string())
            );
        }
    }
    out
}

fn detached_text(detached: &Detachment) -> String {
    format!(
        "detached {} from {} as revision {}\n",
        detached.model, detached.decision, detached.revision
    )
}

fn rebuilt_text(rows: &[Rebuilt]) -> String {
    let mut out = String::new();
    for row in rows {
        let status = serde_json::to_value(row.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let _ = write!(out, "{}\t{}\t{status}", row.decision, row.model);
        if let Some(detail) = &row.detail {
            let _ = write!(out, "\t{detail}");
        }
        out.push('\n');
    }
    out
}
