//! `model list` and `model train`.

use std::fmt::Write;

use lighthouse_session::{Session, TrainRequest, Trained, bound_models, train};
use serde::Serialize;

use crate::surface::{
    Result,
    args::{ModelListArgs, ModelTrainArgs},
    eval::repos_of,
    list::{omitted_line, page},
    reply::{Context, Output, Reply},
};

/// The `ModelList` kind.
#[derive(Debug, Serialize)]
pub struct ModelList {
    pub models: Vec<lighthouse_session::BoundModel>,
    /// How many models `limit` left out.
    pub omitted: usize,
}

/// `model list`.
pub fn list(ctx: &Context, args: &ModelListArgs, output: Output) -> Result<Reply> {
    let session = Session::load_or_default(ctx.config.as_deref())?;
    let shown = page(bound_models(&session, &args.decision)?, args.limit);
    let list = ModelList {
        models: shown.items,
        omitted: shown.omitted,
    };
    Reply::data(output, &list, || {
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
pub fn train_model(ctx: &Context, args: &ModelTrainArgs, output: Output) -> Result<Reply> {
    let trained = train(&TrainRequest {
        decision: args.id.clone(),
        model: args.model.clone(),
        from_model: args.from_model.clone(),
        baseline: args.baseline.clone(),
        repos: repos_of(ctx, &args.repo)?,
        config: ctx.config.clone(),
    })?;
    Reply::data(output, &trained, || train_text(&trained))
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
    let _ = writeln!(
        out,
        "dataset: {} ({})",
        trained.dataset.display(),
        trained.training_file
    );
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
    out
}
