//! The chat route of a project: the models `[models] chat` names, asked for
//! text (the fix of a finding). A request goes to the models in turn until one
//! answers: a failure or a spent budget moves it on, a kept answer is reused
//! without a call. Answers are kept under the model at its version and the
//! digest of the request, in the result cache of the engine.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use lighthouse_model::ModelRef;
use lighthouse_plugin::{ChatRequest, CreateChatCompletionRequest, ModelClient};
use lighthouse_spec::{CommandProtocol, ModelProvider, Routing};

use super::{Engine, models::named};
use crate::cache::{Cache, Digest, KeyBuilder, digest_of};

/// Failures of a model, in one run, after which it is no longer asked.
const MAX_FAILURES: usize = 3;

/// One model of the route, as the route asks it.
struct Member<'a> {
    client: Box<dyn ModelClient + 'a>,
    /// The model answers in plain text: its request carries no response format.
    plain: bool,
}

/// An answer: the text of the first choice and the model that gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reply {
    pub(crate) text: String,
    /// `<provider>/<model>`.
    pub(crate) model: String,
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
}

/// The models of the chat route and what a run has spent on them.
pub(crate) struct ChatRoute<'a> {
    members: Vec<Member<'a>>,
    routing: Routing,
    /// Requests that went through the route, to rotate its models.
    turn: AtomicUsize,
    budget: AtomicUsize,
    cache: Option<Cache>,
    /// The answers of this run, which the cache may not keep.
    memo: Mutex<BTreeMap<Digest, String>>,
    failures: Mutex<BTreeMap<String, (usize, String)>>,
    spent: Mutex<Spent>,
    notes: Mutex<BTreeSet<String>>,
}

impl Engine {
    /// The route of the models `[models] chat` names; `None` when no model is
    /// bound or the run asks no model.
    pub(crate) fn chat_route(&self) -> Option<ChatRoute<'_>> {
        if self.binding.chat.is_empty() || self.models.off {
            return None;
        }
        let clients = self.clients();
        let mut notes = BTreeSet::new();
        let members: Vec<Member<'_>> = self
            .binding
            .chat
            .iter()
            .filter_map(|reference| {
                let model = ModelRef::parse(reference)?;
                let provider = self.binding.provider(model.provider)?;
                let plain = matches!(
                    provider,
                    ModelProvider::Command {
                        protocol: CommandProtocol::PlainText,
                        ..
                    }
                );
                let client = clients.build(reference, model.model, &provider);
                Some(Member { client, plain })
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
        Some(ChatRoute {
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

impl ChatRoute<'_> {
    /// The answer of the route to the request `build` writes: it gets whether
    /// the model takes a response format, which a plain-text command does not.
    pub(crate) fn ask(
        &self,
        build: &dyn Fn(bool) -> CreateChatCompletionRequest,
    ) -> Result<Reply, Refusal> {
        let mut refusal = Refusal::Budget;
        for member in self.order() {
            let manifest = member.client.manifest();
            let body = build(!member.plain);
            let key = answer_key(&named(manifest), &body);
            if let Some(text) = self.kept(&key) {
                return Ok(Reply {
                    text,
                    model: manifest.id.clone(),
                });
            }
            if let Some(reason) = self.skipped(&manifest.id) {
                refusal = Refusal::Failed(reason);
                continue;
            }
            if manifest.cost > 0.0 && !self.spend() {
                continue;
            }
            self.count_call(manifest.cost);
            let request = ChatRequest {
                body,
                options: serde_json::Map::new(),
            };
            let answered = member
                .client
                .chat(&request)
                .map_err(|e| e.to_string())
                .and_then(|response| {
                    response
                        .choices
                        .into_iter()
                        .next()
                        .and_then(|choice| choice.message.content)
                        .ok_or_else(|| "it answered with no text".to_owned())
                });
            match answered {
                Ok(text) => {
                    self.keep(key, &text);
                    return Ok(Reply {
                        text,
                        model: manifest.id.clone(),
                    });
                }
                Err(reason) => refusal = Refusal::Failed(self.failure(&manifest.id, reason)),
            }
        }
        Err(refusal)
    }

    /// Requests that went to a model this run (kept answers are not counted),
    /// and what they cost.
    pub(crate) fn usage(&self) -> (usize, f64) {
        let spent = self.spent.lock().unwrap_or_else(PoisonError::into_inner);
        (spent.calls, spent.cost)
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

    fn count_call(&self, cost: f64) {
        let mut spent = self.spent.lock().unwrap_or_else(PoisonError::into_inner);
        spent.calls += 1;
        spent.cost += cost;
    }

    /// The answer kept for `key`, this run or an earlier one.
    fn kept(&self, key: &Digest) -> Option<String> {
        let memo = self.memo.lock().unwrap_or_else(PoisonError::into_inner);
        memo.get(key)
            .cloned()
            .or_else(|| self.cache.as_ref()?.get(key))
    }

    fn keep(&self, key: Digest, text: &str) {
        if let Some(cache) = &self.cache {
            cache.put(key, &text);
        }
        self.memo
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, text.to_owned());
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

/// The key of an answer: the model at its version, and what it is asked.
fn answer_key(model: &str, body: &CreateChatCompletionRequest) -> Digest {
    let mut key = KeyBuilder::new();
    key.part("chat-answer")
        .part(model)
        .part(digest_of(body).unwrap_or_default());
    key.finish()
}
