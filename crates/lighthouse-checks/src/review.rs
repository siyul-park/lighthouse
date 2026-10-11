//! The model half of a `cel` check: the subjects `review` suspects, put to the
//! models the decision is bound to. A `fail` is reported with the check's own
//! message, evidence and identity; a `pass` is clean; anything else is left
//! to agent review.

use cel::Program;
use lighthouse_model::{Diagnostic, Position, Span, SymbolId, hash};
use lighthouse_plugin::{
    Ask, Ctx, DecisionRef, Judged, Neighbor, Open, ResultKind, RuleManifest, Shot, Subject,
    SubjectContext,
};
use lighthouse_spec::{CelCheck, Decision, ExampleKind, Select};
use serde_json::{Value, json};

use crate::{
    Error,
    eval::{Frame, compile, render, to_json},
};

/// The most characters of a subject's source put to a model.
const MAX_TEXT: usize = 6000;
/// The most characters of a neighbor's source.
const MAX_NEIGHBOR_TEXT: usize = 600;
/// The most callers and the most callees put to a model with a subject.
const MAX_NEIGHBORS: usize = 5;

/// What a check asks a model, compiled.
pub(crate) struct Review {
    program: Program,
    fact: Program,
    select: Select,
    decision: DecisionRef,
    upper: f64,
    revision: String,
}

/// A subject `review` picked, with the finding it becomes if a model says
/// `fail`.
pub(crate) struct Candidate {
    pub(crate) diagnostic: Diagnostic,
    /// What `review` gave besides `true`.
    pub(crate) kind: Option<String>,
    /// The facts of the subject, as the builder made them.
    pub(crate) fact: Value,
    /// `where` already made it a finding: the models are not asked, but a run
    /// that collects subjects for training keeps it.
    pub(crate) decided: bool,
}

impl Review {
    /// Compiles the `review` of `check`; `None` for a check without one.
    pub(crate) fn new(
        decision: &Decision,
        check: &CelCheck,
        select: Select,
    ) -> Result<Option<Self>, Error> {
        let Some(source) = &check.review else {
            return Ok(None);
        };
        let id = decision.id();
        let decision_ref = decision_ref(decision);
        let revision = serde_json::to_vec(&(decision.uid(), check, &decision_ref))
            .map(hash::sha256)
            .map_err(|e| Error::invalid(id, format!("the review cannot be hashed: {e}")))?;
        Ok(Some(Self {
            program: compile(id, "review", source)?,
            fact: compile(id, "review", select.variable())?,
            select,
            decision: decision_ref,
            upper: lighthouse_plugin::UPPER,
            revision,
        }))
    }

    /// Whether the subject in `frame` is to be asked about, and the kind
    /// `review` gave it, if any.
    pub(crate) fn asks(
        &self,
        frame: &Frame,
    ) -> Result<Option<Option<String>>, lighthouse_plugin::Error> {
        match frame.run(&self.program)? {
            cel::Value::Bool(true) => Ok(Some(None)),
            cel::Value::Bool(false) | cel::Value::Null => Ok(None),
            cel::Value::String(kind) => Ok(Some(Some(kind.to_string()))),
            other => Err(frame.fail(format!(
                "`review` is {}, not a bool or a kind",
                render(&other)
            ))),
        }
    }

    /// The facts of the subject in `frame`, as JSON.
    pub(crate) fn fact_of(&self, frame: &Frame) -> Result<Value, lighthouse_plugin::Error> {
        Ok(to_json(&frame.run(&self.fact)?))
    }

