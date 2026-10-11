//! `lighthouse eval`: how a decision's check does on the labels recorded for
//! it, and how a candidate check does beside it.
//!
//! The labelled subjects are found again by running the checks on the
//! repositories the labels come from. A subject a check did not report is
//! taken as one it did not select (or let pass); the labels come mostly from
//! flagged findings, so recall is a bound, not a measurement: sample unflagged
//! subjects for a person to label to measure it.

use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
};

use lighthouse_engine::ModelOptions;
use lighthouse_model::Severity;
use lighthouse_spec::{Catalog, Evaluation, local_files};
use serde::Serialize;

use crate::{
    CheckRequest, ModelFlags, Result, Session,
    assemble::{Gathered, Part, Usage},
    check,
    labels::{Baseline, Collect, Label, Source},
    metrics::{Metrics, Outcome, Rate},
    samples,
};

pub(crate) const CAVEAT: &str = "labelled positives come mostly from flagged findings, so recall is an upper bound; sample unflagged subjects for a person to label";

/// A repository whose findings the labels are about.
pub struct Repo {
    /// The name its baseline rows use (the `repo` column).
    pub name: Option<String>,
    pub dir: PathBuf,
}

/// What to evaluate.
pub struct EvalRequest {
    pub decision: String,
    /// Where to run; the current project when empty.
    pub repos: Vec<Repo>,
    /// The baseline table of reviewed findings, when there is one.
    pub baseline: Option<PathBuf>,
    /// A decision file whose check is evaluated beside the current one.
    pub candidate: Option<PathBuf>,
    pub config: Option<PathBuf>,
    /// Write this many selected subjects the check did not flag to a table
    /// a person labels, to measure recall.
    pub audit: Option<usize>,
}

/// One check's numbers.
#[derive(Debug, Serialize)]
pub struct Side {
    pub check: String,
    pub metrics: Metrics,
    /// Subjects a model decided.
    pub model_decided: usize,
    /// Subjects left to review.
    pub reviews: usize,
    /// Questions put to models, and what they cost.
    pub calls: usize,
    pub cost: f64,
}

/// The result of an evaluation.
#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub decision: String,
    pub labelled: usize,
    pub policy: Evaluation,
    pub current: Side,
    pub candidate: Option<Side>,
    /// How often the candidate answers as the current check does.
    pub agreement: Option<Rate>,
    /// Whether the check (the candidate, when there is one) meets the
    /// evaluation policy by the point estimates of its precision and, when the
    /// policy sets a target, of its recall; the intervals are shown beside.
    pub meets_policy: Option<bool>,
    pub caveat: &'static str,
    /// The table of unflagged subjects written for a person to label.
    pub audit: Option<PathBuf>,
}

/// What one run of a check found, by subject.
#[derive(Default)]
pub(crate) struct Found {
    pub(crate) by_fingerprint: BTreeMap<String, Outcome>,
    /// The subject a fingerprint is about: its file and symbol.
    pub(crate) place: BTreeMap<String, String>,
    pub(crate) by_place: BTreeMap<String, Outcome>,
    pub(crate) model_decided: usize,
    pub(crate) reviews: usize,
    pub(crate) calls: usize,
    pub(crate) cost: f64,
    /// Selected subjects the check did not flag, for an audit, with the
    /// place each is at.
    pub(crate) unflagged: Vec<(String, String)>,
}

/// A candidate decision to run beside the project's.
struct Setup {
    files: BTreeMap<String, String>,
}

