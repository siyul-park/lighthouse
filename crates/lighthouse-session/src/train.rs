//! `lighthouse model train`: collects the labelled subjects of a decision the
//! way evaluation does, writes them as a dataset (the one writer of training
//! files), and asks a model that learns from labels for a fine-tuning job on
//! it.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
};

use lighthouse_engine::ModelOptions;
use lighthouse_plugin::{
    Classifier, CreateFineTuningJobRequest, FineTuneMethod, FineTuneSupervisedMethod,
    FineTuningJob, Question, ResultKind, Subject, SystemOneRequest, UPPER, decide,
};
use serde::Serialize;
use serde_json::json;

use crate::{
    CheckRequest, ModelFlags, Result, Session, check,
    dataset::{self, DIR, Row},
    evaluate::Repo,
    labels::{Baseline, Collect, Label, Source},
    metrics::Truth,
};

/// What to train.
pub struct TrainRequest {
    pub decision: String,
    /// The model to train (`<provider>/<model>`); else the decision's bound
    /// model.
    pub model: Option<String>,
    /// A model whose answers on unlabelled subjects are labels too, at the
    /// weight of a model: a `fail` where it is sure of one, a `pass` where it
    /// is sure of the other.
    pub from_model: Option<String>,
    pub baseline: Option<PathBuf>,
    /// The repositories the labels are about, as in `eval`; this project when
    /// empty.
    pub repos: Vec<Repo>,
    pub config: Option<PathBuf>,
}

/// What a training run did.
#[derive(Debug, Serialize)]
pub struct Trained {
    pub decision: String,
    pub model: String,
    /// Labelled subjects handed over, by source.
    pub subjects: usize,
    pub by_source: BTreeMap<String, usize>,
    /// Labels whose subject no longer exists or is not selected.
    pub unresolved: usize,
    /// The training file, by content.
    pub dataset: PathBuf,
    /// Its digest, `sha256:<hex>`.
    pub training_file: String,
    /// The job the trainer gave; `None` when no model that learns is bound.
    pub result: Option<FineTuningJob>,
}

/// What one repository gave to a training run.
struct Gathered {
    rows: Vec<Sourced>,
    unresolved: usize,
    root: PathBuf,
    reference: Option<String>,
    decision_ref: lighthouse_plugin::DecisionRef,
    /// The run, which holds the models.
    checked: crate::Checked,
}

/// A row with where it came from.
struct Sourced {
    row: Row,
    source: Source,
}

/// Trains the model on the labels of the decision.
pub fn train(request: &TrainRequest) -> Result<Trained> {
    let start = env::current_dir()?;
    let config = request.config.as_ref().map(|c| start.join(c));
    let baseline = request.baseline.as_ref().map(|b| start.join(b));
    let repos: Vec<Repo> = if request.repos.is_empty() {
        vec![Repo {
            name: None,
            dir: start.clone(),
        }]
    } else {
        request
            .repos
            .iter()
            .map(|r| Repo {
                name: r.name.clone(),
                dir: start.join(&r.dir),
            })
            .collect()
    };
    let mut gathered: Vec<Gathered> = Vec::new();
    let outcome = (|| -> Result<()> {
        for (at, repo) in repos.iter().enumerate() {
            env::set_current_dir(&repo.dir)?;
            let one = gather(
                request,
                repo,
                config.as_deref(),
                baseline.as_deref(),
                at == 0,
            )?;
            gathered.push(one);
        }
        Ok(())
    })();
    env::set_current_dir(&start)?;
    outcome?;
    let unresolved = gathered.iter().map(|g| g.unresolved).sum();
    let rows: Vec<Sourced> = gathered.iter_mut().flat_map(|g| g.rows.drain(..)).collect();
    let first = gathered.first().ok_or("nothing to train on")?;
    let (root, reference, decision_ref) = (
        first.root.clone(),
        first.reference.clone(),
        first.decision_ref.clone(),
    );
    let mut by_source = BTreeMap::new();
    for row in &rows {
        *by_source.entry(row.source.name().to_owned()).or_insert(0) += 1;
    }
    let count = rows.len();
    let written = dataset::write(
        &root,
        &decision_ref,
        rows.into_iter().map(|r| r.row).collect(),
    )?;
    let trainer = reference
        .as_deref()
        .map(|r| first.checked.model(r))
        .transpose()?;
    let result = match trainer.filter(|model| {
        model
            .manifest()
            .capabilities
            .fine_tuning
            .is_some_and(|capability| capability.supported)
    }) {
        Some(model) => Some(model.train(&job(
            reference.as_deref().unwrap_or_default(),
            &written.digest,
            &root,
            &request.decision,
        ))?),
        None => None,
    };
    Ok(Trained {
        decision: request.decision.clone(),
        model: reference.unwrap_or_default(),
        subjects: count,
        by_source,
        unresolved,
        dataset: written.path,
        training_file: written.digest,
        result,
    })
}

