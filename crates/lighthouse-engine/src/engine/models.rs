//! The models of a run: which model judges a decision, what asking costs, and
//! the answers kept between runs. A rule hands over the subjects it suspects;
//! this puts them to the layers of the decision's bindings, most specific
//! first, and an abstention or an unsure answer goes on to the next. A layer
//! is one model or several: a failure moves a request to the next of them.
//!
//! Putting what a file's rules queued to the models in as few requests as
//! possible: one request per subject and route, holding every decision that
//! resolves to that route on it, at most [`MAX_QUESTIONS`] questions long
//! (the overflow goes in the next request). Requests run concurrently. A
//! request moves to the next model of its route when a model fails.

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
    Abstained, Answered, Ask, Decided, Explored, Model, ModelClient, Models, Question, Settle,
    Subject, fails, request_of,
};
use lighthouse_protocol::hash::sha256;
use lighthouse_spec::{Models as BoundModels, Routing};
use rayon::prelude::*;

use super::{MAX_FAILURES, Scopes, client::Clients, rotation_start};

/// The most questions put to a model in one request.
pub(super) const MAX_QUESTIONS: usize = 32;

/// The values the first eight hex digits of a digest take, which makes a
/// subject's place in the sample a share between 0 and 1.
const SAMPLE_SPAN: f64 = 4_294_967_296.0;

/// What a run needs to ask models: who is bound, how much may be spent, and
/// where answers are kept.
pub(super) struct RunModels<'a> {
    clients: Clients<'a>,
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
    failures: Failures,
    /// Requests that went through each layer, by layer id.
    turns: Mutex<BTreeMap<String, Arc<AtomicUsize>>>,
    notes: Mutex<BTreeSet<String>>,
    /// Models built so far, by reference; `None` for one that cannot serve.
    built: Mutex<BTreeMap<String, Option<Arc<dyn ModelClient + 'a>>>>,
    /// What rules asked and has not been put to the models yet, by group.
    queued: Mutex<BTreeMap<String, Vec<Pending<'a>>>>,
    usage: Mutex<Usage>,
}

/// What a rule asked and has not had answered: its binding layers, most
/// specific first, what to make of the judgments, and the digests of what the
/// models are told, which key their answers.
pub(super) struct Pending<'a> {
    pub(super) ask: Ask,
    pub(super) settle: Settle,
    pub(super) layers: Vec<Layer<'a>>,
    /// The state of each subject of the ask.
    pub(super) states: Vec<Digest>,
    /// The question the ask puts.
    pub(super) question: Digest,
}

/// A binding layer: the models one binding names, tried in turn.
pub(super) struct Layer<'a> {
    /// The references of the models, joined with `,`.
    pub(super) id: String,
    models: Vec<Arc<dyn ModelClient + 'a>>,
    /// Requests that went through the layer this run, to rotate its models.
    turn: Arc<AtomicUsize>,
    /// The layer is the model a revision attached, the first one.
    pub(super) attached: bool,
}

/// What the models of a run were asked and cost.
#[derive(Default)]
struct Usage {
    /// Requests that went through to a model, and what they cost.
    usage: lighthouse_model::Usage,
    /// Subjects left to review because the budget was spent.
    over_budget: usize,
}

/// Failures shared by every binding of the same model in one run.
#[derive(Default)]
pub(super) struct Failures(Mutex<BTreeMap<String, (usize, String)>>);

/// The `i`th subject of the `p`th ask.
type At = (usize, usize);

/// P(fail) and the model that gave it as evidence names it, or why the
/// subject stays open.
type Answer = Result<(f64, String), Abstained>;

/// Why a model did not answer a request.
enum Miss {
    /// The budget refused it.
    Budget,
    /// It failed, or has failed too often to be asked.
    Failed(String),
}