impl Setup {
    /// The project's own decisions with the candidate file added; the id of
    /// the candidate decision.
    fn read(root: &Path, file: &Path) -> Result<(String, Self)> {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let name = file.file_name().map_or_else(
            || "candidate.yaml".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let mut files = local_files(root)?.unwrap_or_default();
        files.insert(name.clone(), text.clone());
        let alone = BTreeMap::from([(name, text)]);
        let catalog = Catalog::from_local(alone)?;
        let id = catalog
            .decisions()
            .next()
            .map(|d| d.id().to_owned())
            .ok_or("the candidate file holds no decision")?;
        Ok((id, Self { files }))
    }
}

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
    let repos: Vec<Repo> = if request.repos.is_empty() {
        vec![Repo {
            name: None,
            dir: start.clone(),
        }]
    } else {
        request
            .repos
            .iter()
            .map(|r| Repo {
                name: r.name.clone(),
                dir: start.join(&r.dir),
            })
            .collect()
    };
    let baseline = request.baseline.as_ref().map(|b| start.join(b));
    let candidate = request.candidate.as_ref().map(|c| start.join(c));
    let mut gathered = Gathered::default();
    let outcome = (|| -> Result<_> {
        for (at, repo) in repos.iter().enumerate() {
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

/// Writes the audit table, when one was asked for and there is something to
/// label: a person fills in the `verdict` column (`TP` or `FP`).
pub(crate) fn write_audit(request: &EvalRequest, rows: Vec<String>) -> Result<Option<PathBuf>> {
    if request.audit.is_none() || rows.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(format!("{}.audit.tsv", request.decision.replace('/', "-")));
    let mut text = String::from("fingerprint\trepo\tlocation\tverdict\treason\n");
    for row in rows {
        text.push_str(&row);
        text.push('\n');
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(path))
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
    // Only labels a person gave, a reviewed table holds or an example shows
    // grade a check; weak labels (an agent's, a model's) never do.
    let labels: Vec<Label> = Collect {
        session: &session,
        catalog: &catalog,
        decision: &decision,
        rule: &request.decision,
        baseline: table.as_ref(),
        exclude_model: None,
        models: models.clone(),
        examples: inputs.examples,
    }
    .labels()?
    .into_iter()
    .filter(|l| l.weight >= Source::Person.weight())
    .collect();
    let current = run(request, inputs.config, &request.decision, None)?;
    let outcome_of = |found: &Found, l: &Label| {
        l.observed.unwrap_or_else(|| {
            found
                .by_fingerprint
                .get(&l.key)
                .copied()
                .unwrap_or(Outcome::Unselected)
        })
    };
    let mut part = Part {
        policy,
        current: labels
            .iter()
            .map(|l| (l.truth, outcome_of(&current, l)))
            .collect(),
        candidate: Vec::new(),
        pairs: Vec::new(),
        decided: Usage::of(&current),
        candidate_decided: Usage::default(),
        audit: Vec::new(),
        check: request.decision.clone(),
        candidate_check: None,
    };
    if let Some(file) = inputs.candidate {
        let (id, setup) = Setup::read(&session.root, file)?;
        let found = run(request, inputs.config, &id, Some(&setup))?;
        let beside = candidate_session(inputs.config, &setup, &id)?;
        let shown: BTreeMap<String, Outcome> = if inputs.examples {
            samples::shown(&beside, &beside.catalog()?, &decision, &id, &models)?
                .into_iter()
                .map(|s| (s.key, s.outcome))
                .collect()
        } else {
            BTreeMap::new()
        };
        for label in &labels {
            let was = outcome_of(&current, label);
            let now = if label.observed.is_some() {
                shown
                    .get(&label.key)
                    .copied()
                    .unwrap_or(Outcome::Unselected)
            } else {
                current
                    .place
                    .get(&label.key)
                    .and_then(|place| found.by_place.get(place))
                    .copied()
                    .unwrap_or(Outcome::Unselected)
            };
            part.candidate.push((label.truth, now));
            part.pairs.push((was, now));
        }
        part.candidate_decided = Usage::of(&found);
        part.audit = audit_rows(request, repo, &found);
        part.candidate_check = Some(id);
    }
    if inputs.candidate.is_none() {
        part.audit = audit_rows(request, repo, &current);
    }
    Ok(part)
}

/// The project with the candidate's decisions on top and its rule enabled.
fn candidate_session(config: Option<&Path>, setup: &Setup, rule: &str) -> Result<Session> {
    let mut session = Session::load(config)?;
    let local = Catalog::from_local(setup.files.clone())?;
    session = session.with_local(Some(local));
    session.config = session.config.with_rule(rule, Severity::Warn);
    Ok(session)
}

/// The rows of an audit table for a repository: unflagged selected subjects,
/// a deterministic sample.
fn audit_rows(request: &EvalRequest, repo: &Repo, found: &Found) -> Vec<String> {
    let Some(n) = request.audit else {
        return Vec::new();
    };
    let mut rows: Vec<&(String, String)> = found.unflagged.iter().collect();
    rows.sort_by_key(|(fingerprint, _)| lighthouse_model::hash::short(fingerprint, 8));
    rows.into_iter()
        .take(n)
        .map(|(fingerprint, place)| {
            format!(
                "{fingerprint}\t{}\t{place}\t\t",
                repo.name.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

fn run(
    request: &EvalRequest,
    config: Option<&Path>,
    rule: &str,
    setup: Option<&Setup>,
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
                collect: request.audit.is_some(),
                ..ModelFlags::default()
            },
            ..CheckRequest::default()
        },
    )?;
    let mut found = Found::default();
    let outcome = &checked.outcome;
    let each = outcome
        .diagnostics
        .iter()
        .map(|d| (d, Outcome::Fail))
        .chain(outcome.reviews.iter().map(|d| (d, Outcome::Review)));
    for (d, result) in each {
        let place = format!(
            "{}|{}",
            d.file.display(),
            d.symbol.as_deref().unwrap_or(d.fingerprint.as_str())
        );
        let fp = d.fingerprint.as_str().to_owned();
        if result == Outcome::Review {
            found.reviews += 1;
        } else if d.evidence.get("model").is_some() {
            found.model_decided += 1;
        }
        found.by_fingerprint.insert(fp.clone(), result);
        found.by_place.insert(place.clone(), result);
        found.place.insert(fp, place);
    }
    found.calls = outcome.model_calls;
    found.cost = outcome.model_cost;
    let flagged: std::collections::BTreeSet<&String> = found.by_fingerprint.keys().collect();
    found.unflagged = outcome
        .subjects
        .iter()
        .filter(|(fingerprint, _)| !flagged.contains(fingerprint))
        .map(|(fingerprint, subject)| {
            (
                fingerprint.clone(),
                format!("{} {}", subject.file, subject.id),
            )
        })
        .collect();
    Ok(found)
}
