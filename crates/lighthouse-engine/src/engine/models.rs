//! The models of a run: which model judges a decision, what asking costs, and
//! the answers kept between runs. A rule hands over the subjects it suspects;
//! this puts them to the models of the decision's bindings, most specific
//! first, and an abstention or an unsure answer goes on to the next.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::cache::{Cache, DEFAULT_LIMIT, KeyBuilder};
use lighthouse_model::{Diagnostic, hash};
use lighthouse_plugin::{Ask, Classifier, Judged, Model, Models, Open, Question, Settle, Subject};
use lighthouse_spec::ModelBinding;
use serde::{Deserialize, Serialize};

use super::{batch::Pending, instance::Host};

/// What a run needs to ask models: who is bound, how much may be spent, and
/// where answers are kept.
pub(super) struct RunModels<'a> {
    host: Host<'a>,
    pub(super) binding: &'a ModelBinding,
    /// The model attached to a decision by a revision, by decision id.
    attached: &'a BTreeMap<String, String>,
    /// The paths whose findings the run reports; the whole project when it
    /// asks about everything.
    scopes: Option<&'a [PathBuf]>,
    /// The only subjects models are asked about, by fingerprint.
    subjects: Option<&'a BTreeSet<String>>,
    budget: AtomicUsize,
    answers: Option<Cache>,
    /// Models are not asked: the subjects stay for review.
    off: bool,
    collect: bool,
    seen: Mutex<BTreeMap<String, Subject>>,
    /// The probability of failing each subject was given, by fingerprint: the
    /// answer of the last model that gave one.
    answered: Mutex<BTreeMap<String, f64>>,
    notes: Mutex<BTreeSet<String>>,
    /// Models built so far, by reference; `None` for one that cannot serve.
    built: Mutex<BTreeMap<String, Option<Arc<dyn Classifier + 'a>>>>,
    /// What rules asked and has not been put to the models yet, by group.
    queued: Mutex<BTreeMap<String, Vec<Pending<'a>>>>,
    pub(super) over_budget: AtomicUsize,
    pub(super) calls: AtomicUsize,
    pub(super) cost: Mutex<f64>,
}

/// What is kept of an answer: the probability of failing. The model that gave
/// it is in the key.
#[derive(Serialize, Deserialize)]
struct Kept {
    noul: f64,
}

impl super::Engine {
    /// The model `reference` (`<provider>/<model>`) as a classifier, for the
    /// commands that train and evaluate it.
    pub fn model(&self, reference: &str) -> Result<Box<dyn Classifier + '_>, super::Error> {
        let binding = self.config.models();
        let unknown = || super::Error::UnknownModel(reference.to_owned());
        let (provider, model) = ModelBinding::split(reference).ok_or_else(unknown)?;
        let provider = binding.provider(provider).ok_or_else(unknown)?;
        Ok(self.host().build(reference, model, &provider))
    }

    fn host(&self) -> Host<'_> {
        Host {
            registry: &self.registry,
            trusted: self.trusted,
            commands: self.commands.as_ref(),
        }
    }

    /// The models of a run that reports `scopes`.
    pub(super) fn run_models<'a>(&'a self, scopes: &'a [PathBuf]) -> RunModels<'a> {
        let binding = self.config.models();
        RunModels::new(
            self.host(),
            binding,
            &self.models,
            scopes,
            self.cache.as_ref().map(|c| c.dir.as_path()),
        )
    }
}

impl<'a> RunModels<'a> {
    pub(super) fn new(
        host: Host<'a>,
        binding: &'a ModelBinding,
        options: &'a super::ModelOptions,
        scopes: &'a [PathBuf],
        cache_dir: Option<&std::path::Path>,
    ) -> Self {
        Self {
            host,
            binding,
            attached: &options.attached,
            scopes: (!options.all && options.subjects.is_none()).then_some(scopes),
            subjects: options.subjects.as_ref(),
            budget: AtomicUsize::new(binding.budget),
            answers: cache_dir.and_then(|dir| Cache::open(dir, DEFAULT_LIMIT).ok()),
            off: options.off,
            collect: options.collect,
            seen: Mutex::default(),
            answered: Mutex::default(),
            notes: Mutex::default(),
            built: Mutex::default(),
            queued: Mutex::default(),
            over_budget: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            cost: Mutex::default(),
        }
    }

