//! `lighthouse eval`: how a decision's check does on the labels recorded for
//! it, and how a candidate check does beside it.
//!
//! The labelled subjects are found again by running the checks on the
//! repositories the labels come from. A subject a check did not report is
//! taken as one it did not select (or let pass); the labels come mostly from
//! flagged findings, so recall is a bound, not a measurement: sample unflagged
//! subjects for a person to label to measure it.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use lighthouse_engine::Outcome as Run;
use lighthouse_plugin::Subject;
use lighthouse_spec::{Catalog, Evaluation, local_files};
use serde::Serialize;

use crate::{
    Result,
    labels::{Label, Shown},
    metrics::{Metrics, Outcome, Rate, Truth},
};

mod assemble;
mod layer;

pub(crate) use assemble::shortfall;
pub use assemble::{Gathered, Part};
pub(crate) use layer::trained_on;
pub use layer::{Layer, LayerInfo, Layered, overlay, shown};

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
    /// A model (`<provider>/<model>@sha256:<hex>`) to grade as the first
    /// layer of the decision, beside the current check.
    pub model: Option<String>,
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
    /// The probability each labelled subject was given, by repository and
    /// fingerprint: the raw answers the metrics are computed from, so that
    /// reports can be pooled.
    pub scores: Vec<Score>,
    /// What went wrong with the models, or was worth saying: the notices of
    /// the run, so that a failing model is not silent.
    pub notices: Vec<String>,
    /// The model attached by revision this side is graded with, and how.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layer: Option<LayerInfo>,
}

/// What a model gave a labelled subject.
#[derive(Debug, Clone, Serialize)]
pub struct Score {
    /// The label's fingerprint.
    pub fingerprint: String,
    pub repo: Option<String>,
    /// Whether the label says the subject violates the requirement.
    pub violates: bool,
    /// P(fail) the model gave.
    pub probability: f64,
    /// The model that gave it: `<reference>@<version>`.
    pub model: String,
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
    /// Labelled subjects the candidate decides that the current check leaves
    /// for review.
    pub decided: usize,
    /// The examples that should pass and the candidate newly fails.
    pub regressions: Vec<String>,
    pub caveat: &'static str,
    /// The table of unflagged subjects written for a person to label.
    pub audit: Option<PathBuf>,
    /// What to do about an attached model that misses the policy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
}

/// A change the monitor proposes; nothing is written until it is made.
#[derive(Debug, Serialize)]
pub struct Proposal {
    /// `detach`.
    pub action: &'static str,
    pub model: String,
    pub reason: String,
}

/// What one run of a check found, by subject.
#[derive(Default, Clone)]
pub struct Found {
    by_fingerprint: BTreeMap<String, Outcome>,
    /// The subject a fingerprint is about: its file and symbol.
    place: BTreeMap<String, String>,
    by_place: BTreeMap<String, Outcome>,
    model_decided: usize,
    reviews: usize,
    calls: usize,
    cost: f64,
    /// The probability of failing models gave, by place, with the model.
    probabilities: BTreeMap<String, (f64, String)>,
    /// The notices of the run.
    notices: Vec<String>,
    /// Selected subjects the check did not flag, for an audit, with the
    /// place each is at.
    unflagged: Vec<(String, String)>,
    /// With subjects collected, what models are asked about each, by
    /// fingerprint.
    subjects: BTreeMap<String, Subject>,
    /// The layer of an attached model laid over the run, when one was.
    layer: Option<LayerInfo>,
}

impl Found {
    /// What a run found: the diagnostics and reviews by fingerprint and by
    /// place, what models decided, and the selected subjects left unflagged.
    pub fn of(run: &Run) -> Self {
        let mut found = Self::default();
        let each = run
            .diagnostics
            .iter()
            .map(|d| (d, Outcome::Fail))
            .chain(run.reviews.iter().map(|d| (d, Outcome::Review)));
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
        found.probabilities = run
            .probabilities
            .iter()
            .filter_map(|(fp, answer)| Some((found.place.get(fp)?.clone(), answer.clone())))
            .collect();
        found.notices = run.notices.iter().cloned().collect();
        found.subjects.clone_from(&run.subjects);
        found.calls = run.model_calls;
        found.cost = run.model_cost;
        let flagged: BTreeSet<&String> = found.by_fingerprint.keys().collect();
        found.unflagged = run
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
        found
    }

    /// With subjects collected, what models are asked about each, by
    /// fingerprint.
    pub fn subjects(&self) -> &BTreeMap<String, Subject> {
        &self.subjects
    }

    /// The layer laid over the run, when there is one.
    pub fn layer(&self) -> Option<&LayerInfo> {
        self.layer.as_ref()
    }

