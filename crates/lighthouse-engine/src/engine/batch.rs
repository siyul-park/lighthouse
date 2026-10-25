//! Putting what a file's rules queued to the models in as few requests as
//! possible: one request per subject and route, holding every decision that
//! resolves to that route on it, at most [`MAX_QUESTIONS`] questions long
//! (the overflow goes in the next request). Requests run concurrently. A
//! request moves to the next model of its route when a model fails.

use std::{collections::BTreeMap, sync::Arc};

use crate::cache::Digest;
use lighthouse_model::Diagnostic;
use lighthouse_plugin::{
    Abstained, Answered, Ask, Decided, Explored, ModelClient, Settle, fails, request_of,
};
use lighthouse_protocol::hash::sha256;
use rayon::prelude::*;

use super::models::{Layer, RunModels, named};

/// The most questions put to a model in one request.
pub(super) const MAX_QUESTIONS: usize = 32;

/// The values the first eight hex digits of a digest take, which makes a
/// subject's place in the sample a share between 0 and 1.
const SAMPLE_SPAN: f64 = 4_294_967_296.0;

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
