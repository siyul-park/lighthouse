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
    sync::Arc,
};

use lighthouse_model::{ModelRef, Result};
use lighthouse_plugin::{
    DATASETS_DIR, DecisionRef, FineTuningJobStatus, MODELS_DIR, ModelClient, Subject, fails,
    request_of,
};
use lighthouse_protocol::{content_dir, stats::assign};

use super::{Found, Layer};
use crate::{
    dataset::{self, Line},
    labels::Sample,
    metrics::Outcome,
    rebuild::{Learned, unrebuildable},
    train::fine_tune,
};

/// The labels and the run with a layer's answers laid over them.
pub struct Overlaid {
    pub found: Found,
    pub labels: Vec<Sample>,
}

impl<'a> Overlay<'a> {
    /// What the base run left for review, answered by the layer's model.
    pub fn apply(&self, base: &Found, labels: &[Sample]) -> Result<Overlaid> {
        let model = (self.models)(self.model)?;
        let name = model.manifest().id.clone();
        let scratch = Scratch::of(self.root);
        let fit = CrossFit::of(self)?;
        let mut refits = Refits::new(self, fit.as_ref());
        let subjects = Arc::clone(&base.subjects);
        let mut found = base.clone();
        let mut labels = labels.to_vec();
        let mut cross_fitted = 0;
        for label in &mut labels {
            if found.outcome_of(label) != Outcome::Review {
                continue;
            }
            let Some(subject) = label.subject.as_ref().or_else(|| subjects.get(&label.key)) else {
                continue;
            };
            let at = fit.as_ref().and_then(|fit| fit.fold_of.get(&label.key));
            let asked = match at {
                Some(&fold) => {
                    cross_fitted += 1;
                    ask(refits.model(fold)?, self.decision, subject)
                }
                None => ask(model.as_ref(), self.decision, subject),
            };
            match asked {
                Ok((noul, by, cost)) => {
                    found.usage.usage.calls += 1;
                    found.usage.usage.cost += cost;
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
        let Refits { made, served, .. } = refits;
        drop(made);
        found.layer = Some(Layer {
            model: self.model.to_owned(),
            cross_fitted,
            refits: served.into_iter().collect(),
            training_file: fit.map(|fit| fit.dataset),
        });
        drop(scratch);
        Ok(Overlaid { found, labels })
    }
}

/// The refits made so far, by the fold they leave out.
struct Refits<'a, 'l> {
    overlay: &'l Overlay<'a>,
    fit: Option<&'l CrossFit<'a>>,
    made: BTreeMap<usize, Box<dyn ModelClient + 'a>>,
    /// The references the refits are served by.
    served: BTreeSet<String>,
}

impl<'a, 'l> Refits<'a, 'l> {
    fn new(overlay: &'l Overlay<'a>, fit: Option<&'l CrossFit<'a>>) -> Self {
        Self {
            overlay,
            fit,
            made: BTreeMap::new(),
            served: BTreeSet::new(),
        }
    }

    /// The model fitted on every line but those of `fold`.
    fn model(&mut self, fold: usize) -> Result<&dyn ModelClient> {
        if !self.made.contains_key(&fold) {
            let made = self.refit(fold)?;
            self.made.insert(fold, made);
        }
        Ok(self.made[&fold].as_ref())
    }

    fn refit(&mut self, fold: usize) -> Result<Box<dyn ModelClient + 'a>> {
        let fit = self.fit.ok_or("a refit needs the dataset")?;
        let overlay = self.overlay;
        let Learned { model, trainer, .. } = fit.learned;
        let fitter = (overlay.models)(trainer)?;
        if let Some(why) = unrebuildable(fitter.as_ref()) {
            return Err(format!(
                "cannot grade `{model}` on the labels it learned from: {why}; label more and grade on new labels"
            )
            .into());
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
                "cannot grade `{model}` on the labels it learned from: they are of one group, so nothing is left to refit on"
            )
            .into());
        }
        let written = dataset::write_lines(overlay.root, &others)?;
        let done = fine_tune(
            fitter.as_ref(),
            trainer,
            &written,
            overlay.root,
            &overlay.decision.id,
        )?;
        let served = match (&done.status, &done.fine_tuned_model) {
            (FineTuningJobStatus::Succeeded, Some(model)) => model.clone(),
            _ => {
                let why = done
                    .error
                    .map_or_else(String::new, |e| format!(": {}", e.message));
                return Err(format!("the refit of `{model}` failed{why}").into());
            }
        };
        let provider = ModelRef::parse(trainer).map_or("", |parsed| parsed.provider);
        let served = format!("{provider}/{served}");
        let refit = (overlay.models)(&served)?;
        self.served.insert(served);
        Ok(refit)
    }
}

/// The dataset a learned model was fitted on, and the fold each line is in.
struct CrossFit<'a> {
    learned: Learned<'a>,
    /// The dataset (`sha256:<hex>`).
    dataset: String,
    lines: Vec<Line>,
    folds: Vec<usize>,
    /// The fold of the first line with a key.
    fold_of: BTreeMap<String, usize>,
}

impl<'a> CrossFit<'a> {
    /// The dataset `overlay`'s model learned from, when it is a learned one
    /// whose card names a dataset.
    fn of(overlay: &Overlay<'a>) -> Result<Option<Self>> {
        let Some(learned) = Learned::parse(overlay.model) else {
            return Ok(None);
        };
        let Some(dataset) = learned.dataset(overlay.root)? else {
            return Ok(None);
        };
        let lines = dataset::read(overlay.root, &dataset)?.ok_or_else(|| {
            format!(
                "the labels `{}` learned from (dataset {dataset}) are not in {DATASETS_DIR}; train it again",
                overlay.model
            )
        })?;
        let groups: Vec<&str> = lines.iter().map(|line| line.group.as_str()).collect();
        let folds = assign(&groups);
        let mut fold_of = BTreeMap::new();
        for (line, fold) in lines.iter().zip(&folds) {
            fold_of.entry(line.key.clone()).or_insert(*fold);
        }
        Ok(Some(Self {
            learned,
            dataset,
            lines,
            folds,
            fold_of,
        }))
    }
}

/// The layer to grade.
pub struct Overlay<'a> {
    /// The model: `<provider>/<model>@sha256:<hex>` for a learned one.
    pub model: &'a str,
    pub decision: &'a DecisionRef,
    /// The root of the store, where datasets and models are kept.
    pub root: &'a Path,
    /// The classifier of a reference (`<provider>/<model>`).
    pub models: &'a dyn Fn(&str) -> Result<Box<dyn ModelClient + 'a>>,
}

/// The files the dataset and model stores held before grading: what grading
/// adds to them (the refits) is removed again. A trained model serves from the
/// store of the project it was initialised with, so a refit cannot be kept
/// in a directory of its own.
struct Scratch {
    dirs: [PathBuf; 2],
    before: BTreeSet<PathBuf>,
}

impl Scratch {
    fn of(root: &Path) -> Self {
        let dirs = [DATASETS_DIR, MODELS_DIR].map(|dir| content_dir(&root.join(dir)));
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

/// P(fail) the model gave the subject, the model as evidence names it, and
/// what asking cost.
fn ask(
    model: &dyn ModelClient,
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
