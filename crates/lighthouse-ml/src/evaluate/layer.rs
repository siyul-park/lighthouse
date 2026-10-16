//! Grading the model a revision attaches as the first layer of a decision.
//!
//! The base run is the decision's check without that layer. What it leaves
//! for review, the model is asked about: a subject it fails (P(fail) in the
//! band the check itself uses) becomes a `fail`, every other outcome stays.
//!
//! A model fitted on labels is not graded on them as it is: those labels
//! would score what it memorised. Each label that is a line of the dataset the
//! model learned from is graded by a refit of the model on the lines of the
//! other folds, the folds the trainer itself cross-validates by, so that the
//! answer is one the model never saw the label for.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use lighthouse_plugin::{
    Classifier, DATASETS_DIR, DecisionRef, FineTuningJobStatus, MODELS_DIR, Subject, artifact_path,
    fails, request_of,
};
use lighthouse_protocol::stats::assign;
use serde::Serialize;
use serde_json::Value;

use super::Found;
use crate::{
    Result,
    dataset::{self, Line},
    labels::{Label, Shown},
    metrics::Outcome,
    train::job,
};

/// The layer to grade.
pub struct Layer<'a> {
    /// The model: `<provider>/<model>@sha256:<hex>` for a learned one.
    pub model: &'a str,
    pub decision: &'a DecisionRef,
    /// The project root, where datasets and models are stored.
    pub root: &'a Path,
    /// The classifier of a reference (`<provider>/<model>`).
    pub models: &'a dyn Fn(&str) -> Result<Box<dyn Classifier + 'a>>,
}

/// How a layer was graded, for the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LayerInfo {
    pub model: String,
    /// Labelled subjects the model learned from, graded by refits.
    pub cross_fitted: usize,
    /// The models the refits made.
    pub refits: Vec<String>,
}

/// The labels and the run with a layer's answers laid over them.
pub struct Layered {
    pub found: Found,
    pub labels: Vec<Label>,
}

/// The dataset a learned model was fitted on, and the fold each line is in.
struct CrossFit {
    lines: Vec<Line>,
    folds: Vec<usize>,
    /// `<provider>/<model>`, which trains.
    trainer: String,
    reference: String,
}

impl CrossFit {
    /// The dataset `layer`'s model learned from, when it is a learned one
    /// whose card names a dataset.
    fn of(layer: &Layer) -> Result<Option<Self>> {
        let Some(file) = trained_on(layer.root, layer.model)? else {
            return Ok(None);
        };
        let lines = dataset::read(layer.root, &file)?.ok_or_else(|| {
            format!(
                "the labels `{}` learned from (dataset {file}) are not in {DATASETS_DIR}; train it again",
                layer.model
            )
        })?;
        let trainer = layer.model.split('@').next().unwrap_or(layer.model);
        let groups: Vec<&str> = lines.iter().map(|line| line.group.as_str()).collect();
        let folds = assign(&groups);
        Ok(Some(Self {
            lines,
            folds,
            trainer: trainer.to_owned(),
            reference: layer.model.to_owned(),
        }))
    }

    /// The fold of the line a label is, when the model learned from it.
    fn fold_of(&self, key: &str) -> Option<usize> {
        let at = self.lines.iter().position(|line| line.key == key)?;
        Some(self.folds[at])
    }
}

/// The refits made so far, by the fold they leave out.
struct Refits<'a, 'l> {
    layer: &'l Layer<'a>,
    fit: Option<&'l CrossFit>,
    made: BTreeMap<usize, Box<dyn Classifier + 'a>>,
    /// The references the refits are served by.
    served: BTreeSet<String>,
}

impl<'a, 'l> Refits<'a, 'l> {
    fn new(layer: &'l Layer<'a>, fit: Option<&'l CrossFit>) -> Self {
        Self {
            layer,
            fit,
            made: BTreeMap::new(),
            served: BTreeSet::new(),
        }
    }

    /// The model fitted on every line but those of `fold`.
    fn model(&mut self, fold: usize) -> Result<&dyn Classifier> {
        if !self.made.contains_key(&fold) {
            let made = self.refit(fold)?;
            self.made.insert(fold, made);
        }
        Ok(self.made[&fold].as_ref())
    }

    fn refit(&mut self, fold: usize) -> Result<Box<dyn Classifier + 'a>> {
        let fit = self.fit.ok_or("a refit needs the dataset")?;
        let layer = self.layer;
        let reference = &fit.reference;
        let trainer = (layer.models)(&fit.trainer)?;
        let manifest = trainer.manifest();
        if !manifest.deterministic {
            return Err(format!(
                "cannot grade `{reference}` on the labels it learned from: its trainer is not deterministic; label more and grade on new labels"
            )
            .into());
        }
        let learns = manifest
            .capabilities
            .fine_tuning
            .is_some_and(|capability| capability.supported);
        if !learns {
            return Err(format!("cannot refit `{reference}`: its trainer does not learn").into());
        }
        let others: Vec<String> = fit
            .lines
            .iter()
            .zip(&fit.folds)
            .filter(|(_, at)| **at != fold)
            .map(|(line, _)| line.text.clone())
            .collect();
        if others.is_empty() {
            return Err(format!(
                "cannot grade `{reference}` on the labels it learned from: they are of one group, so nothing is left to refit on"
            )
            .into());
        }
        let written = dataset::write_lines(layer.root, &others)?;
        let done = trainer.train(&job(
            &fit.trainer,
            &written.digest,
            layer.root,
            &layer.decision.id,
        ))?;
        let served = match (&done.status, &done.fine_tuned_model) {
            (FineTuningJobStatus::Succeeded, Some(model)) => model.clone(),
            _ => {
                let why = done
                    .error
                    .map_or_else(String::new, |e| format!(": {}", e.message));
                return Err(format!("the refit of `{reference}` failed{why}").into());
            }
        };
        let provider = fit
            .trainer
            .split_once('/')
            .map_or("", |(provider, _)| provider);
        let served = format!("{provider}/{served}");
        let model = (layer.models)(&served)?;
        self.served.insert(served);
        Ok(model)
    }
}

