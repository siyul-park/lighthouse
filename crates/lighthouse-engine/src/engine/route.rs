//! The route of a task: the models `[models]` binds to `edit`, asked to
//! generate (the fix of a finding). A request goes to the models in
//! turn until one answers: a failure or a spent budget moves it on, a kept
//! answer is reused without a call. Answers are kept under a key the task
//! builds from the model at its version and what it is asked, in the result
//! cache of the engine.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use lighthouse_model::ModelRef;
use lighthouse_plugin::ModelClient;
use lighthouse_spec::Routing;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::{Engine, models::named};
use crate::cache::{Cache, Digest};

/// Failures of a model, in one run, after which it is no longer asked.
const MAX_FAILURES: usize = 3;

/// One model of the route, as the route asks it.
pub(crate) struct Member<'a> {
    pub(crate) client: Box<dyn ModelClient + 'a>,
}

/// Why no model of the route answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The budget refused every model that was left.
    Budget,
    /// The models failed, or are no longer asked; the last reason.
    Failed(String),
}

/// What the route was asked and cost this run.
#[derive(Default)]
struct Spent {
    calls: usize,
    cost: f64,
    /// Time the models took to answer.
    time: Duration,
}

/// What a run has spent on the models of a route.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Usage {
    /// Requests that went to a model; kept answers are not counted.
    pub(crate) calls: usize,
    pub(crate) cost: f64,
    pub(crate) time: Duration,
}

/// The models of a route and what a run has spent on them.
pub(crate) struct Route<'a> {
    members: Vec<Member<'a>>,
    routing: Routing,
    /// Requests that went through the route, to rotate its models.
    turn: AtomicUsize,
    budget: AtomicUsize,
    cache: Option<Cache>,
    /// The answers of this run, which the cache may not keep.
    memo: Mutex<BTreeMap<Digest, Value>>,
    failures: Mutex<BTreeMap<String, (usize, String)>>,
    spent: Mutex<Spent>,
    notes: Mutex<BTreeSet<String>>,
}

impl Engine {
    /// The route of the models `[models] edit` names; `None` when no model is
    /// bound or the run asks no model.
    pub(crate) fn edit_route(&self) -> Option<Route<'_>> {
        self.route(&self.binding.edit)
    }

    fn route(&self, bound: &[String]) -> Option<Route<'_>> {
        if bound.is_empty() || self.models.off {
            return None;
        }
        let clients = self.clients();
        let mut notes = BTreeSet::new();
        let members: Vec<Member<'_>> = bound
            .iter()
            .filter_map(|reference| {
                let model = ModelRef::parse(reference)?;
                let provider = self.binding.provider(model.provider)?;
                let client = clients.build(reference, model.model, &provider);
                Some(Member { client })
            })
            .collect();
        if members.is_empty() {
            return None;
        }
        let cache = self.cache.as_ref().and_then(|settings| {
            Cache::open(&settings.dir, settings.limit)
                .map_err(|e| {
                    notes.insert(format!("the answers of models are not kept: {e}"));
                })
                .ok()
        });
        Some(Route {
            members,
            routing: self.binding.routing,
            turn: AtomicUsize::new(0),
            budget: AtomicUsize::new(self.binding.budget),
            cache,
            memo: Mutex::default(),
            failures: Mutex::default(),
            spent: Mutex::default(),
            notes: Mutex::new(notes),
        })
    }
}

