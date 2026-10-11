//! The subjects the examples of a decision show: each example is written to a
//! directory and checked the way `lighthouse decision test` checks it, with
//! the project's own configuration, so a rule bound to a model asks it. An
//! invalid example is `fail` where it says it expects a diagnostic; a valid
//! one is `pass` wherever the check looked.

use lighthouse_engine::{ExampleRun, ModelOptions, Outcome as Run};
use lighthouse_ml::{
    labels::Shown,
    metrics::{Outcome, Truth},
};
use lighthouse_model::Diagnostic;
use lighthouse_spec::{Catalog, Decision, Example, ExampleKind};

use crate::{Result, Session, check::attached_models};

/// What the examples of a decision show, checked by `rule` (the decision's own
/// rule, or a candidate's) under `models`.
pub(crate) fn shown(
    session: &Session,
    catalog: &Catalog,
    decision: &Decision,
    rule: &str,
    models: &ModelOptions,
) -> Result<Vec<Shown>> {
    let mut all = Vec::new();
    for example in &decision.examples {
        let (registry, _) = session.registry()?;
        let run = ExampleRun {
            registry,
            config: session.config.clone(),
            catalog,
            trusted: session.trusted(),
            models: ModelOptions {
                attached: attached_models(&session.root, catalog),
                collect: true,
                ..models.clone()
            },
        }
        .outcome(rule, example)
        .map_err(|e| format!("{} {}: {e}", decision.id(), example.name))?;
        all.extend(of_example(decision.id(), example, &run));
    }
    Ok(all)
}

fn of_example(decision: &str, example: &Example, run: &Run) -> Vec<Shown> {
    let prefix = format!("example:{decision}:{}", example.name);
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
                Shown {
                    key: format!("{prefix}:{}", expect.line),
                    truth: Truth::Fail,
                    outcome: hit.map_or(Outcome::Unselected, |(outcome, _)| outcome),
                    subject: hit
                        .and_then(|(_, d)| run.subjects.get(d.fingerprint.as_str()).cloned()),
                }
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
            let asked = run.subjects.iter().map(|(fingerprint, subject)| Shown {
                key: format!("{prefix}:{fingerprint}"),
                truth: Truth::Pass,
                outcome: state(fingerprint),
                subject: Some(subject.clone()),
            });
            let flagged = run
                .diagnostics
                .iter()
                .filter(|d| !run.subjects.contains_key(d.fingerprint.as_str()))
                .map(|d| Shown {
                    key: format!("{prefix}:{}", d.fingerprint.as_str()),
                    truth: Truth::Pass,
                    outcome: Outcome::Fail,
                    subject: None,
                });
            asked.chain(flagged).collect()
        }
    }
}
