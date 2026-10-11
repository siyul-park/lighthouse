//! `lighthouse learn train`: collects the labelled subjects of a decision the
//! way evaluation does, builds them exactly as a model is asked about them,
//! and hands them to a model that learns from labels.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_plugin::{
    Classifier, ClassifyParams, LabelSource, ResultKind, Subject, TrainParams, TrainResponse,
    TrainSubject,
};
use serde::Serialize;

use crate::{
    CheckRequest, ModelFlags, Result, Session, check,
    labels::{Baseline, Label, WEIGHT_EXAMPLE, WEIGHT_MODEL, collect},
};

/// What to train.
pub struct TrainRequest {
    pub decision: String,
    /// The model to train (`<provider>/<model>`); else the decision's bound
    /// model.
    pub model: Option<String>,
    /// A model whose answers on unlabelled subjects are labels too, at the
    /// weight of a model.
    pub from_model: Option<String>,
    pub baseline: Option<PathBuf>,
    /// The `repo` of the baseline rows to use.
    pub repo: Option<String>,
    pub config: Option<PathBuf>,
}

/// What a training run did.
#[derive(Debug, Serialize)]
pub struct Trained {
    pub decision: String,
    pub model: String,
    /// Labelled subjects handed over, by source.
    pub subjects: usize,
    pub by_source: std::collections::BTreeMap<String, usize>,
    /// Labels whose subject no longer exists or is not selected.
    pub unresolved: usize,
    pub result: TrainResponse,
}

/// Trains the model on the labels of the decision.
pub fn train(request: &TrainRequest) -> Result<Trained> {
    let session = Session::load(request.config.as_deref())?;
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
        .or_else(|| binding.classification.clone())
        .ok_or("no model to train: pass --model or bind one")?;
    let table = request.baseline.as_deref().map(|file| Baseline {
        file,
        repo: request.repo.as_deref(),
    });
    let labels = collect(&root, &decision, table.as_ref(), Some(&reference))?;
    let observed = &checked.outcome.subjects;
    let decision_ref = lighthouse_checks::decision_ref(&decision);
    let (mut subjects, seen) = labelled(&labels, observed);
    let unresolved = labels.len() - subjects.len();
    subjects.extend(examples(&decision_ref));
    if let Some(teacher) = &request.from_model {
        let unlabelled = observed
            .iter()
            .filter(|(fingerprint, _)| !seen.contains(*fingerprint))
            .map(|(_, subject)| subject.clone())
            .collect();
        let teacher = checked.model(teacher)?;
        subjects.extend(taught(teacher.as_ref(), &decision_ref, unlabelled)?);
    }
    let mut by_source = std::collections::BTreeMap::new();
    for s in &subjects {
        *by_source
            .entry(format!("{:?}", s.source).to_lowercase())
            .or_insert(0) += 1;
    }
    let dir = artifact_dir(&root, &reference, &request.decision);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // The same labelled set for the same decision is the same request.
    let request_id =
        lighthouse_model::hash::short(&serde_json::to_string(&(&decision_ref.id, &subjects))?, 8);
    let count = subjects.len();
    let result = checked.model(&reference)?.train(&TrainParams {
        request_id: Some(request_id),
        decision: decision_ref,
        subjects,
        options: serde_json::Map::new(),
        dir: dir.to_string_lossy().into_owned(),
    })?;
    Ok(Trained {
        decision: request.decision.clone(),
        model: reference,
        subjects: count,
        by_source,
        unresolved,
        result,
    })
}

/// The labels whose subject the run saw, as training subjects, and the
/// fingerprints they cover.
fn labelled(
    labels: &[Label],
    observed: &std::collections::BTreeMap<String, Subject>,
) -> (Vec<TrainSubject>, BTreeSet<String>) {
    let mut subjects = Vec::new();
    let mut seen = BTreeSet::new();
    for label in labels {
        if let Some(subject) = observed.get(&label.fingerprint) {
            seen.insert(label.fingerprint.clone());
            subjects.push(TrainSubject {
                subject: subject.clone(),
                label: label.kind,
                weight: label.weight,
                source: label.source,
            });
        }
    }
    (subjects, seen)
}

/// The decision's examples as training subjects.
fn examples(decision: &lighthouse_plugin::DecisionRef) -> Vec<TrainSubject> {
    decision
        .examples
        .iter()
        .map(|shot| TrainSubject {
            subject: Subject {
                key: format!("example:{}", shot.note),
                kind: "example".to_owned(),
                id: shot.note.clone(),
                language: shot.language.clone(),
                file: String::new(),
                text: shot.code.clone(),
                context: lighthouse_plugin::SubjectContext::default(),
            },
            label: shot.kind,
            weight: WEIGHT_EXAMPLE,
            source: LabelSource::Example,
        })
        .collect()
}

/// What a teacher model decides about subjects nobody labelled.
fn taught(
    teacher: &dyn Classifier,
    decision: &lighthouse_plugin::DecisionRef,
    unlabelled: Vec<Subject>,
) -> Result<Vec<TrainSubject>> {
    let answers = teacher.classify(&ClassifyParams {
        decision: decision.clone(),
        subjects: unlabelled.clone(),
        options: serde_json::Map::new(),
    })?;
    Ok(answers
        .results
        .into_iter()
        .filter(|answer| answer.kind != ResultKind::Review)
        .filter_map(|answer| {
            let subject = unlabelled.iter().find(|s| s.key == answer.key)?;
            Some(TrainSubject {
                subject: subject.clone(),
                label: answer.kind,
                weight: WEIGHT_MODEL,
                source: LabelSource::Model,
            })
        })
        .collect())
}

/// Where a model keeps what it learned for a decision:
/// `.lighthouse/models/<provider>/<decision>/`.
fn artifact_dir(root: &Path, reference: &str, decision: &str) -> PathBuf {
    let provider = reference.split('/').next().unwrap_or(reference);
    root.join(".lighthouse/models")
        .join(provider)
        .join(decision.replace('/', "-"))
}