    /// The subjects the rules put to the models this run, by fingerprint.
    pub(super) fn subjects(&self) -> BTreeMap<String, Subject> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The probabilities of failing the models gave this run, by fingerprint.
    pub(super) fn probabilities(&self) -> BTreeMap<String, f64> {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn record(&self, subject: &Subject, noul: f64) {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(subject.key.clone(), noul);
    }

    /// How many questions were put to models this run, and what they cost.
    pub(super) fn usage(&self) -> (usize, f64) {
        let cost = *self.cost.lock().unwrap_or_else(PoisonError::into_inner);
        (self.calls.load(Ordering::SeqCst), cost)
    }

    /// Writes the answers kept this run, and says what went wrong with the
    /// models: a failure never makes the run incomplete, but it is not silent.
    pub(super) fn finish(&self) -> Vec<String> {
        let mut notes: Vec<String> =
            std::mem::take(&mut *self.notes.lock().unwrap_or_else(PoisonError::into_inner))
                .into_iter()
                .collect();
        let spent = self.over_budget.load(Ordering::SeqCst);
        if spent > 0 {
            notes.push(format!(
                "the model budget ({}) was spent: {spent} subject(s) are left to review",
                self.binding.budget
            ));
        }
        if let Some(kept) = &self.answers
            && let Err(e) = kept.finish()
        {
            notes.push(format!("the model answers were not saved: {e}"));
        }
        notes
    }

    /// The models that judge the rule's decision, most specific binding first:
    /// the model a revision attached, the model the rule names, the project's.
    /// A reference that does not resolve, or whose model cannot classify, is
    /// skipped with a notice: that layer abstains.
    pub(super) fn cascade(&self, rule: &str) -> Vec<Arc<dyn Classifier + 'a>> {
        if self.off {
            return Vec::new();
        }
        let layers = [
            self.attached.get(rule),
            self.binding.rules.get(rule),
            self.binding.classification.as_ref(),
        ];
        let mut found: Vec<Arc<dyn Classifier + 'a>> = Vec::new();
        for reference in layers.into_iter().flatten() {
            let Some(model) = self.model(reference) else {
                continue;
            };
            if found.iter().all(|m| m.manifest().id != *reference) {
                found.push(model);
            }
        }
        found
    }

    /// The model of `reference`, built once per run.
    fn model(&self, reference: &str) -> Option<Arc<dyn Classifier + 'a>> {
        let mut built = self.built.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(model) = built.get(reference) {
            return model.clone();
        }
        let model = self.resolve(reference);
        built.insert(reference.to_owned(), model.clone());
        model
    }

    fn resolve(&self, reference: &str) -> Option<Arc<dyn Classifier + 'a>> {
        let provider = ModelBinding::split(reference)
            .and_then(|(provider, model)| Some((self.binding.provider(provider)?, model)));
        let Some((provider, model)) = provider else {
            self.note(format!(
                "model `{reference}` does not resolve to a declared provider; that binding is skipped"
            ));
            return None;
        };
        let model: Arc<dyn Classifier + 'a> =
            Arc::from(self.host.build(reference, model, &provider));
        if model
            .manifest()
            .capabilities
            .classification
            .is_some_and(|capability| capability.supported)
        {
            Some(model)
        } else {
            self.note(format!(
                "model `{reference}` does not list `classification`; that binding is skipped"
            ));
            None
        }
    }

    pub(super) fn note(&self, note: String) {
        self.notes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(note);
    }

    /// Whether models are asked about a subject: it is one the run is limited
    /// to, else it lies in the files the run reports.
    pub(super) fn reported(&self, subject: &Subject) -> bool {
        if let Some(only) = self.subjects {
            return only.contains(&subject.key);
        }
        self.scopes.is_none_or(|scopes| {
            scopes
                .iter()
                .any(|scope| std::path::Path::new(&subject.file).starts_with(scope))
        })
    }

    /// Takes one unit of the budget for a request to a model that costs.
    pub(super) fn spend(&self) -> bool {
        self.budget
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    fn key(&self, model: &str, version: &str, ask: &Ask, subject: &Subject) -> crate::cache::Key {
        // The check's revision, the model at its version, what the model reads
        // of the subject and what it is asked. The band is not part of it: the
        // cache holds the raw probability.
        let mut key = KeyBuilder::new();
        key.part("model-answer")
            .part(&ask.revision)
            .part(format!("{model}@{version}"))
            .part(digest_of(&subject.state()))
            .part(digest_of(&Question::of(&ask.decision)));
        key.finish()
    }

    pub(super) fn recall(&self, manifest: &Model, ask: &Ask, subject: &Subject) -> Option<f64> {
        let kept: Kept =
            self.answers
                .as_ref()?
                .get(&self.key(&manifest.id, &manifest.version, ask, subject))?;
        Some(kept.noul)
    }

    pub(super) fn keep(&self, manifest: &Model, ask: &Ask, subject: &Subject, noul: f64) {
        let Some(answers) = &self.answers else { return };
        let key = self.key(&manifest.id, &manifest.version, ask, subject);
        answers.put(key, &Kept { noul });
    }
}

