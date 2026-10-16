//! The models of a run: which model judges a decision, what asking costs, and
//! the answers kept between runs. A rule hands over the subjects it suspects;
//! this puts them to the routes of the decision's bindings, most specific
//! first, and an abstention or an unsure answer goes on to the next. A route
//! is one model or several: a failure moves a request to the next of them.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::cache::{Cache, DEFAULT_LIMIT, KeyBuilder};
use lighthouse_model::{Diagnostic, ModelRef, hash};
use lighthouse_plugin::{
    Abstained, Answered, Ask, Classifier, Model, Models, Question, Settle, Subject,
};
use lighthouse_spec::{Models as BoundModels, Routing};
use serde::{Deserialize, Serialize};

use super::{batch::Pending, instance::Host};

/// Failures of a model, in one run, after which it is no longer asked.
pub(super) const ALLOWED_FAILS: usize = 3;

/// What a run without attached models is given as the attached ones.
static NONE: BTreeMap<String, String> = BTreeMap::new();

/// A layer of the cascade: the models one binding names, tried in turn.
pub(super) struct Route<'a> {
    /// The references of the models, joined with `,`.
    pub(super) id: String,
    models: Vec<Arc<dyn Classifier + 'a>>,
    /// Requests that went through the route this run, to rotate its models.
    turn: Arc<AtomicUsize>,
    /// The route is the model a revision attached, the first layer.
    pub(super) attached: bool,
}

impl<'a> Route<'a> {
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

/// What a run needs to ask models: who is bound, how much may be spent, and
/// where answers are kept.
pub(super) struct RunModels<'a> {
    host: Host<'a>,
    pub(super) binding: &'a BoundModels,
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
    /// The probability of failing each subject was given, by fingerprint, and
    /// the model that gave it: the answer of the last one that gave one.
    answered: Mutex<BTreeMap<String, (f64, String)>>,
    /// Failures of each model this run, and the last reason, by reference.
    fails: Mutex<BTreeMap<String, (usize, String)>>,
    /// Requests that went through each route, by route id.
    turns: Mutex<BTreeMap<String, Arc<AtomicUsize>>>,
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
        let binding = &self.binding;
        let unknown = || super::Error::UnknownModel(reference.to_owned());
        let model = ModelRef::parse(reference).ok_or_else(unknown)?;
        let provider = binding.provider(model.provider).ok_or_else(unknown)?;
        Ok(self.host().build(reference, model.model, &provider))
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
        let binding = &self.binding;
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
        binding: &'a BoundModels,
        options: &'a super::ModelOptions,
        scopes: &'a [PathBuf],
        cache_dir: Option<&std::path::Path>,
    ) -> Self {
        Self {
            host,
            binding,
            attached: if options.detached {
                &NONE
            } else {
                &options.attached
            },
            scopes: (!options.all && options.subjects.is_none()).then_some(scopes),
            subjects: options.subjects.as_ref(),
            budget: AtomicUsize::new(binding.budget),
            answers: cache_dir.and_then(|dir| Cache::open(dir, DEFAULT_LIMIT).ok()),
            off: options.off,
            collect: options.collect,
            seen: Mutex::default(),
            answered: Mutex::default(),
            fails: Mutex::default(),
            turns: Mutex::default(),
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
    pub(super) fn report(&self, manifest: &Model, reason: String) -> String {
        self.note(format!(
            "model {} failed, its subjects are left to review: {reason}",
            manifest.id
        ));
        reason
    }

    /// Counts a failure of the whole request to a model; at [`ALLOWED_FAILS`]
    /// the model is skipped for the rest of the run. Returns the reason.
    pub(super) fn fail(&self, manifest: &Model, reason: String) -> String {
        let reason = self.report(manifest, reason);
        let mut fails = self.fails.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = fails.entry(manifest.id.clone()).or_default();
        *count += 1;
        last.clone_from(&reason);
        if *count == ALLOWED_FAILS {
            self.note(format!(
                "model {} failed {ALLOWED_FAILS} times; it is skipped for the rest of the run",
                manifest.id
            ));
        }
        reason
    }

    /// Why a model is skipped, when it failed [`ALLOWED_FAILS`] times.
    pub(super) fn cooled(&self, reference: &str) -> Option<String> {
        let fails = self.fails.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = fails.get(reference)?;
        (*count >= ALLOWED_FAILS).then(|| last.clone())
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

    /// The routes that judge the rule's decision, most specific binding first:
    /// the model a revision attached, the models the rule names, the
    /// project's. A reference that does not resolve, or whose model cannot
    /// classify, is skipped with a notice; a route left with none abstains.
    pub(super) fn cascade(&self, rule: &str) -> Vec<Route<'a>> {
        if self.off {
            return Vec::new();
        }
        let attached = self.attached.get(rule).map(|model| vec![model.clone()]);
        let layers = [
            attached,
            self.binding.route(rule).map(<[String]>::to_vec),
            Some(self.binding.classification.clone()),
        ];
        let mut found: Vec<Route<'a>> = Vec::new();
        for (at, references) in layers.into_iter().enumerate() {
            let Some(references) = references else {
                continue;
            };
            let models: Vec<_> = references.iter().filter_map(|r| self.model(r)).collect();
            if models.is_empty() {
                continue;
            }
            let ids: Vec<&str> = models.iter().map(|m| m.manifest().id.as_str()).collect();
            let id = ids.join(",");
            if found.iter().all(|route| route.id != id) {
                let turn = self.turn(&id);
                let attached = at == 0;
                found.push(Route {
                    id,
                    models,
                    turn,
                    attached,
                });
            }
        }
        found
    }

    /// The counter of the route `id`, shared by every layer that names it.
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
                .any(|scope| std::path::Path::new(&subject.state.file).starts_with(scope))
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

    fn key(&self, model: &str, version: &str, ask: &Ask, subject: &Subject) -> crate::cache::Key {
        // The check's revision, the model at its version, what the model reads
        // of the subject and what it is asked. The band is not part of it: the
        // cache holds the raw probability.
        let mut key = KeyBuilder::new();
        key.part("model-answer")
            .part(&ask.revision)
            .part(format!("{model}@{version}"))
            .part(digest_of(&subject.state))
            .part(digest_of(&Question::new(&ask.decision)));
        key.finish()
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
        let routes = self.cascade(&ask.rule);
        if routes.is_empty() {
            let unbound = vec![Answered::Abstained(Abstained::Unbound); ask.subjects.len()];
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
                routes,
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
    hash::digest(&value)
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
