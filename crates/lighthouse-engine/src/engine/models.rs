//! The models of a run: which model judges a decision, what asking costs, and
//! the answers kept between runs. A rule hands over the subjects it suspects;
//! this puts them to the models of the decision's bindings, most specific
//! first, and an abstention or an unsure answer goes on to the next.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

use lighthouse_cache::{Cache, DEFAULT_LIMIT, KeyBuilder};
use lighthouse_model::hash;
use lighthouse_plugin::{
    Ask, Classifier, ClassifyParams, Decided, Judged, ModelManifest, Models, Open, Question,
    ResultKind, Subject, decide,
};
use lighthouse_spec::ModelBinding;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::instance::Host;

/// What a run needs to ask models: who is bound, how much may be spent, and
/// where answers are kept.
pub(super) struct RunModels<'a> {
    host: Host<'a>,
    binding: &'a ModelBinding,
    /// The model attached to a decision by a revision, by decision id.
    attached: &'a BTreeMap<String, String>,
    /// The paths whose findings the run reports; the whole project when it
    /// asks about everything.
    scopes: Option<&'a [PathBuf]>,
    budget: AtomicUsize,
    answers: Option<Cache>,
    /// Models are not asked: the subjects stay for review.
    off: bool,
    collect: bool,
    seen: std::sync::Mutex<BTreeMap<String, Subject>>,
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
        let host = Host {
            registry: &self.registry,
            root: self.ws.root.clone(),
            trusted: self.trusted,
        };
        Ok(host.build(reference, model, &provider))
    }

    /// The models of a run that reports `scopes`.
    pub(super) fn run_models<'a>(&'a self, scopes: &'a [PathBuf]) -> RunModels<'a> {
        let binding = self.config.models();
        RunModels::new(
            Host {
                registry: &self.registry,
                root: self.ws.root.clone(),
                trusted: self.trusted,
            },
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
            scopes: (!options.all).then_some(scopes),
            budget: AtomicUsize::new(binding.budget),
            answers: cache_dir.and_then(|dir| Cache::open(dir, DEFAULT_LIMIT).ok()),
            off: options.off,
            collect: options.collect,
            seen: std::sync::Mutex::default(),
        }
    }

    /// The subjects the rules put to the models this run, by fingerprint.
    pub(super) fn subjects(&self) -> BTreeMap<String, Subject> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Writes the answers kept this run.
    pub(super) fn finish(&self) -> Option<String> {
        let kept = self.answers.as_ref()?;
        kept.finish()
            .err()
            .map(|e| format!("the model answers were not saved: {e}"))
    }

    /// The models that judge the rule's decision, most specific binding first:
    /// the model a revision attached, the model the rule names, the project's.
    fn cascade(&self, rule: &str) -> Vec<Box<dyn Classifier + 'a>> {
        if self.off {
            return Vec::new();
        }
        let layers = [
            self.attached.get(rule),
            self.binding.rules.get(rule),
            self.binding.classification.as_ref(),
        ];
        let mut found: Vec<Box<dyn Classifier + 'a>> = Vec::new();
        for reference in layers.into_iter().flatten() {
            let Some((provider, model)) = ModelBinding::split(reference) else {
                continue;
            };
            let Some(provider) = self.binding.provider(provider) else {
                continue;
            };
            if found.iter().any(|m| m.manifest().id == *reference) {
                continue;
            }
            let model = self.host.build(reference, model, &provider);
            if model.manifest().tasks.iter().any(|t| t == "classification") {
                found.push(model);
            }
        }
        found
    }

    /// What one model says about the subjects at `at`: P(fail), or why there
    /// is none. Kept answers first; the rest asked one subject at a time,
    /// concurrently.
    fn ask(
        &self,
        model: &dyn Classifier,
        ask: &Ask,
        at: &[usize],
    ) -> Vec<(usize, Result<f64, Open>)> {
        let manifest = model.manifest();
        let mut out = Vec::new();
        let mut sent = Vec::new();
        for &i in at {
            let subject = &ask.subjects[i];
            if let Some(noul) = self.recall(manifest, ask, subject) {
                out.push((i, Ok(noul)));
                continue;
            }
            let allowed = if manifest.cost > 0.0 {
                self.spend(subject)
            } else {
                Ok(())
            };
            match allowed {
                Ok(()) => sent.push(i),
                Err(open) => out.push((i, Err(open))),
            }
        }
        out.extend(
            sent.par_iter()
                .map(|&i| (i, self.send(model, ask, i)))
                .collect::<Vec<_>>(),
        );
        out
    }

    /// One subject, one question: the decision.
    fn send(&self, model: &dyn Classifier, ask: &Ask, at: usize) -> Result<f64, Open> {
        let subject = &ask.subjects[at];
        let params = ClassifyParams {
            state: subject.state(),
            model: String::new(),
            questions: BTreeMap::from([(ask.decision.id.clone(), Question::of(&ask.decision))]),
            options: serde_json::Map::new(),
        };
        let result = model
            .classify(&params)
            .map_err(|e| Open::Failed(e.to_string()))?;
        let noul = result
            .answers
            .get(&ask.decision.id)
            .map(|a| a.noul)
            .ok_or(Open::Undecided)?;
        self.keep(model.manifest(), ask, subject, noul);
        Ok(noul)
    }

    /// Whether a subject is asked of a model that costs: it lies in the files
    /// the run reports, and the budget has some left.
    fn spend(&self, subject: &Subject) -> Result<(), Open> {
        let reported = self.scopes.is_none_or(|scopes| {
            scopes
                .iter()
                .any(|scope| std::path::Path::new(&subject.file).starts_with(scope))
        });
        if !reported {
            return Err(Open::Undecided);
        }
        self.budget
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .map(drop)
            .map_err(|_| Open::OverBudget)
    }

    fn key(
        &self,
        model: &str,
        version: &str,
        ask: &Ask,
        subject: &Subject,
    ) -> lighthouse_cache::Key {
        let mut key = KeyBuilder::new();
        key.part("model-answer")
            .part(model)
            .part(version)
            .part(&ask.revision)
            .part(&ask.decision.id)
            .part(hash::sha256(&subject.text))
            .part(hash::sha256(subject.context.facts.to_string()));
        key.finish()
    }

    fn recall(&self, manifest: &ModelManifest, ask: &Ask, subject: &Subject) -> Option<f64> {
        let kept: Kept =
            self.answers
                .as_ref()?
                .get(&self.key(&manifest.id, &manifest.version, ask, subject))?;
        Some(kept.noul)
    }

    fn keep(&self, manifest: &ModelManifest, ask: &Ask, subject: &Subject, noul: f64) {
        let Some(answers) = &self.answers else { return };
        let key = self.key(&manifest.id, &manifest.version, ask, subject);
        answers.put(key, &Kept { noul });
    }
}