impl<'a> Layer<'a> {
    /// The models in the order a request tries them: as written, or rotated
    /// by one more than the request before.
    pub(super) fn order(&self, routing: Routing) -> Vec<&Arc<dyn ModelClient + 'a>> {
        let start = rotation_start(routing, self.models.len(), &self.turn);
        self.models
            .iter()
            .cycle()
            .skip(start)
            .take(self.models.len())
            .collect()
    }

    /// The configured model order, for looking up cached answers without
    /// consuming a request turn.
    pub(super) fn models(&self) -> &[Arc<dyn ModelClient + 'a>] {
        &self.models
    }
}

impl Failures {
    /// Records a failed request and whether this is the threshold failure.
    pub(super) fn record(&self, model: &str, reason: &str) -> bool {
        let mut failures = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = failures.entry(model.to_owned()).or_default();
        *count += 1;
        last.clear();
        last.push_str(reason);
        *count == MAX_FAILURES
    }

    /// The last failure once a model has reached the per-run limit.
    pub(super) fn skipped(&self, model: &str) -> Option<String> {
        let failures = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = failures.get(model)?;
        (*count >= MAX_FAILURES).then(|| last.clone())
    }
}

impl super::Engine {
    /// The model `reference` (`<provider>/<model>`) as a client, for the
    /// commands that train and evaluate it.
    pub fn model(&self, reference: &str) -> Result<Box<dyn ModelClient + '_>, super::Error> {
        let binding = &self.binding;
        let unknown = || super::Error::UnknownModel(reference.to_owned());
        let model = ModelRef::parse(reference).ok_or_else(unknown)?;
        let provider = binding.provider(model.provider).ok_or_else(unknown)?;
        Ok(self.clients().build(reference, model.model, &provider))
    }

    pub(super) fn clients(&self) -> Clients<'_> {
        Clients {
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
            clients: engine.clients(),
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
            failures: Failures::default(),
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
        self.note(format!("model {} failed: {reason}", manifest.id));
        reason
    }

    /// Counts a failure of the whole request to a model; at [`MAX_FAILURES`]
    /// the model is skipped for the rest of the run. Returns the reason.
    pub(super) fn failure(&self, manifest: &Model, reason: String) -> String {
        let reason = self.note_failure(manifest, reason);
        if self.failures.record(&manifest.id, &reason) {
            self.note(format!(
                "model {} failed {MAX_FAILURES} times; it is skipped for the rest of the run",
                manifest.id
            ));
        }
        reason
    }

    /// Why a model is skipped, when it failed [`MAX_FAILURES`] times.
    pub(super) fn skipped(&self, reference: &str) -> Option<String> {
        self.failures.skipped(reference)
    }

    /// Counts a request that goes to a model.
    pub(super) fn count_call(&self, cost: f64) {
        let mut usage = self.usage.lock().unwrap_or_else(PoisonError::into_inner);
        usage.usage.calls += 1;
        usage.usage.cost += cost;
    }

    /// Counts subjects left to review for want of budget.
    pub(super) fn count_over_budget(&self, subjects: usize) {
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .over_budget += subjects;
    }

    /// How many questions were put to models this run, and what they cost.
    pub(super) fn usage(&self) -> lighthouse_model::Usage {
        self.usage
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .usage
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
    fn model(&self, reference: &str) -> Option<Arc<dyn ModelClient + 'a>> {
        let mut built = self.built.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(model) = built.get(reference) {
            return model.clone();
        }
        let model = self.resolve(reference);
        built.insert(reference.to_owned(), model.clone());
        model
    }

    fn resolve(&self, reference: &str) -> Option<Arc<dyn ModelClient + 'a>> {
        let provider = ModelRef::parse(reference)
            .and_then(|model| Some((self.binding.provider(model.provider)?, model.model)));
        let Some((provider, model)) = provider else {
            self.note(format!(
                "model `{reference}` does not resolve to a declared provider; that binding is skipped"
            ));
            return None;
        };
        let model: Arc<dyn ModelClient + 'a> =
            Arc::from(self.clients.build(reference, model, &provider));
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

