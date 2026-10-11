//! `lighthouse model train`: collects the labelled subjects of a decision the
//! way evaluation does, in each repository the labels are about, and hands
//! them to the host's training (`lighthouse_ml::train`).

use std::{
    env,
    path::{Path, PathBuf},
};

use lighthouse_engine::ModelOptions;
use lighthouse_ml::{
    evaluate::{Repo, repos},
    labels::{Baseline, Collect},
    train::{Rows, Target, TrainRequest, Trained},
};
use lighthouse_plugin::DecisionRef;

use crate::{CheckRequest, Checked, ModelFlags, Result, Session, check, samples};

/// What one repository gave to a training run.
struct Gathered {
    root: PathBuf,
    reference: Option<String>,
    decision_ref: DecisionRef,
    /// The run, which holds the models.
    checked: Checked,
}

/// Trains the model on the labels of the decision.
pub fn train(request: &TrainRequest) -> Result<Trained> {
    let start = env::current_dir()?;
    let config = request.config.as_ref().map(|c| start.join(c));
    let baseline = request.baseline.as_ref().map(|b| start.join(b));
    let mut parts: Vec<Rows> = Vec::new();
    let mut gathered: Vec<Gathered> = Vec::new();
    let outcome = (|| -> Result<()> {
        for (at, repo) in repos(&request.repos, &start).iter().enumerate() {
            env::set_current_dir(&repo.dir)?;
            let one = gather(
                request,
                repo,
                config.as_deref(),
                baseline.as_deref(),
                at == 0,
            )?;
            parts.push(one.0);
            gathered.push(one.1);
        }
        Ok(())
    })();
    env::set_current_dir(&start)?;
    outcome?;
    let first = gathered.first().ok_or("nothing to train on")?;
    let (root, reference, decision_ref) = (
        first.root.clone(),
        first.reference.clone(),
        first.decision_ref.clone(),
    );
    let model = reference
        .as_deref()
        .map(|r| first.checked.model(r))
        .transpose()?;
    let target = Target {
        root: &root,
        reference: reference.as_deref(),
        decision: &decision_ref,
        model: model.as_deref(),
    };
    lighthouse_ml::train::train(request, parts, &target)
}

/// The labelled subjects of the repository the process is in.
fn gather(
    request: &TrainRequest,
    repo: &Repo,
    config: Option<&Path>,
    baseline: Option<&Path>,
    examples: bool,
) -> Result<(Rows, Gathered)> {
    let labelling = Session::load(config)?;
    let session = Session::load(config)?;
    let root = session.root.clone();
    let binding = session.config.models().clone();
    let checked = check(
        session,
        &CheckRequest {
            rules: vec![request.decision.clone()],
            store: false,
            cache: false,
            models: ModelFlags {
                off: true,
                collect: true,
                ..ModelFlags::default()
            },
            ..CheckRequest::default()
        },
    )?;
    let decision = checked
        .catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?
        .clone();
    let reference = request
        .model
        .clone()
        .or_else(|| binding.rules.get(&request.decision).cloned())
        .or_else(|| binding.classification.clone());
    let table = baseline.map(|file| Baseline {
        file,
        repo: repo.name.as_deref(),
    });
    let shown = if examples {
        samples::shown(
            &labelling,
            &checked.catalog,
            &decision,
            &request.decision,
            &ModelOptions {
                off: true,
                ..ModelOptions::default()
            },
        )?
    } else {
        Vec::new()
    };
    let labels = Collect {
        root: &labelling.root,
        decision: &decision,
        baseline: table.as_ref(),
        exclude_model: reference.as_deref(),
        examples: shown,
    }
    .labels()?;
    let decision_ref = decision.reference();
    let teacher = request
        .from_model
        .as_deref()
        .map(|teacher| checked.model(teacher))
        .transpose()?;
    let rows = Rows::of(
        &labels,
        &checked.outcome.subjects,
        teacher.as_deref(),
        &decision_ref,
    )?;
    drop(teacher);
    Ok((
        rows,
        Gathered {
            root,
            reference,
            decision_ref,
            checked,
        },
    ))
}
