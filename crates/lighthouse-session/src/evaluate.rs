//! `lighthouse eval`: runs the checks of a decision, and of a candidate beside
//! it, on the repositories its labels come from, and hands what they found to
//! the host's evaluation (`lighthouse_ml::evaluate`). The model a revision
//! attached, and the one `--model` names, are layers laid over the run of the
//! check without models attached.

use std::{collections::BTreeSet, env, path::Path};

use lighthouse_engine::ModelOptions;
use lighthouse_ml::{
    evaluate::{
        Candidate, EvalReport, EvalRequest, Found, Gathered, Layer, Part, Repo, Setup, grade,
        overlay, repos, shown,
    },
    labels::{Baseline, Collect, Label, Source},
};
use lighthouse_model::Severity;
use lighthouse_spec::Decision;

use crate::{
    CheckRequest, Checked, ModelFlags, Result, Session, attached_models, check, judged::judged,
    samples,
};

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
    if request.model.is_some() && request.candidate.is_some() {
        return Err("--model and --candidate are exclusive".into());
    }
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
    gathered.finish(request, candidate.is_some() || request.model.is_some())
}

fn one(request: &EvalRequest, repo: &Repo, inputs: &Inputs) -> Result<Part> {
    let session = Session::load(inputs.config)?;
    let catalog = session.catalog()?;
    let decision = catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?
        .clone();
    let policy = session
        .config
        .models(catalog.projects())?
        .policy(&request.decision);
    let table = inputs.baseline.map(|file| Baseline {
        file,
        repo: repo.name.as_deref(),
    });
    // The base run leaves the layer a revision attaches out: it is laid over
    // the run afterwards, by the same code that grades a model not attached.
    let models = ModelOptions {
        all: true,
        ..ModelOptions::default()
    };
    let labels = labels_of(
        &session,
        &decision,
        table.as_ref(),
        inputs.examples,
        &models,
    )?;
    let attached = attached_models(&session.root, &catalog).remove(&request.decision);
    let layered = attached.is_some() || request.model.is_some();
    let (checked, base) = run(
        request,
        inputs.config,
        &request.decision,
        None,
        Some(asked_about(&labels)),
        layered,
    )?;
    let reference = decision.reference();
    let models_of = |model: &str| checked.model(model);
    let layer = |model| Layer {
        model,
        decision: &reference,
        root: &session.root,
        models: &models_of,
    };
    let current = attached
        .as_deref()
        .map(|model| overlay(&layer(model), &base, &labels))
        .transpose()?;
    let (current, labels) = current
        .as_ref()
        .map_or((&base, &labels), |l| (&l.found, &l.labels));
    if let Some(model) = request.model.as_deref() {
        let beside = overlay(&layer(model), &base, labels)?;
        let candidate = Candidate {
            id: request.decision.clone(),
            found: &beside.found,
            shown: shown(&beside.labels),
        };
        return Ok(grade(
            request,
            repo,
            policy,
            labels,
            current,
            Some(candidate),
        ));
    }
    let Some(file) = inputs.candidate else {
        return Ok(grade(request, repo, policy, labels, current, None));
    };
    let (id, setup) = Setup::read(&session.root, file)?;
    // The candidate's fingerprints are not known before it runs, so it asks
    // models about everything it selects.
    let (_, found) = run(request, inputs.config, &id, Some(&setup), None, false)?;
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
        labels,
        current,
        Some(candidate),
    ))
}

/// The labels that grade the check of `decision`. Only those a person gave, a
/// reviewed table holds or an example shows do; weak labels (an agent's, a
/// model's) never do. The examples are the decision's, not the repository's:
/// they are subjects once per run.
fn labels_of(
    session: &Session,
    decision: &Decision,
    table: Option<&Baseline>,
    examples: bool,
    models: &ModelOptions,
) -> Result<Vec<Label>> {
    let examples = if examples {
        samples::shown(
            session,
            &session.catalog()?,
            decision,
            decision.id(),
            models,
        )?
    } else {
        Vec::new()
    };
    Ok(Collect {
        judgments: judged(&session.root)?,
        decision,
        baseline: table,
        exclude_model: None,
        examples,
    }
    .labels()?
    .into_iter()
    .filter(|l| l.weight >= Source::Person.weight())
    .collect())
}

/// The subjects models are asked about: the labelled ones only. A label with
/// an observed outcome (an example) is graded without a run.
fn asked_about(labels: &[Label]) -> BTreeSet<String> {
    labels
        .iter()
        .filter(|l| l.observed.is_none())
        .map(|l| l.key.clone())
        .collect()
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
    layered: bool,
) -> Result<(Checked, Found)> {
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
                collect: layered || request.audit.is_some(),
                detached: true,
                ..ModelFlags::default()
            },
            ..CheckRequest::default()
        },
    )?;
    let found = Found::of(&checked.outcome);
    Ok((checked, found))
}
