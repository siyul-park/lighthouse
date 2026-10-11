//! `lighthouse model train`, the host's half: the labelled subjects of a
//! decision become the rows of a dataset (the one writer of training files),
//! and a model that learns from labels is asked for a fine-tuning job on it.
//! Finding the subjects takes a project's checks and stays with the caller.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_plugin::{
    Classifier, CreateFineTuningJobRequest, DecisionRef, FineTuneMethod, FineTuneSupervisedMethod,
    FineTuningJob, Question, ResultKind, Subject, SystemOneRequest, UPPER, decide,
};
use serde::Serialize;
use serde_json::json;

use crate::{
    Result,
    dataset::{self, DIR, Row},
    evaluate::Repo,
    labels::{Label, Source},
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

/// What one repository gave to a training run: rows, each with where it came
/// from.
pub struct Rows {
    rows: Vec<Sourced>,
    unresolved: usize,
}

/// A row with where it came from.
struct Sourced {
    row: Row,
    source: Source,
}

impl Rows {
    /// The rows the `labels` make where `observed` (or the label itself) has
    /// the subject; those a `teacher` model is sure of among the subjects
    /// nobody labelled are rows too.
    pub fn of(
        labels: &[Label],
        observed: &BTreeMap<String, Subject>,
        teacher: Option<&dyn Classifier>,
        decision: &DecisionRef,
    ) -> Result<Self> {
        let (mut rows, seen) = rows_of(labels, observed);
        let unresolved = labels
            .iter()
            .filter(|l| l.truth != Truth::NotApplicable)
            .count()
            - rows.len();
        if let Some(teacher) = teacher {
            let unlabelled = observed
                .iter()
                .filter(|(fingerprint, _)| !seen.contains(*fingerprint))
                .map(|(fingerprint, subject)| (fingerprint.clone(), subject.clone()))
                .collect();
            rows.extend(taught(teacher, decision, unlabelled)?);
        }
        Ok(Self { rows, unresolved })
    }
}

/// Where a training run writes and what it trains.
pub struct Target<'a> {
    /// The project root the dataset is stored under.
    pub root: &'a Path,
    /// The model trained, `<provider>/<model>`.
    pub reference: Option<&'a str>,
    pub decision: &'a DecisionRef,
    /// The bound model, to give the job to; it must be able to learn.
    pub model: Option<&'a dyn Classifier>,
}

/// Writes the dataset of the rows of every repository and asks the model for
/// a job on it.
pub fn train(request: &TrainRequest, parts: Vec<Rows>, target: &Target) -> Result<Trained> {
    let unresolved = parts.iter().map(|p| p.unresolved).sum();
    let rows: Vec<Sourced> = parts.into_iter().flat_map(|p| p.rows).collect();
    let mut by_source = BTreeMap::new();
    for row in &rows {
        *by_source.entry(row.source.name().to_owned()).or_insert(0) += 1;
    }
    let count = rows.len();
    let written = dataset::write(
        target.root,
        target.decision,
        rows.into_iter().map(|r| r.row).collect(),
    )?;
    let reference = target.reference;
    let result = match target.model.filter(|model| {
        model
            .manifest()
            .capabilities
            .fine_tuning
            .is_some_and(|capability| capability.supported)
    }) {
        Some(model) => Some(model.train(&job(
            reference.unwrap_or_default(),
            &written.digest,
            target.root,
            &request.decision,
        ))?),
        None => None,
    };
    Ok(Trained {
        decision: request.decision.clone(),
        model: reference.unwrap_or_default().to_owned(),
        subjects: count,
        by_source,
        unresolved,
        dataset: written.path,
        training_file: written.digest,
        result,
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
    decision: &DecisionRef,
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
