//! Running a check: analyze, narrow the report, remember the run.

use std::path::{Path, PathBuf};

use lighthouse_engine::{Engine, Outcome};
use lighthouse_model::Severity;
use lighthouse_report::Briefing;
use lighthouse_spec::Catalog;
use serde::Serialize;

use crate::{Remembered, Result, Session, findings, scope};

/// What to report on. The whole project is always analyzed.
#[derive(Default)]
pub struct CheckRequest {
    /// Report only under these paths (also narrows `changed` and `diff`).
    pub paths: Vec<PathBuf>,
    /// Report only files changed in the working tree against HEAD.
    pub changed: bool,
    /// Report only files changed since the merge base with this ref.
    pub diff: Option<String>,
    /// Run only these fully qualified rule ids.
    pub rules: Vec<String>,
    /// Record the run and apply review verdicts.
    pub store: bool,
}

/// How a run ended. `Incomplete` wins over everything: "not checked" is never
/// "passed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Clean,
    Findings,
    Incomplete,
}

/// The counts of a run, shared by every frontend that reports on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub status: Status,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    /// Findings of heuristic and judgment decisions, whatever their severity:
    /// the ones that ask for a verdict.
    pub reviews: usize,
    pub incomplete: usize,
    /// Findings that verdicts kept out of the report.
    pub suppressed: usize,
    /// Findings that source annotations allow.
    pub allowed: usize,
}

/// A finished run with what the store decided about it.
pub struct Checked {
    pub outcome: Outcome,
    pub catalog: Catalog,
    pub remembered: Remembered,
    /// What the user should know, in the order it happened.
    pub messages: Vec<String>,
}

impl Checked {
    /// The status and counts of this run.
    pub fn summary(&self) -> Summary {
        let outcome = &self.outcome;
        let count = |severity| {
            outcome
                .diagnostics
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        let incomplete = outcome.incomplete.len();
        let status = if incomplete > 0 {
            Status::Incomplete
        } else if outcome.diagnostics.is_empty() {
            Status::Clean
        } else {
            Status::Findings
        };
        Summary {
            status,
            errors: count(Severity::Error),
            warnings: count(Severity::Warn),
            infos: count(Severity::Info),
            reviews: {
                let briefing = self.briefing(None);
                outcome
                    .diagnostics
                    .iter()
                    .filter(|d| briefing.needs_verdict(d))
                    .count()
            },
            incomplete,
            suppressed: self.remembered.suppressed,
            allowed: outcome.allowed.len(),
        }
    }

    /// The briefing the agent formats draw on. It always carries the
    /// catalog, so that the summary and every format agree on which findings
    /// ask for a verdict.
    pub fn briefing(&self, limit: Option<usize>) -> Briefing<'_> {
        Briefing {
            catalog: Some(&self.catalog),
            facts: Some(&self.outcome.facts),
            notes: Some(&self.remembered.notes),
            suppressed: self.remembered.suppressed,
            allowed: self.outcome.allowed.len(),
            limit,
        }
    }
}

/// Analyzes the project, reports what `request` selects and, when asked,
/// records the run and drops the findings that verdicts keep out.
pub fn check(session: Session, request: &CheckRequest) -> Result<Checked> {
    let (registry, plugins) = session.registry()?;
    let trusted = session.trusted();
    let root = session.root.clone();
    let catalog = session.catalog()?;
    let engine = Engine::new(registry, session.config, &root)?
        .with_incomplete(plugins.incomplete)
        .with_trust(trusted);
    let mut messages = Vec::new();
    let mut outcome = analyze(&engine, &root, request, &mut messages)?;
    messages.extend(plugins.notices.iter().chain(&outcome.notices).cloned());
    let remembered = if request.store {
        findings::remember(&root, &catalog, &mut outcome)
    } else {
        Remembered::default()
    };
    messages.extend(remembered.messages.iter().cloned());
    if !outcome.allowed.is_empty() {
        messages.push(format!(
            "{} finding(s) allowed by source annotations",
            outcome.allowed.len()
        ));
    }
    Ok(Checked {
        outcome,
        catalog,
        remembered,
        messages,
    })
}

fn analyze(
    engine: &Engine,
    root: &Path,
    request: &CheckRequest,
    messages: &mut Vec<String>,
) -> Result<Outcome> {
    let files = match (request.changed, request.diff.as_deref()) {
        (true, _) => Some(scope::changed(root)?),
        (_, Some(base)) => Some(scope::since(root, base)?),
        _ => None,
    };
    if let Some(files) = files {
        let files = within(root, &request.paths, files)?;
        messages.push(format!(
            "reporting {} changed file(s); the whole project is analyzed",
            files.len()
        ));
        return Ok(engine.check_files(&files, &request.rules)?);
    }
    let default = [PathBuf::from(".")];
    let paths = if request.paths.is_empty() {
        &default[..]
    } else {
        &request.paths
    };
    Ok(engine.check(paths, &request.rules)?)
}

/// The changed files that also lie under the given paths, when there are any.
fn within(root: &Path, paths: &[PathBuf], files: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Ok(files);
    }
    let root = root.canonicalize()?;
    let mut scopes = Vec::new();
    for path in paths {
        let absolute = path.canonicalize()?;
        scopes.push(
            absolute
                .strip_prefix(&root)
                .map(Path::to_owned)
                .map_err(|_| {
                    format!(
                        "{} is outside the project root {}",
                        path.display(),
                        root.display()
                    )
                })?,
        );
    }
    Ok(files
        .into_iter()
        .filter(|f| scopes.iter().any(|s| f.starts_with(s)))
        .collect())
}