impl Models for RunModels<'_> {
    fn collecting(&self) -> bool {
        self.collect
    }

    fn observe(&self, subjects: Vec<Subject>) {
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for subject in subjects {
            seen.insert(subject.key.clone(), subject);
        }
    }

    fn bound(&self, rule: &str) -> bool {
        !self.cascade(rule).is_empty()
    }

    fn classify(&self, ask: &Ask) -> Vec<Judged> {
        let mut judged: Vec<Judged> = ask
            .subjects
            .iter()
            .map(|_| Judged::Open(Open::Unbound))
            .collect();
        let mut open: Vec<usize> = (0..judged.len()).collect();
        for model in self.cascade(&ask.rule) {
            let model = model.as_ref();
            let manifest = model.manifest();
            let mut still = Vec::new();
            for (i, answer) in self.ask(model, ask, &open) {
                match answer.map(|noul| decide(noul, ask.upper)) {
                    Ok((ResultKind::Fail, confidence)) => {
                        judged[i] = Judged::Decided(Decided {
                            kind: ResultKind::Fail,
                            confidence,
                            model: manifest.id.clone(),
                            version: manifest.version.clone(),
                        });
                    }
                    Ok(_) => {
                        judged[i] = Judged::Open(Open::Undecided);
                        still.push(i);
                    }
                    Err(open) => {
                        judged[i] = Judged::Open(open);
                        still.push(i);
                    }
                }
            }
            still.sort_unstable();
            open = still;
        }
        judged
    }
}
