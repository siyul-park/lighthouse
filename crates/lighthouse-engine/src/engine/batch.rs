//! Putting what a file's rules queued to the models in as few requests as
//! possible: one request per subject and model, holding every decision that
//! resolves to that model on it, at most [`MAX_QUESTIONS`] questions long
//! (the overflow goes in the next request). Requests run concurrently.

use std::{collections::BTreeMap, sync::Arc, sync::PoisonError, sync::atomic::Ordering};

use lighthouse_model::Diagnostic;
use lighthouse_plugin::{
    Ask, Classifier, Decided, Judged, Model, Open, Question, Settle, SystemOneRequest, decide,
};
use rayon::prelude::*;

use super::models::{RunModels, digest_of};

/// The most questions put to a model in one request.
pub(super) const MAX_QUESTIONS: usize = 32;

/// What a rule asked and has not had answered: its models, most specific
/// first, and what to make of the judgments.
pub(super) struct Pending<'a> {
    pub(super) ask: Ask,
    pub(super) settle: Settle,
    pub(super) models: Vec<Arc<dyn Classifier + 'a>>,
}

/// The `i`th subject of the `p`th ask.
type At = (usize, usize);

type Answer = Result<f64, Open>;

impl<'a> RunModels<'a> {
    /// Settles every ask: each layer of the cascade is put to the subjects
    /// still open, an abstention or an unsure answer going on to the next.
    pub(super) fn run(&self, pending: Vec<Pending<'a>>) -> Vec<(String, Vec<Diagnostic>)> {
        let mut judged: Vec<Vec<Judged>> = pending
            .iter()
            .map(|p| vec![Judged::Open(Open::Undecided); p.ask.subjects.len()])
            .collect();
        let layers = pending.iter().map(|p| p.models.len()).max().unwrap_or(0);
        for layer in 0..layers {
            let groups = Self::groups(&pending, &judged, layer);
            let answers: Vec<(At, Answer)> = groups
                .par_iter()
                .flat_map_iter(|items| self.ask_group(&pending, layer, items))
                .collect();
            for ((p, i), answer) in answers {
                if let Ok(noul) = answer {
                    self.record(&pending[p].ask.subjects[i], noul);
                }
                judged[p][i] = verdict(&pending[p], layer, answer);
            }
        }
        pending
            .into_iter()
            .zip(judged)
            .map(|(p, judged)| (p.ask.rule, (p.settle)(judged)))
            .collect()
    }

    /// The open subjects the models of `layer` are to judge, grouped by the
    /// model and what it reads of the subject: one request each.
    fn groups(pending: &[Pending<'a>], judged: &[Vec<Judged>], layer: usize) -> Vec<Vec<At>> {
        let mut groups: BTreeMap<(String, String), Vec<At>> = BTreeMap::new();
        for (p, item) in pending.iter().enumerate() {
            let Some(model) = item.models.get(layer) else {
                continue;
            };
            for (i, subject) in item.ask.subjects.iter().enumerate() {
                if matches!(judged[p][i], Judged::Decided(_)) {
                    continue;
                }
                groups
                    .entry((model.manifest().id.clone(), digest_of(&subject.state())))
                    .or_default()
                    .push((p, i));
            }
        }
        groups.into_values().collect()
    }

    /// What one model says about the subjects at `items`, which share a state:
    /// kept answers first, the rest in requests of at most [`MAX_QUESTIONS`].
    fn ask_group(&self, pending: &[Pending<'a>], layer: usize, items: &[At]) -> Vec<(At, Answer)> {
        let model = pending[items[0].0].models[layer].as_ref();
        let manifest = model.manifest();
        let mut out = Vec::new();
        let mut todo = Vec::new();
        for &at in items {
            let ask = &pending[at.0].ask;
            let subject = &ask.subjects[at.1];
            if !self.reported(subject) {
                out.push((at, Err(Open::Undecided)));
            } else if let Some(noul) = self.recall(manifest, ask, subject) {
                out.push((at, Ok(noul)));
            } else {
                todo.push(at);
            }
        }
        for chunk in todo.chunks(MAX_QUESTIONS) {
            out.extend(self.send(model, pending, chunk));
        }
        out
    }

    /// One request: the state of the subject, one question per decision.
    fn send(
        &self,
        model: &dyn Classifier,
        pending: &[Pending<'a>],
        chunk: &[At],
    ) -> Vec<(At, Answer)> {
        let manifest = model.manifest();
        let all = |open: Open| chunk.iter().map(|&at| (at, Err(open.clone()))).collect();
        if manifest.cost > 0.0 && !self.spend() {
            self.over_budget.fetch_add(chunk.len(), Ordering::SeqCst);
            return all(Open::OverBudget);
        }
        let params = SystemOneRequest {
            state: pending[chunk[0].0].ask.subjects[chunk[0].1].state(),
            model: String::new(),
            questions: chunk
                .iter()
                .map(|&(p, _)| {
                    let decision = &pending[p].ask.decision;
                    (decision.id.clone(), Question::of(decision))
                })
                .collect(),
            options: serde_json::Map::new(),
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.cost.lock().unwrap_or_else(PoisonError::into_inner) += manifest.cost;
        let failed = |reason: String| {
            self.note(format!(
                "model {} failed, its subjects are left to review: {reason}",
                manifest.id
            ));
            Open::Failed(reason)
        };
        let result = match model.classify(&params) {
            Ok(result) => result,
            Err(e) => return all(failed(e.to_string())),
        };
        chunk
            .iter()
            .map(|&at| {
                let ask = &pending[at.0].ask;
                let answer = match result.answers.get(&ask.decision.id) {
                    Some(answer) if answer.kind == "noul" => Ok(answer.noul),
                    _ => Err(failed(format!(
                        "it gave no `noul` answer for `{}`",
                        ask.decision.id
                    ))),
                };
                if let Ok(noul) = answer {
                    self.keep(manifest, ask, &ask.subjects[at.1], noul);
                }
                (at, answer)
            })
            .collect()
    }
}

/// What a model's answer makes of a subject: failed when P(fail) reaches the
/// band, otherwise open for the next layer.
fn verdict(item: &Pending<'_>, layer: usize, answer: Answer) -> Judged {
    match answer {
        Ok(noul) => match decide(noul, item.ask.upper) {
            Some(confidence) => Judged::Decided(Decided {
                confidence,
                model: named(item.models[layer].manifest()),
            }),
            None => Judged::Open(Open::Undecided),
        },
        Err(open) => Judged::Open(open),
    }
}

/// The model as evidence names it: `<provider>/<model>@<version>`.
fn named(manifest: &Model) -> String {
    format!("{}@{}", manifest.id, manifest.version)
}
