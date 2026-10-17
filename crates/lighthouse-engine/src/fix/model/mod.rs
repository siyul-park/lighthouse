//! The fix by a model. A model is given the code of a finding, the symbol it
//! is about or the small file it is in, and answers the new text of it. It
//! proposes one replacement, which the orchestrator applies in memory and
//! keeps only if the checks accept it (see [`verify`]); a model's word is
//! never taken for the result.

mod account;
mod extent;
mod prompt;
mod scratch;
mod verify;

use std::collections::BTreeMap;

use lighthouse_model::{
    Safety,
    ucm::{EditOp, FixOutcome},
};
use lighthouse_plugin::{Error, FixRequest, Fixer, FixerManifest};
use lighthouse_protocol::prompt::json_in;
use lighthouse_spec::{Example, ExampleKind, Fix, FixKind};
use serde_json::Value;

use super::FixedBy;
use crate::{
    Engine,
    engine::{ChatRoute, Refusal},
};
pub use account::ModelCounts;
pub(super) use account::{Account, Class, Event};
use prompt::Ask;
pub(super) use scratch::run_verify;
pub(super) use verify::{Verdict, judge};

/// What a decision tells the model besides its requirement.
#[derive(Debug, Clone, Default)]
pub(super) struct ModelFix {
    /// The `prompt` of the fix.
    instruction: Option<String>,
    /// Examples that show a fix.
    examples: Vec<Example>,
    /// The decision has no fix of its own: it is given to a model because the
    /// project binds one.
    implicit: bool,
}

impl ModelFix {
    /// The fix by a model of a decision whose `fix` is `fix`, or that has none
    /// and `implicit` is true: `None` for a fix made without a model.
    pub(super) fn of(fix: Option<&Fix>, examples: &[Example], implicit: bool) -> Option<Self> {
        let instruction = match fix.map(|f| &f.kind) {
            None => None,
            Some(FixKind::Model { prompt }) => prompt.clone(),
            Some(_) => return None,
        };
        Some(Self {
            instruction,
            examples: examples
                .iter()
                .filter(|e| e.kind == ExampleKind::Invalid && !e.fixed.is_empty())
                .cloned()
                .collect(),
            implicit,
        })
    }

    /// Whether the decision has no fix of its own.
    pub(super) fn implicit(&self) -> bool {
        self.implicit
    }
}

/// The fixer of the findings given to a model: asks the route of `[models]
/// chat` and counts what happens.
pub(super) struct ModelFixer<'e> {
    route: ChatRoute<'e>,
    fixes: &'e BTreeMap<String, ModelFix>,
    pub(super) account: Account,
    manifest: FixerManifest,
}

impl<'e> ModelFixer<'e> {
    /// The fixer of the model fixes of `fixes`; `None` when the run has none
    /// or no model to ask.
    pub(super) fn new(engine: &'e Engine, fixes: &'e BTreeMap<String, ModelFix>) -> Option<Self> {
        if fixes.is_empty() {
            return None;
        }
        Some(Self {
            route: engine.chat_route()?,
            fixes,
            account: Account::default(),
            manifest: FixerManifest {
                id: FixedBy::Model.to_string(),
                requires: Vec::new(),
            },
        })
    }

    /// Requests that went to a model this run, and what they cost.
    pub(super) fn usage(&self) -> (usize, f64) {
        self.route.usage()
    }

    /// What the user should know about the route, once the run is over.
    pub(super) fn finish(&self) -> Vec<String> {
        self.route.finish()
    }

    fn decline(&self, decision: &str, class: Class, reason: String) -> Result<FixOutcome, Error> {
        self.account.add(decision, Event::Declined(class));
        Ok(FixOutcome::declined(reason))
    }
}

impl Fixer for ModelFixer<'_> {
    fn manifest(&self) -> &FixerManifest {
        &self.manifest
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, Error> {
        let (decision, finding) = (request.decision, request.finding);
        let extent = match extent::choose(request.project, finding, request.text) {
            Ok(extent) => extent,
            Err(reason) => return self.decline(&decision.id, Class::NoExtent, reason),
        };
        let fix = self.fixes.get(&decision.id);
        let language = request
            .project
            .file(&finding.file)
            .map_or("", |f| f.lang.as_str());
        let examples = fix.map_or_else(Vec::new, |f| prompt::examples_for(&f.examples, language));
        let ask = Ask {
            decision,
            instruction: fix.and_then(|f| f.instruction.as_deref()),
            finding,
            language,
            extent: &extent,
            examples: &examples,
        };
        let (structured, plain) = match (ask.request(true), ask.request(false)) {
            (Ok(structured), Ok(plain)) => (structured, plain),
            (Err(e), _) | (_, Err(e)) => {
                let reason = format!("the request could not be built: {e}");
                return self.decline(&decision.id, Class::Unanswered, reason);
            }
        };
        self.account.add(&decision.id, Event::Asked);
        let asked = |takes_format: bool| {
            if takes_format {
                structured.clone()
            } else {
                plain.clone()
            }
        };
        let reply = match self.route.ask(&asked) {
            Ok(reply) => reply,
            Err(Refusal::Budget) => {
                let reason = "the budget for models is spent".to_owned();
                return self.decline(&decision.id, Class::Unanswered, reason);
            }
            Err(Refusal::Failed(why)) => {
                let reason = format!("no model answered: {why}");
                return self.decline(&decision.id, Class::Unanswered, reason);
            }
        };
        self.account.add(&decision.id, Event::Answered);
        let text = match replacement(&reply.text, &extent.text) {
            Ok(text) => text,
            Err((class, reason)) => return self.decline(&decision.id, class, reason),
        };
        Ok(FixOutcome::Proposed {
            description: format!("model fix by `{}`", reply.model),
            ops: vec![EditOp::Replace {
                file: finding.file.clone(),
                span: extent.span,
                text,
            }],
            safety: Safety::Suggested,
        })
    }
}

/// The new text of the extent a model's answer holds: the `replacement` of the
/// first JSON object in it, with the extent's trailing newline kept as it was.
fn replacement(answer: &str, extent: &str) -> Result<String, (Class, String)> {
    let unparsed = || {
        (
            Class::Unparsed,
            "the answer did not parse: it holds no JSON object with a `replacement` text"
                .to_owned(),
        )
    };
    let text = json_objects(answer)
        .find_map(|object| object.get("replacement")?.as_str().map(str::to_owned))
        .ok_or_else(unparsed)?;
    let text = match (extent.ends_with('\n'), text.ends_with('\n')) {
        (true, false) if !text.is_empty() => text + "\n",
        (false, true) => text.trim_end_matches('\n').to_owned(),
        _ => text,
    };
    if text == extent {
        return Err((
            Class::Unchanged,
            "the model returned the code unchanged".to_owned(),
        ));
    }
    Ok(text)
}

/// The JSON objects in a model's text: the text itself or a fenced block when
/// that is one, else every object that starts at a brace in it.
fn json_objects(answer: &str) -> impl Iterator<Item = Value> + '_ {
    let whole = json_in(answer).filter(Value::is_object);
    let scattered = answer.match_indices('{').filter_map(|(at, _)| {
        serde_json::Deserializer::from_str(&answer[at..])
            .into_iter::<Value>()
            .next()?
            .ok()
            .filter(Value::is_object)
    });
    whole.into_iter().chain(scattered)
}

#[cfg(test)]
mod tests;
