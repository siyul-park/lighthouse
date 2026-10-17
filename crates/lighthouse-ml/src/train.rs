//! `lighthouse model train`, the host's half: the labelled subjects of a
//! decision become the rows of a dataset (the one writer of training files),
//! and a model that learns from labels is asked for a fine-tuning job on it.
//! Finding the subjects takes a project's checks and stays with the caller.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_model::{Judgment, ModelRef, Result};
use lighthouse_plugin::{
    Classifier, CreateFineTuningJobRequest, DATASETS_DIR, DecisionRef, FineTuneMethod,
    FineTuneSupervisedMethod, FineTuningJob, MODELS_DIR, Subject, fails, request_of,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    artifact,
    dataset::{self, Row},
    labels::{Label, Source},
};

/// What a training run did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trained {
    pub decision: String,
    pub model: String,
    /// Labelled subjects handed over, by source.
    pub subjects: usize,
    pub by_source: BTreeMap<String, usize>,
    /// Labels whose subject no longer exists or is not selected.
    pub unresolved: usize,
    /// The training file, by content: `sha256:<hex>`.
    pub training_file: String,
    /// The job the trainer gave; `None` when no model that learns is bound.
    pub result: Option<FineTuningJob>,
    /// The card of the artifact the job made, when it is in the store.
    pub card: Option<Value>,
}

/// The rows of a training run, each with where it came from, and the labels
/// that found no subject.
#[derive(Default)]
pub struct Rows {
    rows: Vec<Row>,
    unresolved: usize,
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
        let unresolved = labels.iter().filter(|l| teaches(l)).count() - rows.len();
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

    /// Adds what another repository gave.
    pub fn extend(&mut self, more: Self) {
        self.rows.extend(more.rows);
        self.unresolved += more.unresolved;
    }

    /// Writes the dataset of the rows under `root` and gives its digest
    /// (`sha256:<hex>`).
    pub fn write(self, root: &Path, decision: &DecisionRef) -> Result<String> {
        dataset::write(root, decision, self.rows)
    }
}

/// Where a training run writes and what it trains.
pub struct Fit<'a> {
    /// The project root the dataset is stored under.
    pub root: &'a Path,
    /// The model trained, `<provider>/<model>`.
    pub reference: Option<&'a str>,
    pub decision: &'a DecisionRef,
    /// The bound model, to give the job to; it must be able to learn.
    pub model: Option<&'a dyn Classifier>,
}

/// Writes the dataset of the rows and asks the model for a job on it.
pub fn train(rows: Rows, fit: &Fit) -> Result<Trained> {
    let mut by_source = BTreeMap::new();
    for row in &rows.rows {
        *by_source.entry(row.source.name().to_owned()).or_insert(0) += 1;
    }
    let (subjects, unresolved) = (rows.rows.len(), rows.unresolved);
    let training_file = rows.write(fit.root, fit.decision)?;
    let reference = fit.reference.unwrap_or_default();
    let result = match fit
        .model
        .filter(|model| model.manifest().capabilities.learns())
    {
        Some(model) => Some(fine_tune(
            model,
            reference,
            &training_file,
            fit.root,
            &fit.decision.id,
        )?),
        None => None,
    };
    let card = result.as_ref().and_then(|job| card_of(fit.root, job));
    Ok(Trained {
        decision: fit.decision.id.clone(),
        model: reference.to_owned(),
        subjects,
        by_source,
        unresolved,
        training_file,
        result,
        card,
    })
}

/// Asks `trainer` for OpenAI's fine-tuning job on the dataset `training_file`:
/// the same dataset for the same base model is the same job (its seed comes
/// from the digest).
pub(crate) fn fine_tune(
    trainer: &dyn Classifier,
    reference: &str,
    training_file: &str,
    root: &Path,
    decision: &str,
) -> Result<FineTuningJob> {
    let model = ModelRef::parse(reference).map_or(reference, |parsed| parsed.model);
    let hex = training_file
        .strip_prefix("sha256:")
        .unwrap_or(training_file);
    let seed = hex
        .get(..8)
        .and_then(|prefix| u32::from_str_radix(prefix, 16).ok())
        .unwrap_or(0);
    let request = CreateFineTuningJobRequest {
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
                root.join(DATASETS_DIR).to_string_lossy().into_owned(),
            ),
            (
                "dir".to_owned(),
                root.join(MODELS_DIR).to_string_lossy().into_owned(),
            ),
        ]),
        options: serde_json::Map::new(),
    };
    Ok(trainer.train(&request)?)
}

/// Whether a label is a row to learn from. A `notApplicable` label teaches
/// nothing to a classifier, and a catalog example is not drawn from the
/// distribution of the labels (every invalid one is a positive): examples
/// already ride in each line's question.
fn teaches(label: &Label) -> bool {
    label.judgment != Judgment::NotApplicable && label.source != Source::Example
}

/// The labels with a subject to learn from, as rows, and the keys they cover.
fn rows_of(labels: &[Label], observed: &BTreeMap<String, Subject>) -> (Vec<Row>, BTreeSet<String>) {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    for label in labels.iter().filter(|l| teaches(l)) {
        let Some(subject) = label.subject.as_ref().or_else(|| observed.get(&label.key)) else {
            continue;
        };
        seen.insert(label.key.clone());
        rows.push(Row {
            subject: subject.clone(),
            fail: label.judgment == Judgment::Fail,
            source: label.source,
            key: label.key.clone(),
            attribution: label.attribution.clone(),
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
) -> Result<Vec<Row>> {
    let attribution = json!({"type": "SoftwareAgent", "hadRole": "model"});
    let mut labelled = Vec::new();
    for (key, subject) in unlabelled {
        let answered = teacher.classify(&request_of(subject.state.clone(), [decision]))?;
        let Some(noul) = answered.answers.get(&decision.id).map(|a| a.noul) else {
            continue;
        };
        let fail = if fails(noul).is_some() {
            true
        } else if fails(1.0 - noul).is_some() {
            false
        } else {
            continue;
        };
        labelled.push(Row {
            subject,
            fail,
            source: Source::Model,
            key,
            attribution: attribution.clone(),
        });
    }
    Ok(labelled)
}

/// The card of the artifact the job made, when it is in the store.
fn card_of(root: &Path, job: &FineTuningJob) -> Option<Value> {
    let document = artifact::read(root, artifact::made(job)?).ok()??;
    document.get("card").cloned()
}
