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
    Answer, Ask, Classifier, ClassifyParams, Decided, Judged, ModelManifest, ModelRef, Models,
    Open, Subject, Verdict,
};
use lighthouse_spec::{ModelBinding, ModelScope};
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
}

/// What is kept of an answer: the model that gave it is in the key.
#[derive(Serialize, Deserialize)]
struct Kept {
    verdict: Verdict,
    confidence: f64,
    reason: String,
    version: String,
}

impl super::Engine {
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
            &self.models.attached,
            scopes,
            self.models.scope.unwrap_or(binding.scope),
            self.cache.as_ref().map(|c| c.dir.as_path()),
            self.models.off,
        )
    }
}

impl<'a> RunModels<'a> {
    pub(super) fn new(
        host: Host<'a>,
        binding: &'a ModelBinding,
        attached: &'a BTreeMap<String, String>,
        scopes: &'a [PathBuf],
        scope: ModelScope,
        cache_dir: Option<&std::path::Path>,
        off: bool,
    ) -> Self {
        Self {
            host,
            binding,
            attached,
            scopes: (scope == ModelScope::Changed).then_some(scopes),
            budget: AtomicUsize::new(binding.budget),
            answers: cache_dir.and_then(|dir| Cache::open(dir, DEFAULT_LIMIT).ok()),
            off,
        }
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

    /// The answers of one model to the subjects at `at`, kept ones first.
    fn ask(
        &self,
        model: &dyn Classifier,
        ask: &Ask,
        at: &[usize],
    ) -> Vec<(usize, Result<Answer, Open>, String)> {
        let manifest = model.manifest();
        let mut out = Vec::new();
        let mut missing = Vec::new();
        for &i in at {
            let subject = &ask.params.subjects[i];
            match self.recall(manifest, ask, subject) {
                Some(kept) => out.push((i, Ok(kept.0), kept.1)),
                None => missing.push(i),
            }
        }
        let mut sent = Vec::new();
        for i in missing {
            let subject = &ask.params.subjects[i];
            let allowed = if manifest.cost > 0.0 {
                self.spend(subject)
            } else {
                Ok(())
            };
            match allowed {
                Ok(()) => sent.push(i),
                Err(open) => out.push((i, Err(open), String::new())),
            }
        }
        for batch in sent.chunks(self.binding.batch) {
            out.extend(self.send(model, ask, batch));
        }
        out
    }

    fn send(
        &self,
        model: &dyn Classifier,
        ask: &Ask,
        batch: &[usize],
    ) -> Vec<(usize, Result<Answer, Open>, String)> {
        let params = ClassifyParams {
            decision: ask.params.decision.clone(),
            subjects: batch
                .iter()
                .map(|&i| ask.params.subjects[i].clone())
                .collect(),
            options: ask.params.options.clone(),
        };
        match model.classify(&params) {
            Ok(result) => {
                let version = result.model.version;
                batch
                    .iter()
                    .filter_map(|&i| {
                        let key = &ask.params.subjects[i].key;
                        let answer = result.results.iter().find(|a| &a.key == key)?;
                        self.keep(model.manifest(), &version, ask, i, answer);
                        Some((i, Ok(answer.clone()), version.clone()))
                    })
                    .collect()
            }
            Err(e) => batch
                .iter()
                .map(|&i| (i, Err(Open::Failed(e.to_string())), String::new()))
                .collect(),
        }
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
            .part(hash::sha256(&subject.text))
            .part(hash::sha256(subject.context.facts.to_string()));
        key.finish()
    }

    fn recall(
        &self,
        manifest: &ModelManifest,
        ask: &Ask,
        subject: &Subject,
    ) -> Option<(Answer, String)> {
        let kept: Kept =
            self.answers
                .as_ref()?
                .get(&self.key(&manifest.id, &manifest.version, ask, subject))?;
        let answer = Answer {
            key: subject.key.clone(),
            verdict: kept.verdict,
            confidence: kept.confidence,
            reason: kept.reason,
            span: None,
        };
        Some((answer, kept.version))
    }

    fn keep(&self, manifest: &ModelManifest, version: &str, ask: &Ask, at: usize, answer: &Answer) {
        let Some(answers) = &self.answers else { return };
        let key = self.key(
            &manifest.id,
            &manifest.version,
            ask,
            &ask.params.subjects[at],
        );
        answers.put(
            key,
            &Kept {
                verdict: answer.verdict,
                confidence: answer.confidence,
                reason: answer.reason.clone(),
                version: version.to_owned(),
            },
        );
    }
}

impl Models for RunModels<'_> {
    fn bound(&self, rule: &str) -> bool {
        !self.cascade(rule).is_empty()
    }

    fn classify(&self, ask: &Ask) -> Vec<Judged> {
        let mut judged: Vec<Judged> = ask
            .params
            .subjects
            .iter()
            .map(|_| Judged::Open(Open::Unbound))
            .collect();
        let mut open: Vec<usize> = (0..judged.len()).collect();
        for model in self.cascade(&ask.rule) {
            let model = model.as_ref();
            let id = model.manifest().id.clone();
            let mut still = Vec::new();
            for (i, answer, version) in self.ask(model, ask, &open) {
                match answer {
                    Ok(a) if a.verdict != Verdict::Abstain && a.confidence >= ask.threshold => {
                        judged[i] = Judged::Decided(Decided {
                            verdict: a.verdict,
                            confidence: a.confidence,
                            reason: a.reason,
                            model: ModelRef {
                                id: id.clone(),
                                version,
                            },
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
