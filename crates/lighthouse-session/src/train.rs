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

use crate::{CheckRequest, Checked, ModelFlags, Result, Session, check, judged::judged, samples};

/// What one repository gave to a training run.
pub(crate) struct Gathered {
    pub(crate) root: PathBuf,
    pub(crate) reference: Option<String>,
    pub(crate) decision_ref: DecisionRef,
    /// The run, which holds the models.
    pub(crate) checked: Checked,
}

/// What every repository gave to a training run: the rows of each, and the
/// first one's run.
pub(crate) struct Collected {
    pub(crate) rows: Vec<Rows>,
    pub(crate) first: Gathered,
}

/// Trains the model on the labels of the decision.
pub fn train(request: &TrainRequest) -> Result<Trained> {
    let collected = collect(request)?;
    let first = &collected.first;
    let model = first
        .reference
        .as_deref()
        .map(|r| first.checked.model(r))
        .transpose()?;
    let target = Target {
        root: &first.root,
        reference: first.reference.as_deref(),
        decision: &first.decision_ref,
        model: model.as_deref(),
    };
    lighthouse_ml::train::train(request, collected.rows, &target)
}

/// The labelled subjects of the decision in each repository the labels are
/// about.
pub(crate) fn collect(request: &TrainRequest) -> Result<Collected> {
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
    let first = gathered.into_iter().next().ok_or("nothing to train on")?;
    Ok(Collected { rows: parts, first })
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
        .or_else(|| {
            let route = binding.rules.get(&request.decision);
            route.and_then(|models| models.first()).cloned()
        })
        .or_else(|| binding.classification.first().cloned());
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
        judgments: judged(&labelling.root)?,
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
