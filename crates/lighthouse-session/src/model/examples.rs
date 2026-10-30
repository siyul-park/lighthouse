//! The subjects the examples of a decision show: each example is written to a
//! directory and checked the way `lighthouse decision test` checks it, with
//! the project's own configuration, so a rule bound to a model asks it. An
//! invalid example is `fail` where it says it expects a diagnostic; a valid
//! one is `pass` wherever the check looked.

use lighthouse_engine::{ExampleRun, ModelOptions, Run};
use lighthouse_ml::{labels::Label, metrics::Outcome};
use lighthouse_model::{Diagnostic, Judgment};
use lighthouse_spec::{Decision, Example, ExampleKind};

use super::attached;
use crate::{Result, Session};

/// What the examples of a decision show, checked by `rule` (the decision's own
/// rule, or a candidate's) under `models`.
pub(super) fn labels(
    session: &Session,
    decision: &Decision,
    rule: &str,
    models: &ModelOptions,
) -> Result<Vec<Label>> {
    let models = ModelOptions {
        attached: attached(&session.root, &session.catalog),
        collect: true,
        ..models.clone()
    };
    let mut all = Vec::new();
    for example in &decision.examples {
        let (registry, _) = session.registry()?;
        let config = session.enabled_in(&registry);
        let run = ExampleRun {
            registry,
            config,
            catalog: &session.catalog,
            trusted: session.trusted(),
            models: models.clone(),
        }
        .run(rule, example)
        .map_err(|e| format!("{} {}: {e}", decision.id(), example.name))?;
        all.extend(of_example(decision.id(), example, &run));
    }
    Ok(all)
}

fn of_example(decision: &str, example: &Example, run: &Run) -> Vec<Label> {
    let prefix = format!("example:{decision}:{}", example.name);
    let label =
        |key, judgment, outcome, subject| Label::example(decision, key, judgment, outcome, subject);
    match example.kind {
        ExampleKind::Invalid => example
            .expect
            .iter()
            .map(|expect| {
                let at = |d: &&Diagnostic| d.span.start.line == expect.line;
                let hit = run
                    .diagnostics
                    .iter()
                    .find(at)
                    .map(|d| (Outcome::Fail, d))
                    .or_else(|| run.reviews.iter().find(at).map(|d| (Outcome::Review, d)));
                label(
                    format!("{prefix}:{}", expect.line),
                    Judgment::Fail,
                    hit.map_or(Outcome::NotApplicable, |(outcome, _)| outcome),
                    hit.and_then(|(_, d)| run.questions.get(d.fingerprint.as_str()).cloned()),
                )
            })
            .collect(),
        ExampleKind::Valid => {
            let state = |fingerprint: &str| {
                let is = |d: &Diagnostic| d.fingerprint.as_str() == fingerprint;
                if run.diagnostics.iter().any(is) {
                    Outcome::Fail
                } else if run.reviews.iter().any(is) {
                    Outcome::Review
                } else {
                    Outcome::Pass
                }
            };
            let asked = run.questions.iter().map(|(fingerprint, subject)| {
                label(
                    format!("{prefix}:{fingerprint}"),
                    Judgment::Pass,
                    state(fingerprint),
                    Some(subject.clone()),
                )
            });
            let flagged = run
                .diagnostics
                .iter()
                .filter(|d| !run.questions.contains_key(d.fingerprint.as_str()))
                .map(|d| {
                    label(
                        format!("{prefix}:{}", d.fingerprint.as_str()),
                        Judgment::Pass,
                        Outcome::Fail,
                        None,
                    )
                });
            asked.chain(flagged).collect()
        }
    }
}