    /// The probability of failing a model gave the subject of a label this
    /// run was shown, found by the place `current` knows it at.
    /// A `notApplicable` label is about the selector and is not scored.
    fn scored(&self, current: &Found, label: &Label, repo: &Repo) -> Option<Score> {
        let place = current.place.get(&label.key)?;
        let (probability, model) = self.probabilities.get(place)?;
        Some(Score {
            fingerprint: label.key.clone(),
            repo: repo.name.clone(),
            violates: match label.truth {
                Truth::Fail => true,
                Truth::Pass => false,
                Truth::NotApplicable => return None,
            },
            probability: *probability,
            model: model.clone(),
        })
    }

    /// How the run came out on a label's subject: the outcome an example
    /// carries, else the one the run gave its fingerprint.
    pub fn outcome_of(&self, label: &Label) -> Outcome {
        label.observed.unwrap_or_else(|| {
            self.by_fingerprint
                .get(&label.key)
                .copied()
                .unwrap_or(Outcome::Unselected)
        })
    }
}

/// A candidate check's run beside the current one.
pub struct Candidate<'a> {
    /// The id of the candidate decision.
    pub id: String,
    pub found: &'a Found,
    /// What the examples show under the candidate.
    pub shown: Vec<Shown>,
}

/// A candidate decision to run beside the project's.
pub struct Setup {
    files: BTreeMap<String, String>,
}

impl Setup {
    /// The project's own decisions with the candidate file added; the id of
    /// the candidate decision.
    pub fn read(root: &Path, file: &Path) -> Result<(String, Self)> {
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

    /// The decision files of the project with the candidate's added, by name.
    pub fn catalog(&self) -> Result<Catalog> {
        Ok(Catalog::from_local(self.files.clone())?)
    }
}

/// The repositories to run in: those asked for, inside `start`, else `start`
/// itself.
pub fn repos(requested: &[Repo], start: &Path) -> Vec<Repo> {
    if requested.is_empty() {
        return vec![Repo {
            name: None,
            dir: start.to_path_buf(),
        }];
    }
    requested
        .iter()
        .map(|r| Repo {
            name: r.name.clone(),
            dir: start.join(&r.dir),
        })
        .collect()
}

/// What a repository gave: the labels graded by the outcome of the current
/// check and, when there is one, of the candidate beside it.
pub fn grade(
    request: &EvalRequest,
    repo: &Repo,
    policy: Evaluation,
    labels: &[Label],
    current: &Found,
    candidate: Option<Candidate>,
) -> Part {
    let mut part = Part {
        policy,
        current: labels
            .iter()
            .map(|l| (l.truth, current.outcome_of(l)))
            .collect(),
        candidate: Vec::new(),
        scored: labels
            .iter()
            .filter_map(|l| current.scored(current, l, repo))
            .collect(),
        notices: current.notices.clone(),
        candidate_notices: Vec::new(),
        candidate_scored: Vec::new(),
        pairs: Vec::new(),
        decided: assemble::Usage::of(current),
        candidate_decided: assemble::Usage::default(),
        audit: Vec::new(),
        check: request.decision.clone(),
        candidate_check: None,
        regressions: Vec::new(),
        layer: current.layer.clone(),
        candidate_layer: None,
    };
    let Some(candidate) = candidate else {
        part.audit = audit_rows(request, repo, current);
        return part;
    };
    let shown: BTreeMap<String, Outcome> = candidate
        .shown
        .into_iter()
        .map(|s| (s.key, s.outcome))
        .collect();
    for label in labels {
        let was = current.outcome_of(label);
        let now = if label.observed.is_some() {
            shown
                .get(&label.key)
                .copied()
                .unwrap_or(Outcome::Unselected)
        } else {
            current
                .place
                .get(&label.key)
                .and_then(|place| candidate.found.by_place.get(place))
                .copied()
                .unwrap_or(Outcome::Unselected)
        };
        if label.observed.is_some()
            && label.truth == Truth::Pass
            && now == Outcome::Fail
            && was != Outcome::Fail
        {
            part.regressions.push(label.key.clone());
        }
        part.candidate.push((label.truth, now));
        part.candidate_scored
            .extend(candidate.found.scored(current, label, repo));
        part.pairs.push((was, now));
    }
    part.candidate_decided = assemble::Usage::of(candidate.found);
    part.candidate_notices.clone_from(&candidate.found.notices);
    part.audit = audit_rows(request, repo, candidate.found);
    part.candidate_check = Some(candidate.id);
    part.candidate_layer.clone_from(&candidate.found.layer);
    part
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