impl Route<'_> {
    /// The answer of the first model of the order that gives one, and that
    /// model, `<provider>/<model>`. `key` is what a model's answer is kept
    /// under, given the model and its name at its version; `call` asks the
    /// model, and its failure moves on to the next.
    pub(crate) fn through<T: Serialize + DeserializeOwned>(
        &self,
        key: &dyn Fn(&Member<'_>, &str) -> Digest,
        call: &dyn Fn(&Member<'_>) -> Result<T, String>,
    ) -> Result<(T, String), Refusal> {
        let mut refusal = Refusal::Budget;
        for member in self.order() {
            let manifest = member.client.manifest();
            let key = key(member, &named(manifest));
            if let Some(kept) = self.kept(&key) {
                return Ok((kept, manifest.id.clone()));
            }
            if let Some(reason) = self.skipped(&manifest.id) {
                refusal = Refusal::Failed(reason);
                continue;
            }
            if manifest.cost > 0.0 && !self.spend() {
                continue;
            }
            let started = Instant::now();
            let answered = call(member);
            self.count_call(manifest.cost, started.elapsed());
            match answered {
                Ok(answer) => {
                    self.keep(key, &answer);
                    return Ok((answer, manifest.id.clone()));
                }
                Err(reason) => refusal = Refusal::Failed(self.failure(&manifest.id, reason)),
            }
        }
        Err(refusal)
    }

    /// What the requests that went to a model this run took.
    pub(crate) fn usage(&self) -> Usage {
        let spent = self.spent.lock().unwrap_or_else(PoisonError::into_inner);
        Usage {
            calls: spent.calls,
            cost: spent.cost,
            time: spent.time,
        }
    }

    /// Writes the answers the cache is to keep and returns what the user
    /// should know about the route.
    pub(crate) fn finish(&self) -> Vec<String> {
        let mut notes =
            std::mem::take(&mut *self.notes.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(cache) = &self.cache
            && let Err(e) = cache.finish()
        {
            notes.insert(format!("the answers of models were not saved: {e}"));
        }
        notes.into_iter().collect()
    }

    /// The models in the order a request tries them: as written, or rotated by
    /// one more than the request before.
    fn order(&self) -> impl Iterator<Item = &Member<'_>> {
        let start = match self.routing {
            Routing::Ordered => 0,
            Routing::RoundRobin => self.turn.fetch_add(1, Ordering::SeqCst) % self.members.len(),
        };
        self.members
            .iter()
            .cycle()
            .skip(start)
            .take(self.members.len())
    }

    /// Takes one unit of the budget for a request to a model that costs.
    fn spend(&self) -> bool {
        self.budget
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    fn count_call(&self, cost: f64, took: Duration) {
        let mut spent = self.spent.lock().unwrap_or_else(PoisonError::into_inner);
        spent.calls += 1;
        spent.cost += cost;
        spent.time += took;
    }

    /// The answer kept for `key`, this run or an earlier one.
    fn kept<T: DeserializeOwned>(&self, key: &Digest) -> Option<T> {
        let memo = self.memo.lock().unwrap_or_else(PoisonError::into_inner);
        memo.get(key)
            .and_then(|kept| serde_json::from_value(kept.clone()).ok())
            .or_else(|| self.cache.as_ref()?.get(key))
    }

    fn keep<T: Serialize>(&self, key: Digest, answer: &T) {
        if let Some(cache) = &self.cache {
            cache.put(key, answer);
        }
        if let Ok(value) = serde_json::to_value(answer) {
            self.memo
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, value);
        }
    }

    /// Why a model is skipped, when it failed [`MAX_FAILURES`] times.
    fn skipped(&self, model: &str) -> Option<String> {
        let failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = failures.get(model)?;
        (*count >= MAX_FAILURES).then(|| last.clone())
    }

    /// Notes that `model` failed; at [`MAX_FAILURES`] it is skipped for the
    /// rest of the run. Returns the reason.
    fn failure(&self, model: &str, reason: String) -> String {
        let mut notes = self.notes.lock().unwrap_or_else(PoisonError::into_inner);
        notes.insert(format!(
            "model {model} failed, its fixes are declined: {reason}"
        ));
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let (count, last) = failures.entry(model.to_owned()).or_default();
        *count += 1;
        last.clone_from(&reason);
        if *count == MAX_FAILURES {
            notes.insert(format!(
                "model {model} failed {MAX_FAILURES} times; it is skipped for the rest of the run"
            ));
        }
        reason
    }
}
