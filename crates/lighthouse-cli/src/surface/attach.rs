//! `model attach` and `model detach`: a model made the first layer of a
//! decision, or removed from it, by a revision.

use std::fmt::Write;

use lighthouse_session::{
    Session,
    model::{Attachment, Detachment, EvalRequest, attach as attach_model, detach as detach_model},
};
use serde_json::json;

use crate::surface::{
    Result,
    args::{ModelAttachArgs, ModelDetachArgs},
    eval::{default_baseline, labelling_of, rate_text},
    reply::{Context, FOUND, Output, Reply},
};

/// `model attach`: exits 1 and writes nothing when the evaluation does not
/// allow the model.
pub fn attach(ctx: &Context, args: &ModelAttachArgs, output: Output) -> Result<Reply> {
    let root = Session::load_or_default(ctx.config.as_deref())?.root;
    let attachment = attach_model(
        &EvalRequest {
            decision: args.id.clone(),
            labelling: labelling_of(
                ctx,
                &args.repo,
                args.baseline.clone().or_else(|| default_baseline(&args.id)),
            )?,
            candidate: None,
            model: Some(args.model.clone()),
            audit: None,
        },
        ctx.attribution(&root, args.attributed_to.as_deref())?,
    )?;
    let mut value = json!({
        "decision": attachment.decision,
        "model": attachment.model,
        "evaluation": attachment.evaluation,
    });
    match &attachment.revision {
        Some(revision) => value["revision"] = json!(revision),
        None => value["refused"] = json!(attachment.reasons),
    }
    let reply = Reply::data(output, &value, || attached_text(&attachment))?;
    Ok(if attachment.revision.is_some() {
        reply
    } else {
        reply.with_code(FOUND)
    })
}

/// `model detach`.
pub fn detach(ctx: &Context, args: &ModelDetachArgs, output: Output) -> Result<Reply> {
    let root = Session::load_or_default(ctx.config.as_deref())?.root;
    let approval = ctx.attribution(&root, args.attributed_to.as_deref())?;
    let detached = detach_model(ctx.config.as_deref(), &args.id, approval)?;
    let value = json!({
        "revision": detached.revision,
        "decision": detached.decision,
        "model": detached.model,
    });
    Reply::data(output, &value, || detached_text(&detached))
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
            let _ = writeln!(out, "  {name}: {}", rate_text(rate));
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
