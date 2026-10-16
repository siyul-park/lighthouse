//! The models of a run: which model judges a decision, what asking costs, and
//! the answers kept between runs. A rule hands over the subjects it suspects;
//! this puts them to the layers of the decision's bindings, most specific
//! first, and an abstention or an unsure answer goes on to the next. A layer
//! is one model or several: a failure moves a request to the next of them.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::cache::{Cache, Digest, KeyBuilder, digest_of};
use lighthouse_model::{Diagnostic, ModelRef};
use lighthouse_plugin::{
    Abstained, Answered, Ask, Classifier, Model, Models, Question, Settle, Subject,
};
use lighthouse_spec::{Models as BoundModels, Routing};

use super::{Scopes, batch::Pending, classifier::Classifiers};

/// Failures of a model, in one run, after which it is no longer asked.
pub(super) const MAX_FAILURES: usize = 3;

/// A binding layer: the models one binding names, tried in turn.
pub(super) struct Layer<'a> {
    /// The references of the models, joined with `,`.
    pub(super) id: String,
    models: Vec<Arc<dyn Classifier + 'a>>,
    /// Requests that went through the layer this run, to rotate its models.
    turn: Arc<AtomicUsize>,
    /// The layer is the model a revision attached, the first one.
    pub(super) attached: bool,
}

impl<'a> Layer<'a> {
    /// The models in the order a request tries them: as written, or rotated
    /// by one more than the request before.
    pub(super) fn order(&self, routing: Routing) -> Vec<&Arc<dyn Classifier + 'a>> {
        let start = match routing {
            Routing::Ordered => 0,
            Routing::RoundRobin => self.turn.fetch_add(1, Ordering::SeqCst) % self.models.len(),
        };
        self.models
            .iter()
            .cycle()
            .skip(start)
            .take(self.models.len())
            .collect()
    }
}

/// What the models of a run were asked and cost.
#[derive(Default)]
struct Usage {
    /// Requests that went through to a model; answers kept are not counted.
    calls: usize,
    cost: f64,
    /// Subjects left to review because the budget was spent.
    over_budget: usize,
}

/// What a run needs to ask models: who is bound, how much may be spent, and
/// where answers are kept.
pub(super) struct RunModels<'a> {
    classifiers: Classifiers<'a>,
    pub(super) binding: &'a BoundModels,
    /// The model attached to a decision by a revision, by decision id.
    attached: &'a BTreeMap<String, String>,
    /// The paths whose findings the run reports; the whole project when it
    /// asks about everything.
    scopes: Option<&'a Scopes>,
    /// The only subjects models are asked about, by fingerprint.
    fingerprints: Option<&'a BTreeSet<String>>,
    budget: AtomicUsize,
    /// Where answers are kept between runs: the cache of the run.
    answers: Option<&'a Cache>,
    /// Models are not asked: the subjects stay for review.
    off: bool,
    collect: bool,
    seen: Mutex<BTreeMap<String, Subject>>,
    /// The probability of failing each subject was given, by fingerprint, and
    /// the model that gave it: the answer of the last one that gave one.
    answered: Mutex<BTreeMap<String, (f64, String)>>,
    /// Failures of each model this run, and the last reason, by reference.
    failures: Mutex<BTreeMap<String, (usize, String)>>,
    /// Requests that went through each layer, by layer id.
    turns: Mutex<BTreeMap<String, Arc<AtomicUsize>>>,
    notes: Mutex<BTreeSet<String>>,
    /// Models built so far, by reference; `None` for one that cannot serve.
    built: Mutex<BTreeMap<String, Option<Arc<dyn Classifier + 'a>>>>,
    /// What rules asked and has not been put to the models yet, by group.
    queued: Mutex<BTreeMap<String, Vec<Pending<'a>>>>,
    usage: Mutex<Usage>,
}

impl super::Engine {
    /// The model `reference` (`<provider>/<model>`) as a classifier, for the
    /// commands that train and evaluate it.
    pub fn model(&self, reference: &str) -> Result<Box<dyn Classifier + '_>, super::Error> {
        let binding = &self.binding;
        let unknown = || super::Error::UnknownModel(reference.to_owned());
        let model = ModelRef::parse(reference).ok_or_else(unknown)?;
        let provider = binding.provider(model.provider).ok_or_else(unknown)?;
        Ok(self.classifiers().build(reference, model.model, &provider))
    }

    fn classifiers(&self) -> Classifiers<'_> {
        Classifiers {
            registry: &self.registry,
            trusted: self.trusted,
            commands: self.commands.as_ref(),
        }
    }
}

