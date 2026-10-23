//! The model half of a `cel` check: the subjects `abstain` suspects, put to the
//! models the decision is bound to. A `fail` is reported with the check's own
//! message and identity and the model's evidence; a subject no model failed is
//! left to agent review.

use cel::Program;
use lighthouse_model::{Diagnostic, Position, Span, ucm::SymbolId};
use lighthouse_plugin::{
    Answered, Ask, Context, DecisionRef, Neighbor, RuleManifest, State, Subject,
};
use lighthouse_protocol::hash;
use lighthouse_spec::{Cel, CelCheck, Decision, Select};
use serde_json::{Value, json};

use crate::{
    Error,
    eval::{Frame, compile, render, to_json},
    glob::slashed,
};

/// The most characters of a subject's source put to a model.
const MAX_TEXT: usize = 6000;
/// The most characters of a neighbor's source.
const MAX_NEIGHBOR_TEXT: usize = 600;
/// The most callers and the most callees put to a model with a subject.
const MAX_NEIGHBORS: usize = 5;

/// What a check asks a model, compiled.
pub(crate) struct Abstain {
    program: Program,
    fact: Program,
    select: Select,
    decision: DecisionRef,
    revision: String,
}

/// A subject `abstain` picked, with the finding it becomes if a model says
/// `fail`.
pub(crate) struct Candidate {
    pub(crate) diagnostic: Diagnostic,
    /// The facts of the subject, as the builder made them.
    pub(crate) fact: Value,
    /// `where` already made it a finding: the models are not asked, but a run
    /// that collects subjects for training keeps it.
    pub(crate) decided: bool,
}

impl Abstain {
    /// Compiles the `abstain` of `check`; `None` for a check without one.
    pub(crate) fn new(
        cel: &Cel,
        decision: &Decision,
        check: &CelCheck,
        select: Select,
    ) -> Result<Option<Self>, Error> {
        let Some(source) = &check.abstain else {
            return Ok(None);
        };
        let id = decision.id();
        let decision_ref = decision.reference();
        let revision = serde_json::to_vec(&(decision.uid(), check, &decision_ref))
            .map(hash::sha256)
            .map_err(|e| Error::invalid(id, format!("the abstain cannot be hashed: {e}")))?;
        Ok(Some(Self {
            program: compile(cel, id, "abstain", source)?,
            fact: compile(cel, id, "abstain", select.variable())?,
            select,
            decision: decision_ref,
            revision,
        }))
    }

    /// The expression that says whether a subject is asked about.
    pub(crate) fn program(&self) -> &Program {
        &self.program
    }

    /// Whether the subject in `frame` is to be asked about.
    pub(crate) fn asks(&self, frame: &Frame) -> Result<bool, lighthouse_plugin::Error> {
        match frame.run(&self.program)? {
            cel::Value::Bool(asks) => Ok(asks),
            other => Err(frame.fail(format!("`abstain` is {}, not a bool", render(&other)))),
        }
    }

    /// The facts of the subject in `frame`, as JSON.
    pub(crate) fn fact_of(&self, frame: &Frame) -> Result<Value, lighthouse_plugin::Error> {
        Ok(to_json(&frame.run(&self.fact)?))
    }

    /// Queues the candidates of the focused file for the models, which are
    /// asked when the file's rules are done, so that the decisions on one
    /// subject share a request. A subject a model fails is a finding; one
    /// nothing decided (undecided, unbound, over budget, a model's error) is a
    /// diagnostic marked `review`, which the engine reports apart. The
    /// findings come back from the engine's flush, not from here, unless no
    /// model could be asked.
    pub(crate) fn settle(
        &self,
        ctx: &Context,
        rule: &RuleManifest,
        candidates: Vec<Candidate>,
    ) -> Vec<Diagnostic> {
        let Some((file, text)) = ctx.file else {
            return Vec::new();
        };
        if candidates.is_empty() {
            return Vec::new();
        }
        if ctx.models.collecting() {
            let seen = candidates.iter().map(|c| self.subject(ctx, text, c));
            ctx.models.observe(seen.collect());
        }
        let candidates: Vec<Candidate> = candidates.into_iter().filter(|c| !c.decided).collect();
        if candidates.is_empty() {
            return Vec::new();
        }
        let subjects = candidates
            .iter()
            .map(|c| self.subject(ctx, text, c))
            .collect();
        let ask = Ask {
            rule: rule.id.clone(),
            revision: self.revision.clone(),
            decision: self.decision.clone(),
            subjects,
        };
        let decision = self.decision.clone();
        let settle = Box::new(move |answered: Vec<Answered>| {
            answered
                .into_iter()
                .zip(candidates)
                .map(|(answered, candidate)| outcome(answered, candidate, &decision))
                .collect()
        });
        let group = file.path.to_string_lossy();
        ctx.models.queue(&group, ask, settle).unwrap_or_default()
    }

