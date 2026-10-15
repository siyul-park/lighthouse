//! `lighthouse eval`: runs the checks of a decision, and of a candidate beside
//! it, on the repositories its labels come from, and hands what they found to
//! the host's evaluation (`lighthouse_ml::evaluate`).

use std::{collections::BTreeSet, env, path::Path};

use lighthouse_engine::ModelOptions;
use lighthouse_ml::{
    evaluate::{
        Candidate, EvalReport, EvalRequest, Found, Gathered, Part, Repo, Setup, grade, repos,
    },
    labels::{Baseline, Collect, Label, Source},
};
use lighthouse_model::Severity;

use crate::{CheckRequest, ModelFlags, Result, Session, check, judged::judged, samples};

/// What one repository is evaluated with.
struct Inputs<'a> {
    config: Option<&'a Path>,
    baseline: Option<&'a Path>,
    candidate: Option<&'a Path>,
    /// Whether the examples of the decision are subjects here (once per run).
    examples: bool,
}

/// Runs the evaluation.
pub fn eval(request: &EvalRequest) -> Result<EvalReport> {
    let start = env::current_dir()?;
    let config = request.config.as_ref().map(|c| start.join(c));
    let baseline = request.baseline.as_ref().map(|b| start.join(b));
    let candidate = request.candidate.as_ref().map(|c| start.join(c));
    let mut gathered = Gathered::default();
    let outcome = (|| -> Result<_> {
        for (at, repo) in repos(&request.repos, &start).iter().enumerate() {
            env::set_current_dir(&repo.dir)?;
            gathered.add(one(
                request,
                repo,
                &Inputs {
                    config: config.as_deref(),
                    baseline: baseline.as_deref(),
                    candidate: candidate.as_deref(),
                    // The examples are the decision's, not the repository's.
                    examples: at == 0,
                },
            )?);
        }
        Ok(())
    })();
    env::set_current_dir(&start)?;
    outcome?;
    gathered.finish(request, candidate.is_some())
}

fn one(request: &EvalRequest, repo: &Repo, inputs: &Inputs) -> Result<Part> {
    let session = Session::load(inputs.config)?;
    let catalog = session.catalog()?;
    let decision = catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?
        .clone();
    let policy = session.config.models().policy(&request.decision);
    let table = inputs.baseline.map(|file| Baseline {
        file,
        repo: repo.name.as_deref(),
    });
    let models = ModelOptions {
        all: true,
        ..ModelOptions::default()
    };
    let examples = if inputs.examples {
        samples::shown(&session, &catalog, &decision, &request.decision, &models)?
    } else {
        Vec::new()
    };
    // Only labels a person gave, a reviewed table holds or an example shows
    // grade a check; weak labels (an agent's, a model's) never do.
    let labels: Vec<Label> = Collect {
        judgments: judged(&session.root)?,
        decision: &decision,
        baseline: table.as_ref(),
        exclude_model: None,
        examples,
    }
    .labels()?
    .into_iter()
    .filter(|l| l.weight >= Source::Person.weight())
    .collect();
    // Models are asked about the labelled subjects only; a label with an
    // observed outcome (an example) is graded without a run.
    let asked = labels
        .iter()
        .filter(|l| l.observed.is_none())
        .map(|l| l.key.clone())
        .collect();
    let current = run(request, inputs.config, &request.decision, None, Some(asked))?;
    let Some(file) = inputs.candidate else {
        return Ok(grade(request, repo, policy, &labels, &current, None));
    };
    let (id, setup) = Setup::read(&session.root, file)?;
    // The candidate's fingerprints are not known before it runs, so it asks
    // models about everything it selects.
    let found = run(request, inputs.config, &id, Some(&setup), None)?;
    let beside = candidate_session(inputs.config, &setup, &id)?;
    let shown = if inputs.examples {
        samples::shown(&beside, &beside.catalog()?, &decision, &id, &models)?
    } else {
        Vec::new()
    };
    let candidate = Candidate {
        id,
        found: &found,
        shown,
    };
    Ok(grade(
        request,
        repo,
        policy,
        &labels,
        &current,
        Some(candidate),
    ))
}

/// The project with the candidate's decisions on top and its rule enabled.
fn candidate_session(config: Option<&Path>, setup: &Setup, rule: &str) -> Result<Session> {
    let mut session = Session::load(config)?;
    session = session.with_local(Some(setup.catalog()?));
    session.config = session.config.with_rule(rule, Severity::Warn);
    Ok(session)
}

fn run(
    request: &EvalRequest,
    config: Option<&Path>,
    rule: &str,
    setup: Option<&Setup>,
    subjects: Option<BTreeSet<String>>,
) -> Result<Found> {
    let session = match setup {
        Some(setup) => candidate_session(config, setup, rule)?,
        None => Session::load(config)?,
    };
    let checked = check(
        session,
        &CheckRequest {
            rules: vec![rule.to_owned()],
            store: false,
            cache: false,
            models: ModelFlags {
                all: true,
                subjects,
                collect: request.audit.is_some(),
                ..ModelFlags::default()
            },
            ..CheckRequest::default()
        },
    )?;
    Ok(Found::of(&checked.outcome))
}