impl<'a> RunModels<'a> {
    /// The models of a run of `engine` that reports `scopes`, keeping answers
    /// in `cache`.
    pub(super) fn new(
        engine: &'a super::Engine,
        scopes: &'a Scopes,
        cache: Option<&'a Cache>,
    ) -> Self {
        let options = &engine.models;
        Self {
            classifiers: engine.classifiers(),
            binding: &engine.binding,
            attached: &options.attached,
            scopes: (!options.all && options.fingerprints.is_none()).then_some(scopes),
            fingerprints: options.fingerprints.as_ref(),
            budget: AtomicUsize::new(engine.binding.budget),
            answers: cache,
            off: options.off,
            collect: options.collect,
            seen: Mutex::default(),
            answered: Mutex::default(),
            failures: Mutex::default(),
            turns: Mutex::default(),
            notes: Mutex::default(),
            built: Mutex::default(),
            queued: Mutex::default(),
            usage: Mutex::default(),
        }
    }

    /// The subjects the rules put to the models this run, by fingerprint.
    pub(super) fn subjects(&self) -> BTreeMap<String, Subject> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The probabilities of failing the models gave this run, by fingerprint,
    /// with the model that gave each.
    pub(super) fn probabilities(&self) -> BTreeMap<String, (f64, String)> {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(super) fn record(&self, subject: &Subject, noul: f64, model: &str) {
        self.answered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(subject.key.clone(), (noul, model.to_owned()));
    }

    /// Notes that `manifest`'s model failed, and returns the reason.
    pub(super) fn note_failure(&self, manifest: &Model, reason: String) -> String {
        self.note(format!(
            "model {} failed, its subjects are left to review: {reason}",
            manifest.id
        ));
        reason
    }

    /// Counts a failure of the whole request to a model; at [`MAX_FAILURES`]
    /// the model is skipped for the rest of the run. Returns the reason.
    pub(super) fn failure(&self, manifest: &Model, reason: String) -> String {
        let reason = self.note_failure(manifest, reason);
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = failures.entry(manifest.id.clone()).or_default();
        *count += 1;
        last.clone_from(&reason);
        if *count == MAX_FAILURES {
            self.note(format!(
                "model {} failed {MAX_FAILURES} times; it is skipped for the rest of the run",
                manifest.id
            ));
        }
        reason
    }

    /// Why a model is skipped, when it failed [`MAX_FAILURES`] times.
    pub(super) fn skipped(&self, reference: &str) -> Option<String> {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = failures.get(reference)?;
        (*count >= MAX_FAILURES).then(|| last.clone())
    }

    /// Counts a request that goes to a model.
    pub(super) fn count_call(&self, cost: f64) {
        let mut usage = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
        usage.calls += 1;
        usage.cost += cost;
    }

    /// Counts subjects left to review for want of budget.
    pub(super) fn count_over_budget(&self, subjects: usize) {
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .over_budget += subjects;
    }

    /// How many questions were put to models this run, and what they cost.
    pub(super) fn usage(&self) -> (usize, f64) {
        let usage = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
        (usage.calls, usage.cost)
    }

    /// What went wrong with the models: a failure never makes the run
    /// incomplete, but it is not silent.
    pub(super) fn finish(&self) -> Vec<String> {
        let mut notes: Vec<String> =
            std::mem::take(&mut *self.notes.lock().unwrap_or_else(PoisonError::into_inner))
                .into_iter()
                .collect();
        let spent = self
            .usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .over_budget;
        if spent > 0 {
            notes.push(format!(
                "the model budget ({}) was spent: {spent} subject(s) are left to review",
                self.binding.budget
            ));
        }
        notes
    }

    /// The layers that judge the rule's decision, most specific binding first:
    /// the model a revision attached, the models the rule names, the
    /// project's. A reference that does not resolve, or whose model cannot
    /// classify, is skipped with a notice; a layer left with none abstains.
    pub(super) fn layers(&self, rule: &str) -> Vec<Layer<'a>> {
        if self.off {
            return Vec::new();
        }
        let attached = self.attached.get(rule).map(|model| vec![model.clone()]);
        let bound = [
            attached,
            self.binding.route(rule).map(<[String]>::to_vec),
            Some(self.binding.classification.clone()),
        ];
        let mut found: Vec<Layer<'a>> = Vec::new();
        for (at, references) in bound.into_iter().enumerate() {
            let Some(references) = references else {
                continue;
            };
            let models: Vec<_> = references.iter().filter_map(|r| self.model(r)).collect();
            if models.is_empty() {
                continue;
            }
            let ids: Vec<&str> = models.iter().map(|m| m.manifest().id.as_str()).collect();
            let id = ids.join(",");
            if found.iter().all(|layer| layer.id != id) {
                let turn = self.turn(&id);
                let attached = at == 0;
                found.push(Layer {
                    id,
                    models,
                    turn,
                    attached,
                });
            }
        }
        found
    }