/// The files the dataset and model stores held before grading: what grading
/// adds to them (the refits) is removed again.
struct Scratch {
    dirs: [PathBuf; 2],
    before: BTreeSet<PathBuf>,
}

impl Scratch {
    fn of(root: &Path) -> Self {
        let dirs = [
            root.join(DATASETS_DIR).join("sha256"),
            root.join(MODELS_DIR).join("sha256"),
        ];
        let before = dirs.iter().flat_map(|dir| files(dir)).collect();
        Self { dirs, before }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        for file in self.dirs.iter().flat_map(|dir| files(dir)) {
            if !self.before.contains(&file) {
                let _ = fs::remove_file(file);
            }
        }
    }
}

impl Found {
    /// A layer's answer about a subject the run left for review.
    fn answered(&mut self, fingerprint: &str, noul: f64, by: &str, fails: bool) {
        let Some(place) = self.place.get(fingerprint).cloned() else {
            return;
        };
        if fails {
            self.by_fingerprint
                .insert(fingerprint.to_owned(), Outcome::Fail);
            self.by_place.insert(place.clone(), Outcome::Fail);
            self.model_decided += 1;
            self.reviews = self.reviews.saturating_sub(1);
            self.probabilities.insert(place, (noul, by.to_owned()));
        } else {
            self.probabilities
                .entry(place)
                .or_insert((noul, by.to_owned()));
        }
    }
}

/// The examples among `labels` as the subjects they show.
pub fn shown(labels: &[Label]) -> Vec<Shown> {
    labels
        .iter()
        .filter_map(|label| {
            Some(Shown {
                key: label.key.clone(),
                truth: label.truth,
                outcome: label.observed?,
                subject: label.subject.clone(),
            })
        })
        .collect()
}

/// What the base run left for review, answered by the layer's model.
pub fn overlay(layer: &Layer, base: &Found, labels: &[Label]) -> Result<Layered> {
    let model = (layer.models)(layer.model)?;
    let name = model.manifest().id.clone();
    let scratch = Scratch::of(layer.root);
    let fit = CrossFit::of(layer)?;
    let mut refits = Refits::new(layer, fit.as_ref());
    let mut found = base.clone();
    let mut labels = labels.to_vec();
    let mut cross_fitted = 0;
    for label in &mut labels {
        if found.outcome_of(label) != Outcome::Review {
            continue;
        }
        let Some(subject) = label
            .subject
            .clone()
            .or_else(|| found.subjects.get(&label.key).cloned())
        else {
            continue;
        };
        let at = fit.as_ref().and_then(|fit| fit.fold_of(&label.key));
        let asked = match at {
            Some(fold) => {
                cross_fitted += 1;
                ask(refits.model(fold)?, layer.decision, &subject)
            }
            None => ask(model.as_ref(), layer.decision, &subject),
        };
        match asked {
            Ok((noul, by, cost)) => {
                found.calls += 1;
                found.cost += cost;
                let fails = fails(noul).is_some();
                if label.observed.is_some() {
                    label.observed = Some(if fails {
                        Outcome::Fail
                    } else {
                        Outcome::Review
                    });
                } else {
                    found.answered(&label.key, noul, &by, fails);
                }
            }
            Err(e) => {
                found.notices.push(format!("{name}: {e}"));
            }
        }
    }
    found.layer = Some(LayerInfo {
        model: layer.model.to_owned(),
        cross_fitted,
        refits: refits.served.into_iter().collect(),
    });
    drop(refits.made);
    drop(scratch);
    Ok(Layered { found, labels })
}

/// The dataset (`sha256:<hex>`) the card of a learned model's artifact names
/// as the file it was trained on; none for a model without an artifact here.
pub(crate) fn trained_on(root: &Path, model: &str) -> Result<Option<String>> {
    let Some((_, digest)) = model.split_once('@') else {
        return Ok(None);
    };
    let Some(hex) = digest.strip_prefix("sha256:") else {
        return Ok(None);
    };
    let path = artifact_path(&root.join(MODELS_DIR), hex);
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(None);
    };
    let artifact: Value = serde_json::from_str(&text)?;
    Ok(artifact
        .pointer("/card/trainingFile")
        .and_then(Value::as_str)
        .map(str::to_owned))
}

/// P(fail) the model gave the subject, the model as evidence names it, and
/// what asking cost.
fn ask(
    model: &dyn Classifier,
    decision: &DecisionRef,
    subject: &Subject,
) -> std::result::Result<(f64, String, f64), String> {
    let manifest = model.manifest();
    let answered = model
        .classify(&request_of(subject.state.clone(), [decision]))
        .map_err(|e| e.to_string())?;
    match answered.answers.get(&decision.id) {
        Some(answer) if answer.kind == "noul" => Ok((
            answer.noul,
            format!("{}@{}", manifest.id, manifest.version),
            manifest.cost,
        )),
        _ => Err(format!("it gave no `noul` answer for `{}`", decision.id)),
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}