impl<'a> RunModels<'a> {
    /// Settles every ask: each layer of the cascade is put to the subjects
    /// still open, an abstention or an unsure answer going on to the next.
    pub(super) fn run(&self, pending: Vec<Pending<'a>>) -> Vec<(String, Vec<Diagnostic>)> {
        let mut judged: Vec<Vec<Answered>> = pending
            .iter()
            .map(|p| vec![Answered::Abstained(Abstained::Undecided); p.ask.subjects.len()])
            .collect();
        let layers = pending.iter().map(|p| p.layers.len()).max().unwrap_or(0);
        for layer in 0..layers {
            let groups = Self::groups(&pending, &judged, layer);
            let answers: Vec<(At, Answer)> = groups
                .par_iter()
                .flat_map_iter(|items| self.ask_group(&pending, layer, items))
                .collect();
            for ((p, i), answer) in answers {
                if let Ok((noul, model)) = &answer {
                    self.record(&pending[p].ask.subjects[i], *noul, model);
                }
                judged[p][i] = self.verdict(&pending[p], (layer, i), &judged[p][i], answer);
            }
        }
        pending
            .into_iter()
            .zip(judged)
            .map(|(p, judged)| (p.ask.rule, (p.settle)(judged)))
            .collect()
    }

    /// The open subjects the layer `layer` is to judge, grouped by the
    /// models and what they read of the subject: one request each.
    fn groups(pending: &[Pending<'a>], judged: &[Vec<Answered>], layer: usize) -> Vec<Vec<At>> {
        let mut groups: BTreeMap<(&str, Digest), Vec<At>> = BTreeMap::new();
        for (p, item) in pending.iter().enumerate() {
            let Some(models) = item.layers.get(layer) else {
                continue;
            };
            for (i, state) in item.states.iter().enumerate() {
                if matches!(judged[p][i], Answered::Decided(_)) {
                    continue;
                }
                groups
                    .entry((models.id.as_str(), *state))
                    .or_default()
                    .push((p, i));
            }
        }
        groups.into_values().collect()
    }

    /// What a layer says about the subjects at `items`, which share a state:
    /// kept answers first, the rest in requests of at most [`MAX_QUESTIONS`].
    fn ask_group(&self, pending: &[Pending<'a>], layer: usize, items: &[At]) -> Vec<(At, Answer)> {
        let route = &pending[items[0].0].layers[layer];
        let mut out = Vec::new();
        let mut todo = Vec::new();
        for &at in items {
            let item = &pending[at.0];
            if !self.reported(&item.ask.subjects[at.1]) {
                out.push((at, Err(Abstained::Undecided)));
                continue;
            }
            let kept = route.models().iter().find_map(|model| {
                let manifest = model.manifest();
                let noul = self.recall(manifest, item, at.1)?;
                Some((noul, named(manifest)))
            });
            match kept {
                Some(kept) => out.push((at, Ok(kept))),
                None => todo.push(at),
            }
        }
        for chunk in todo.chunks(MAX_QUESTIONS) {
            let order = route.order(self.binding.routing);
            out.extend(self.send(&order, pending, chunk));
        }
        out
    }

    /// One request, put to the models of the layer in turn until one answers.
    /// Only a failure or a refusal of the budget moves on; an abstention is an
    /// answer. Every model out, the subjects are left to review.
    fn send(
        &self,
        order: &[&Arc<dyn ModelClient + 'a>],
        pending: &[Pending<'a>],
        chunk: &[At],
    ) -> Vec<(At, Answer)> {
        let mut failure = None;
        for model in order {
            match self.attempt(model.as_ref(), pending, chunk) {
                Ok(answers) => return answers,
                Err(Miss::Failed(reason)) => failure = Some(reason),
                Err(Miss::Budget) => {}
            }
        }
        let open = match failure {
            Some(reason) => Abstained::Failed(reason),
            None => {
                self.count_over_budget(chunk.len());
                Abstained::OverBudget
            }
        };
        chunk.iter().map(|&at| (at, Err(open.clone()))).collect()
    }

    /// The request to one model: the state of the subject, one question per
    /// decision.
    fn attempt(
        &self,
        model: &dyn ModelClient,
        pending: &[Pending<'a>],
        chunk: &[At],
    ) -> Result<Vec<(At, Answer)>, Miss> {
        let manifest = model.manifest();
        if let Some(reason) = self.skipped(&manifest.id) {
            return Err(Miss::Failed(reason));
        }
        if manifest.cost > 0.0 && !self.spend() {
            return Err(Miss::Budget);
        }
        let params = request_of(
            pending[chunk[0].0].ask.subjects[chunk[0].1].state.clone(),
            chunk.iter().map(|&(p, _)| &pending[p].ask.decision),
        );
        self.count_call(manifest.cost);
        let result = model
            .classify(&params)
            .map_err(|e| Miss::Failed(self.failure(manifest, e.to_string())))?;
        if let Some(reason) = chunk.iter().find_map(|&(p, _)| {
            let decision = &pending[p].ask.decision.id;
            match result.answers.get(decision) {
                Some(answer) if answer.kind != "noul" => {
                    Some(format!("it gave an unusable answer type for `{decision}`"))
                }
                Some(answer) if !(0.0..=1.0).contains(&answer.noul) => {
                    Some(format!("it gave an invalid probability for `{decision}`"))
                }
                _ => None,
            }
        }) {
            return Err(Miss::Failed(self.failure(manifest, reason)));
        }
        let by = named(manifest);
        Ok(chunk
            .iter()
            .map(|&at| {
                let item = &pending[at.0];
                let decision = &item.ask.decision.id;
                let answer = result
                    .answers
                    .get(decision)
                    .map(|answer| (answer.noul, by.clone()))
                    .ok_or(Abstained::Undecided);
                if let Ok((noul, _)) = answer {
                    self.keep(manifest, item, at.1, noul);
                }
                (at, answer)
            })
            .collect())
    }

    /// What a model's answer makes of the subject at `(layer, i)`: failed when
    /// P(fail) reaches the band, otherwise open for the next layer. The fail of
    /// the attached model on a sampled subject is an abstention instead, with
    /// what it said kept (`prior` is what the layers above made of it).
    fn verdict(
        &self,
        item: &Pending<'a>,
        (layer, i): At,
        prior: &Answered,
        answer: Answer,
    ) -> Answered {
        let kept = explored(prior);
        let (noul, model) = match answer {
            Ok(given) => given,
            Err(Abstained::Undecided) if kept.is_some() => return prior.clone(),
            Err(open) => return Answered::Abstained(open),
        };
        let Some(confidence) = fails(noul) else {
            return if kept.is_some() {
                prior.clone()
            } else {
                Answered::Abstained(Abstained::Undecided)
            };
        };
        let subject = &item.ask.subjects[i].key;
        let sample =
            item.layers[layer].attached && sampled(&item.ask.rule, subject, self.binding.explore);
        if sample {
            return Answered::Abstained(Abstained::Explored(Explored { model, confidence }));
        }
        Answered::Decided(Decided {
            confidence,
            model,
            explored: kept.cloned(),
        })
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

/// Whether exploring takes the subject: the same subjects every run, a share
/// `explore` of them over many.
fn sampled(rule: &str, subject: &str, explore: f64) -> bool {
    let digest = sha256(format!("{rule}\0{subject}"));
    u32::from_str_radix(&digest[..8], 16).is_ok_and(|at| f64::from(at) / SAMPLE_SPAN < explore)
}

/// What an exploration of the subject said so far, kept when the layers below
/// have nothing to add.
fn explored(prior: &Answered) -> Option<&Explored> {
    match prior {
        Answered::Abstained(Abstained::Explored(explored)) => Some(explored),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