/// The labelled subjects of the repository the process is in.
fn gather(
    request: &TrainRequest,
    repo: &Repo,
    config: Option<&Path>,
    baseline: Option<&Path>,
    examples: bool,
) -> Result<Gathered> {
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
    let labels = Collect {
        session: &labelling,
        catalog: &checked.catalog,
        decision: &decision,
        rule: &request.decision,
        baseline: table.as_ref(),
        exclude_model: reference.as_deref(),
        models: ModelOptions {
            off: true,
            ..ModelOptions::default()
        },
        examples,
    }
    .labels()?;
    let observed = &checked.outcome.subjects;
    let decision_ref = decision.reference();
    let (mut rows, seen) = rows_of(&labels, observed);
    let unresolved = labels
        .iter()
        .filter(|l| l.truth != Truth::NotApplicable)
        .count()
        - rows.len();
    if let Some(teacher) = &request.from_model {
        let unlabelled = observed
            .iter()
            .filter(|(fingerprint, _)| !seen.contains(*fingerprint))
            .map(|(fingerprint, subject)| (fingerprint.clone(), subject.clone()))
            .collect();
        let teacher = checked.model(teacher)?;
        rows.extend(taught(teacher.as_ref(), &decision_ref, unlabelled)?);
    }
    Ok(Gathered {
        rows,
        unresolved,
        root,
        reference,
        decision_ref,
        checked,
    })
}

/// The labels with a subject to learn from, as rows, and the keys they cover.
/// A `notApplicable` label teaches nothing to a classifier.
fn rows_of(
    labels: &[Label],
    observed: &BTreeMap<String, Subject>,
) -> (Vec<Sourced>, BTreeSet<String>) {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for label in labels {
        let kind = match label.truth {
            Truth::Fail => ResultKind::Fail,
            Truth::Pass => ResultKind::Pass,
            Truth::NotApplicable => continue,
        };
        let Some(subject) = label.subject.as_ref().or_else(|| observed.get(&label.key)) else {
            continue;
        };
        seen.insert(label.key.clone());
        rows.push(Sourced {
            row: Row {
                subject: subject.clone(),
                label: kind,
                weight: label.weight,
                key: label.key.clone(),
                attribution: label.attribution.clone(),
            },
            source: label.source,
        });
    }
    (rows, seen)
}

/// What a teacher model decides about subjects nobody labelled: a `fail` where
/// it is at least as sure as the band's upper bound, a `pass` where it is as
/// sure the other way; what it is unsure of teaches nothing.
fn taught(
    teacher: &dyn Classifier,
    decision: &lighthouse_plugin::DecisionRef,
    unlabelled: Vec<(String, Subject)>,
) -> Result<Vec<Sourced>> {
    let question = Question::of(decision);
    let attribution = json!({"type": "SoftwareAgent", "hadRole": "model"});
    let mut labelled = Vec::new();
    for (key, subject) in unlabelled {
        let answered = teacher.classify(&SystemOneRequest {
            state: subject.state(),
            model: String::new(),
            questions: BTreeMap::from([(decision.id.clone(), question.clone())]),
            options: serde_json::Map::new(),
        })?;
        let Some(noul) = answered.answers.get(&decision.id).map(|a| a.noul) else {
            continue;
        };
        let kind = if decide(noul, UPPER).is_some() {
            ResultKind::Fail
        } else if decide(1.0 - noul, UPPER).is_some() {
            ResultKind::Pass
        } else {
            continue;
        };
        labelled.push(Sourced {
            row: Row {
                subject,
                label: kind,
                weight: Source::Model.weight(),
                key,
                attribution: attribution.clone(),
            },
            source: Source::Model,
        });
    }
    Ok(labelled)
}

/// OpenAI's fine-tuning job creation for the dataset: the same dataset for the
/// same base model is the same job (its seed comes from the digest).
fn job(
    reference: &str,
    training_file: &str,
    root: &Path,
    decision: &str,
) -> CreateFineTuningJobRequest {
    let model = reference.split_once('/').map_or(reference, |(_, m)| m);
    let seed = u32::from_str_radix(&training_file["sha256:".len()..][..8], 16).unwrap_or(0);
    CreateFineTuningJobRequest {
        model: model.to_owned(),
        training_file: training_file.to_owned(),
        validation_file: None,
        method: FineTuneMethod {
            kind: "supervised".to_owned(),
            supervised: FineTuneSupervisedMethod::default(),
        },
        seed,
        suffix: None,
        metadata: BTreeMap::from([
            ("decision".to_owned(), decision.to_owned()),
            (
                "datasets".to_owned(),
                root.join(DIR).to_string_lossy().into_owned(),
            ),
            (
                "dir".to_owned(),
                artifact_dir(root, reference, decision)
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]),
        options: serde_json::Map::new(),
    }
}

/// Where a model keeps what it learned for a decision:
/// `.lighthouse/models/<provider>/<decision>/`.
fn artifact_dir(root: &Path, reference: &str, decision: &str) -> PathBuf {
    let provider = reference.split('/').next().unwrap_or(reference);
    root.join(".lighthouse/models")
        .join(provider)
        .join(decision.replace('/', "-"))
}