    /// The counter of the layer `id`, shared by every binding that names it.
    fn turn(&self, id: &str) -> Arc<AtomicUsize> {
        let mut turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(turns.entry(id.to_owned()).or_default())
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
        let provider = ModelRef::parse(reference)
            .and_then(|model| Some((self.binding.provider(model.provider)?, model.model)));
        let Some((provider, model)) = provider else {
            self.note(format!(
                "model `{reference}` does not resolve to a declared provider; that binding is skipped"
            ));
            return None;
        };
        let model: Arc<dyn Classifier + 'a> =
            Arc::from(self.classifiers.build(reference, model, &provider));
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
        if let Some(only) = self.fingerprints {
            return only.contains(&subject.key);
        }
        self.scopes
            .is_none_or(|scopes| scopes.contains(Path::new(&subject.state.file)))
    }

    /// Takes one unit of the budget for a request to a model that costs.
    pub(super) fn spend(&self) -> bool {
        self.budget
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    /// The probability of failing a model gave the `i`th subject of `item`
    /// in an earlier run.
    pub(super) fn recall(&self, model: &Model, item: &Pending<'a>, i: usize) -> Option<f64> {
        self.answers?.get(&Self::key(model, item, i))
    }

    pub(super) fn keep(&self, model: &Model, item: &Pending<'a>, i: usize, noul: f64) {
        if let Some(answers) = self.answers {
            answers.put(Self::key(model, item, i), &noul);
        }
    }

    /// The key of an answer: the check's revision, the model at its version,
    /// what the model reads of the subject and what it is asked. The band is
    /// not part of it: the cache holds the raw probability.
    fn key(model: &Model, item: &Pending<'a>, i: usize) -> Digest {
        let mut key = KeyBuilder::new();
        key.part("model-answer")
            .part(&item.ask.revision)
            .part(named(model))
            .part(item.states[i])
            .part(item.question);
        key.finish()
    }
}

impl Models for RunModels<'_> {
    fn collecting(&self) -> bool {
        self.collect
    }

    fn asks(&self, rule: &str) -> bool {
        self.collect || self.scopes.is_none() || !self.layers(rule).is_empty()
    }

    fn observe(&self, subjects: Vec<Subject>) {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        for subject in subjects {
            seen.insert(subject.key.clone(), subject);
        }
    }

    fn queue(&self, group: &str, ask: Ask, settle: Settle) -> Option<Vec<Diagnostic>> {
        let layers = self.layers(&ask.rule);
        if layers.is_empty() {
            let unbound = vec![Answered::Abstained(Abstained::Unbound); ask.subjects.len()];
            return Some(settle(unbound));
        }
        let states = ask
            .subjects
            .iter()
            .map(|subject| digest_of(&subject.state).unwrap_or_default())
            .collect();
        let question = digest_of(&Question::new(&ask.decision)).unwrap_or_default();
        self.queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(group.to_owned())
            .or_default()
            .push(Pending {
                ask,
                settle,
                layers,
                states,
                question,
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

/// The model as evidence names it: `<provider>/<model>@<version>`.
pub(super) fn named(model: &Model) -> String {
    format!("{}@{}", model.id, model.version)
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
            let scopes = Scopes::default();
            let models = RunModels::new(&engine, &scopes, None);

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