impl Models for RunModels<'_> {
    fn collecting(&self) -> bool {
        self.collect
    }

    fn asks(&self, rule: &str) -> bool {
        self.collect || self.scopes.is_none() || !self.cascade(rule).is_empty()
    }

    fn observe(&self, subjects: Vec<Subject>) {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        for subject in subjects {
            seen.insert(subject.key.clone(), subject);
        }
    }

    fn queue(&self, group: &str, ask: Ask, settle: Settle) -> Option<Vec<Diagnostic>> {
        let models = self.cascade(&ask.rule);
        if models.is_empty() {
            let unbound = vec![Judged::Open(Open::Unbound); ask.subjects.len()];
            return Some(settle(unbound));
        }
        self.queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(group.to_owned())
            .or_default()
            .push(Pending {
                ask,
                settle,
                models,
            });
        None
    }

    fn flush(&self, group: &str) -> Vec<(String, Vec<Diagnostic>)> {
        let pending = self
            .queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(group)
            .unwrap_or_default();
        self.run(pending)
    }
}

/// The digest of a value as canonical JSON.
pub(super) fn digest_of<T: Serialize>(value: &T) -> String {
    let value = serde_json::to_value(value).unwrap_or_default();
    hash::sha256(lighthouse_model::canonical(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lighthouse_plugin::{
        Capabilities, CreateFineTuningJobRequest, Error, FineTuningJob, Plugin, PluginManifest,
        Registry, Supports, SystemOneRequest, SystemOneResponse,
    };
    use lighthouse_spec::{Catalog, Config};

    struct Echo(Model);

    impl Classifier for Echo {
        fn manifest(&self) -> &Model {
            &self.0
        }

        fn classify(&self, _: &SystemOneRequest) -> Result<SystemOneResponse, Error> {
            Ok(SystemOneResponse {
                model: "echo".to_owned(),
                answers: BTreeMap::new(),
                usage: serde_json::Value::Null,
            })
        }

        fn train(&self, _: &CreateFineTuningJobRequest) -> Result<FineTuningJob, Error> {
            Err(Error::Failed("no".to_owned()))
        }
    }

    struct Models {
        manifest: PluginManifest,
        model: Model,
    }

    impl Plugin for Models {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }

        fn models(&self) -> Vec<Box<dyn Classifier>> {
            vec![Box::new(Echo(self.model.clone()))]
        }
    }

    #[test]
    fn run_resolves_only_models_that_declare_classification() {
        for supported in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut registry = Registry::default();
            registry
                .register(&Models {
                    manifest: PluginManifest {
                        id: "echo".to_owned(),
                        version: "1".to_owned(),
                    },
                    model: Model {
                        id: "echo".to_owned(),
                        object: "model".to_owned(),
                        created: 0,
                        owned_by: "echo".to_owned(),
                        capabilities: Capabilities {
                            classification: Some(Supports { supported }),
                            ..Capabilities::default()
                        },
                        version: "1".to_owned(),
                        deterministic: true,
                        abstains: false,
                        cost: 0.0,
                    },
                })
                .unwrap();
            let config = Config::parse_inline(
                "plugins = [\"echo\"]\n[models.providers.work]\ntype = \"rpc\"\nplugin = \"echo\"\n",
            )
            .unwrap();
            let engine =
                super::super::Engine::new(registry, config, Catalog::bundled(), dir.path())
                    .unwrap()
                    .with_models(super::super::ModelOptions::default());
            let models = engine.run_models(&[]);

            assert_eq!(models.model("work/echo").is_some(), supported);
            if !supported {
                assert!(
                    models
                        .notes
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|note| { note.contains("does not list `classification`") })
                );
            }
        }
    }
}
