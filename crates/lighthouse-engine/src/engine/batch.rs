//! Putting what a file's rules queued to the models in as few requests as
//! possible: one request per subject and route, holding every decision that
//! resolves to that route on it, at most [`MAX_QUESTIONS`] questions long
//! (the overflow goes in the next request). Requests run concurrently. A
//! request moves to the next model of its route when a model fails.

use std::{collections::BTreeMap, sync::Arc, sync::PoisonError, sync::atomic::Ordering};

use lighthouse_model::Diagnostic;
use lighthouse_plugin::{
    Ask, Classifier, Decided, Judged, Model, Open, Settle, decide, request_of,
};
use rayon::prelude::*;

use super::models::{Route, RunModels, digest_of};

/// The most questions put to a model in one request.
pub(super) const MAX_QUESTIONS: usize = 32;

/// What a rule asked and has not had answered: its routes, most specific
/// first, and what to make of the judgments.
pub(super) struct Pending<'a> {
    pub(super) ask: Ask,
    pub(super) settle: Settle,
    pub(super) routes: Vec<Route<'a>>,
}

/// The `i`th subject of the `p`th ask.
type At = (usize, usize);

/// P(fail) and the model that gave it as evidence names it, or why the
/// subject stays open.
type Answer = Result<(f64, String), Open>;

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
        let mut judged: Vec<Vec<Judged>> = pending
            .iter()
            .map(|p| vec![Judged::Open(Open::Undecided); p.ask.subjects.len()])
            .collect();
        let layers = pending.iter().map(|p| p.routes.len()).max().unwrap_or(0);
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
                judged[p][i] = verdict(&pending[p], answer);
            }
        }
        pending
            .into_iter()
            .zip(judged)
            .map(|(p, judged)| (p.ask.rule, (p.settle)(judged)))
            .collect()
    }

    /// The open subjects the routes of `layer` are to judge, grouped by the
    /// route and what it reads of the subject: one request each.
    fn groups(pending: &[Pending<'a>], judged: &[Vec<Judged>], layer: usize) -> Vec<Vec<At>> {
        let mut groups: BTreeMap<(String, String), Vec<At>> = BTreeMap::new();
        for (p, item) in pending.iter().enumerate() {
            let Some(route) = item.routes.get(layer) else {
                continue;
            };
            for (i, subject) in item.ask.subjects.iter().enumerate() {
                if matches!(judged[p][i], Judged::Decided(_)) {
                    continue;
                }
                groups
                    .entry((route.id.clone(), digest_of(&subject.state())))
                    .or_default()
                    .push((p, i));
            }
        }
        groups.into_values().collect()
    }

    /// What a route says about the subjects at `items`, which share a state:
    /// kept answers first, the rest in requests of at most [`MAX_QUESTIONS`].
    fn ask_group(&self, pending: &[Pending<'a>], layer: usize, items: &[At]) -> Vec<(At, Answer)> {
        let order = pending[items[0].0].routes[layer].order(self.binding.routing);
        let mut out = Vec::new();
        let mut todo = Vec::new();
        for &at in items {
            let ask = &pending[at.0].ask;
            let subject = &ask.subjects[at.1];
            if !self.reported(subject) {
                out.push((at, Err(Open::Undecided)));
                continue;
            }
            let kept = order.iter().find_map(|model| {
                let manifest = model.manifest();
                let noul = self.recall(manifest, ask, subject)?;
                Some((noul, named(manifest)))
            });
            match kept {
                Some(kept) => out.push((at, Ok(kept))),
                None => todo.push(at),
            }
        }
        for chunk in todo.chunks(MAX_QUESTIONS) {
            out.extend(self.send(&order, pending, chunk));
        }
        out
    }

    /// One request, put to the models of the route in turn until one answers.
    /// Only a failure or a refusal of the budget moves on; an abstention is an
    /// answer. Every model out, the subjects are left to review.
    fn send(
        &self,
        order: &[&Arc<dyn Classifier + 'a>],
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
            Some(reason) => Open::Failed(reason),
            None => {
                self.over_budget.fetch_add(chunk.len(), Ordering::SeqCst);
                Open::OverBudget
            }
        };
        chunk.iter().map(|&at| (at, Err(open.clone()))).collect()
    }

    /// The request to one model: the state of the subject, one question per
    /// decision.
    fn attempt(
        &self,
        model: &dyn Classifier,
        pending: &[Pending<'a>],
        chunk: &[At],
    ) -> Result<Vec<(At, Answer)>, Miss> {
        let manifest = model.manifest();
        if let Some(reason) = self.cooled(&manifest.id) {
            return Err(Miss::Failed(reason));
        }
        if manifest.cost > 0.0 && !self.spend() {
            return Err(Miss::Budget);
        }
        let params = request_of(
            pending[chunk[0].0].ask.subjects[chunk[0].1].state(),
            chunk.iter().map(|&(p, _)| &pending[p].ask.decision),
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.cost.lock().unwrap_or_else(PoisonError::into_inner) += manifest.cost;
        let result = model
            .classify(&params)
            .map_err(|e| Miss::Failed(self.fail(manifest, e.to_string())))?;
        let by = named(manifest);
        Ok(chunk
            .iter()
            .map(|&at| {
                let ask = &pending[at.0].ask;
                let answer = match result.answers.get(&ask.decision.id) {
                    Some(answer) if answer.kind == "noul" => Ok((answer.noul, by.clone())),
                    _ => Err(Open::Failed(self.report(
                        manifest,
                        format!("it gave no `noul` answer for `{}`", ask.decision.id),
                    ))),
                };
                if let Ok((noul, _)) = answer {
                    self.keep(manifest, ask, &ask.subjects[at.1], noul);
                }
                (at, answer)
            })
            .collect())
    }
}

/// What a model's answer makes of a subject: failed when P(fail) reaches the
/// band, otherwise open for the next layer.
fn verdict(item: &Pending<'_>, answer: Answer) -> Judged {
    match answer {
        Ok((noul, model)) => match decide(noul, item.ask.upper) {
            Some(confidence) => Judged::Decided(Decided { confidence, model }),
            None => Judged::Open(Open::Undecided),
        },
        Err(open) => Judged::Open(open),
    }
}

/// The model as evidence names it: `<provider>/<model>@<version>`.
fn named(manifest: &Model) -> String {
    format!("{}@{}", manifest.id, manifest.version)
}