    /// Puts the candidates of the focused file to the models. A subject a
    /// model fails is a finding; one a model passes is clean; one nothing
    /// decided (undecided, unbound, over budget, a model's error) is a
    /// diagnostic marked `review`, which the engine reports apart.
    pub(crate) fn settle(
        &self,
        ctx: &Ctx,
        rule: &RuleManifest,
        candidates: Vec<Candidate>,
    ) -> Vec<Diagnostic> {
        let Some((_, text)) = ctx.file else {
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
        let judged: Vec<Judged> = if ctx.models.bound(&rule.id) {
            let subjects = candidates
                .iter()
                .map(|c| self.subject(ctx, text, c))
                .collect();
            ctx.models.classify(&Ask {
                rule: rule.id.clone(),
                revision: self.revision.clone(),
                upper: self.upper,
                decision: self.decision.clone(),
                subjects,
            })
        } else {
            candidates
                .iter()
                .map(|_| Judged::Open(Open::Unbound))
                .collect()
        };
        judged
            .into_iter()
            .zip(candidates)
            .filter_map(|(judged, candidate)| outcome(judged, candidate, &self.decision))
            .collect()
    }

    fn subject(&self, ctx: &Ctx, source: &str, candidate: &Candidate) -> Subject {
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
            language: ctx
                .project
                .file(&diagnostic.file)
                .map(|f| f.lang.clone())
                .unwrap_or_default(),
            file: diagnostic.file.to_string_lossy().replace('\\', "/"),
            text,
            context: SubjectContext {
                facts: candidate.fact.clone(),
                neighbors: symbol.map_or_else(Vec::new, |s| neighbors(ctx, source, s)),
            },
        }
    }
}

/// The decision as a model is told about it: its requirement, what the check
/// asks (the `prompt`, else the requirement) and its examples as shots.
pub fn decision_ref(decision: &Decision) -> DecisionRef {
    let prompt = decision.check.as_ref().and_then(|check| match &check.kind {
        lighthouse_spec::CheckKind::Cel(cel) => cel.prompt.clone(),
        lighthouse_spec::CheckKind::Model(model) => model.prompt.clone(),
        _ => None,
    });
    DecisionRef {
        id: decision.id().to_owned(),
        requirement: decision.requirement.clone(),
        prompt: prompt.unwrap_or_else(|| decision.requirement.clone()),
        examples: decision
            .examples
            .iter()
            .map(|example| Shot {
                kind: match example.kind {
                    ExampleKind::Invalid => ResultKind::Fail,
                    ExampleKind::Valid => ResultKind::Pass,
                },
                language: example.language.clone(),
                code: example
                    .files
                    .iter()
                    .map(|f| format!("// {}\n{}", f.path, f.text()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                note: example.name.clone(),
            })
            .collect(),
    }
}

/// What a model's judgment makes of a candidate: the finding it confirms,
/// with the model and its confidence in the evidence; a review task for a subject
/// left open; nothing for a `pass`.
fn outcome(judged: Judged, candidate: Candidate, decision: &DecisionRef) -> Option<Diagnostic> {
    let mut diagnostic = candidate.diagnostic;
    let mut evidence = match std::mem::take(&mut diagnostic.evidence) {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    match judged {
        Judged::Decided(decided) if decided.kind == ResultKind::Pass => return None,
        Judged::Decided(decided) => {
            if diagnostic.message.is_empty() {
                diagnostic.message.clone_from(&decision.requirement);
            }
            evidence.insert("source".to_owned(), json!("model"));
            if let Some(kind) = candidate.kind {
                evidence.insert("kind".to_owned(), json!(kind));
            }
            evidence.insert("model".to_owned(), json!(decided.model));
            evidence.insert("version".to_owned(), json!(decided.version));
            evidence.insert("confidence".to_owned(), json!(decided.confidence));
        }
        Judged::Open(open) => {
            if diagnostic.message.is_empty() {
                diagnostic.message.clone_from(&decision.requirement);
            }
            diagnostic.review = Some(open.to_string());
        }
    }
    diagnostic.evidence = Value::Object(evidence);
    Some(diagnostic)
}

/// The callers and callees of a symbol, with the source of those in the same
/// file.
fn neighbors(ctx: &Ctx, source: &str, symbol: &lighthouse_model::Symbol) -> Vec<Neighbor> {
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