    fn subject(&self, ctx: &Context, source: &str, candidate: &Candidate) -> Subject {
        let diagnostic = &candidate.diagnostic;
        let symbol = diagnostic
            .symbol
            .as_deref()
            .and_then(SymbolId::parse)
            .and_then(|id| ctx.project.symbol(&id));
        let text = match (self.select, symbol) {
            (Select::File, _) | (_, None) => {
                if self.select == Select::File {
                    cut(source, MAX_TEXT)
                } else {
                    cut(&lines(source, diagnostic.span), MAX_TEXT)
                }
            }
            (_, Some(symbol)) => cut(
                &lines(source, symbol.extent.unwrap_or(symbol.span)),
                MAX_TEXT,
            ),
        };
        Subject {
            key: diagnostic.fingerprint.as_str().to_owned(),
            kind: self.select.name().to_owned(),
            id: symbol.map_or_else(
                || diagnostic.file.to_string_lossy().into_owned(),
                |s| s.id.as_str().to_owned(),
            ),
            state: State {
                language: ctx
                    .project
                    .file(&diagnostic.file)
                    .map(|f| f.lang.clone())
                    .unwrap_or_default(),
                file: slashed(&diagnostic.file),
                text,
                facts: candidate.fact.clone(),
                neighbors: symbol.map_or_else(Vec::new, |s| neighbors(ctx, source, s)),
            },
        }
    }
}

/// What the models' answer makes of a candidate: the finding it confirms,
/// with the confidence as the evidence and the model as its version; a review
/// task for a subject left undecided.
fn outcome(answered: Answered, candidate: Candidate, decision: &DecisionRef) -> Diagnostic {
    let mut diagnostic = candidate.diagnostic;
    if diagnostic.message.is_empty() {
        diagnostic.message.clone_from(&decision.requirement);
    }
    match answered {
        Answered::Decided(decided) => {
            diagnostic.evidence = json!({ "confidence": decided.confidence });
            diagnostic.model_version = Some(decided.model);
            if let Some(explored) = decided.explored {
                diagnostic.evidence["explored"] = json!({ "confidence": explored.confidence });
            }
        }
        Answered::Abstained(abstained) => diagnostic.review = Some(abstained.to_string()),
    }
    diagnostic
}

/// The callers and callees of a symbol, with the source of those in the same
/// file.
fn neighbors(ctx: &Context, source: &str, symbol: &lighthouse_model::ucm::Symbol) -> Vec<Neighbor> {
    let project = ctx.project;
    let around = |ids: &[SymbolId], relation: &str| -> Vec<Neighbor> {
        ids.iter()
            .filter_map(|id| project.symbol(id))
            .take(MAX_NEIGHBORS)
            .map(|near| Neighbor {
                id: near.id.as_str().to_owned(),
                kind: json!(near.kind).as_str().unwrap_or_default().to_owned(),
                relation: relation.to_owned(),
                text: if near.file == symbol.file {
                    cut(
                        &lines(source, near.extent.unwrap_or(near.span)),
                        MAX_NEIGHBOR_TEXT,
                    )
                } else {
                    String::new()
                },
            })
            .collect()
    };
    let mut found = around(project.callers(&symbol.id), "caller");
    found.extend(around(project.callees(&symbol.id), "callee"));
    found
}

/// The lines of `source` a span covers.
fn lines(source: &str, span: Span) -> String {
    let Span {
        start: Position { line: first, .. },
        end: Position { line: last, .. },
    } = span;
    source
        .lines()
        .skip((first as usize).saturating_sub(1))
        .take((last.max(first) - first + 1) as usize)
        .collect::<Vec<_>>()
        .join("\n")
}

/// At most `max` characters of `text`.
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
