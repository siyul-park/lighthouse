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

use lighthouse_model::Severity;
use lighthouse_plugin::ResultKind;
use lighthouse_spec::{Catalog, Evaluation, local_files};
use serde::Serialize;

use crate::{
    CheckRequest, ModelFlags, Result, Session, check,
    labels::{Baseline, Label, collect},
    metrics::{Metrics, Outcome, Rate, Truth, agreement, evaluate},
};

const CAVEAT: &str = "labelled positives come mostly from flagged findings, so recall is an upper bound; sample unflagged subjects for a person to label";

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
    /// Leave models out of both checks.
    pub no_models: bool,
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
    /// Whether the check (the candidate, when there is one) reaches the
    /// precision target by the lower bound of its interval.
    pub meets_precision: Option<bool>,
    pub caveat: &'static str,
}

/// What one run of a check found, by subject.
#[derive(Default)]
struct Found {
    by_fingerprint: BTreeMap<String, Outcome>,
    /// The subject a fingerprint is about: its file and symbol.
    place: BTreeMap<String, String>,
    by_place: BTreeMap<String, Outcome>,
    model_decided: usize,
    reviews: usize,
}

/// What one repository gave.
struct Part {
    policy: Evaluation,
    current: Vec<(Truth, Outcome)>,
    candidate: Vec<(Truth, Outcome)>,
    pairs: Vec<(Outcome, Outcome)>,
    decided: (usize, usize),
    candidate_decided: (usize, usize),
    check: String,
    candidate_check: Option<String>,
}

#[derive(Default)]
struct Gathered {
    parts: Vec<Part>,
}

impl Gathered {
    fn add(&mut self, part: Part) {
        self.parts.push(part);
    }

    fn finish(self, request: &EvalRequest, with_candidate: bool) -> Result<EvalReport> {
        let first = self.parts.first().ok_or("nothing to evaluate")?;
        let policy = first.policy;
        let pool = |pick: fn(&Part) -> &Vec<(Truth, Outcome)>| -> Vec<(Truth, Outcome)> {
            self.parts.iter().flat_map(|p| pick(p).clone()).collect()
        };
        let current = pool(|p| &p.current);
        let side = |check: String, pairs: &[(Truth, Outcome)], decided: (usize, usize)| Side {
            check,
            metrics: evaluate(pairs),
            model_decided: decided.0,
            reviews: decided.1,
        };
        let sum = |pick: fn(&Part) -> (usize, usize)| {
            self.parts
                .iter()
                .fold((0, 0), |a, p| (a.0 + pick(p).0, a.1 + pick(p).1))
        };
        let current_side = side(first.check.clone(), &current, sum(|p| p.decided));
        let candidate_side = with_candidate.then(|| {
            let pairs = pool(|p| &p.candidate);
            side(
                first.candidate_check.clone().unwrap_or_default(),
                &pairs,
                sum(|p| p.candidate_decided),
            )
        });
        let judged = candidate_side.as_ref().unwrap_or(&current_side);
        let meets_precision = judged
            .metrics
            .precision
            .map(|rate| rate.low >= policy.precision);
        let pairs: Vec<(Outcome, Outcome)> =
            self.parts.iter().flat_map(|p| p.pairs.clone()).collect();
        Ok(EvalReport {
            decision: request.decision.clone(),
            labelled: current.len(),
            policy,
            agreement: with_candidate.then(|| agreement(&pairs)).flatten(),
            current: current_side,
            candidate: candidate_side,
            meets_precision,
            caveat: CAVEAT,
        })
    }
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
        for repo in &repos {
            env::set_current_dir(&repo.dir)?;
            gathered.add(one(
                request,
                repo,
                config.as_deref(),
                baseline.as_deref(),
                candidate.as_deref(),
            )?);
        }
        Ok(())
    })();
    env::set_current_dir(&start)?;
    outcome?;
    gathered.finish(request, candidate.is_some())
}

fn one(
    request: &EvalRequest,
    repo: &Repo,
    config: Option<&Path>,
    baseline: Option<&Path>,
    candidate: Option<&Path>,
) -> Result<Part> {
    let session = Session::load(config)?;
    let root = session.root.clone();
    let catalog = session.catalog()?;
    let decision = catalog
        .decision(&request.decision)
        .ok_or_else(|| format!("unknown decision `{}`", request.decision))?
        .clone();
    let policy = session.config.models().policy(&request.decision);
    let table = baseline.map(|file| Baseline {
        file,
        repo: repo.name.as_deref(),
    });
    let labels = collect(&root, &decision, table.as_ref(), None)?;
    let current = run(request, config, &request.decision, None)?;
    let truth = |l: &Label| match l.kind {
        ResultKind::Fail => Truth::Fail,
        _ => Truth::Pass,
    };
    let mut part = Part {
        policy,
        current: labels
            .iter()
            .map(|l| {
                let outcome = current.by_fingerprint.get(&l.fingerprint);
                (truth(l), outcome.copied().unwrap_or(Outcome::Unselected))
            })
            .collect(),
        candidate: Vec::new(),
        pairs: Vec::new(),
        decided: (current.model_decided, current.reviews),
        candidate_decided: (0, 0),
        check: request.decision.clone(),
        candidate_check: None,
    };
    if let Some(file) = candidate {
        let (id, setup) = Setup::read(&root, file)?;
        let found = run(request, config, &id, Some(&setup))?;
        for label in &labels {
            let was = current
                .by_fingerprint
                .get(&label.fingerprint)
                .copied()
                .unwrap_or(Outcome::Unselected);
            let now = current
                .place
                .get(&label.fingerprint)
                .and_then(|place| found.by_place.get(place))
                .copied()
                .unwrap_or(Outcome::Unselected);
            part.candidate.push((truth(label), now));
            part.pairs.push((was, now));
        }
        part.candidate_decided = (found.model_decided, found.reviews);
        part.candidate_check = Some(id);
    }
    Ok(part)
}

fn run(
    request: &EvalRequest,
    config: Option<&Path>,
    rule: &str,
    setup: Option<&Setup>,
) -> Result<Found> {
    let mut session = Session::load(config)?;
    if let Some(setup) = setup {
        let local = Catalog::from_local(setup.files.clone())?;
        session = session.with_local(Some(local));
        session.config = session.config.with_rule(rule, Severity::Warn);
    }
    let checked = check(
        session,
        &CheckRequest {
            rules: vec![rule.to_owned()],
            store: false,
            cache: false,
            models: ModelFlags {
                off: request.no_models,
                all: true,
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
        } else if d.evidence.get("source").and_then(|v| v.as_str()) == Some("model") {
            found.model_decided += 1;
        }
        found.by_fingerprint.insert(fp.clone(), result);
        found.by_place.insert(place.clone(), result);
        found.place.insert(fp, place);
    }
    Ok(found)
}
