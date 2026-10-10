//! Running a check: analyze, narrow the report, remember the run.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Instant,
};

use lighthouse_engine::{Engine, FixPlan, Outcome};
use lighthouse_model::{Diagnostic, Severity};
use lighthouse_report::{Briefing, Detail, Fix, FixFile};
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
    /// The engine that ran. A report asks for fix previews after the check,
    /// when its limit has chosen the findings to show, and a preview needs the
    /// engine's registry and workspace; the engine is not rebuilt for it.
    engine: Engine,
    /// The project root, to read the files SARIF columns are measured in.
    root: PathBuf,
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

    /// The fixes proposed for `findings`, by fingerprint, computed in memory:
    /// nothing is written, formatted or re-checked, and a finding whose fix
    /// does not apply cleanly, runs a program or does not exist has none.
    /// Applying a fix goes through a fix run, which verifies it.
    pub fn fixes(&self, findings: &[&Diagnostic]) -> BTreeMap<String, Fix> {
        let plan = FixPlan::from_catalog(&self.catalog);
        self.engine
            .preview_fixes(&plan, &self.outcome, findings)
            .into_iter()
            .map(|preview| {
                let files = preview
                    .changes
                    .into_iter()
                    .map(|change| {
                        let edits = preview.edits.iter().filter(|e| e.file == change.path);
                        FixFile::new(
                            change.path.to_string_lossy().replace('\\', "/"),
                            &change.before,
                            &change.after,
                            edits.map(|e| (e.start, e.end, e.text.clone())),
                        )
                    })
                    .collect();
                let fix = Fix {
                    safety: preview.safety,
                    description: preview.description,
                    files,
                };
                (preview.fingerprint.as_str().to_owned(), fix)
            })
            .collect()
    }

    /// The text of the files that have findings and non-ASCII characters,
    /// by project-relative path: what SARIF needs to count columns in UTF-16
    /// code units. A file that cannot be read is left out.
    pub fn sources(&self) -> BTreeMap<String, String> {
        let files: BTreeSet<&Path> = self
            .outcome
            .diagnostics
            .iter()
            .map(|d| d.file.as_path())
            .collect();
        files
            .into_iter()
            .filter_map(|file| {
                let text = std::fs::read_to_string(self.root.join(file)).ok()?;
                let key = file.to_string_lossy().replace('\\', "/");
                (!text.is_ascii()).then_some((key, text))
            })
            .collect()
    }

    /// The fixes of the findings an agent format shows at `detail` under
    /// `limit`, or of the first `cap` findings for a format that shows them
    /// all.
    pub fn shown_fixes(&self, limit: Option<usize>, detail: Detail) -> BTreeMap<String, Fix> {
        let briefing = Briefing {
            detail,
            ..self.briefing(limit)
        };
        let shown = lighthouse_report::shown(&self.outcome.diagnostics, &briefing);
        self.fixes(&shown)
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
            detail: Detail::default(),
            fixes: None,
            sources: None,
            mcp: false,
        }
    }
}

/// Analyzes the project, reports what `request` selects and, when asked,
/// records the run and drops the findings that verdicts keep out.
pub fn check(session: Session, request: &CheckRequest) -> Result<Checked> {
    let started = Instant::now();
    let (registry, plugins) = session.registry()?;
    let trusted = session.trusted();
    let root = session.root.clone();
    let catalog = session.catalog()?;
    let engine = Engine::new(registry, session.config, &catalog, &root)?
        .with_incomplete(plugins.incomplete)
        .with_trust(trusted);
    let setup = started.elapsed();
    let mut messages = Vec::new();
    let mut outcome = analyze(&engine, &root, request, &mut messages)?;
    outcome.timings.setup = setup;
    messages.extend(plugins.notices.iter().chain(&outcome.notices).cloned());
    let started = Instant::now();
    let remembered = if request.store {
        findings::remember(&root, &catalog, &mut outcome)
    } else {
        Remembered::default()
    };
    outcome.timings.store = started.elapsed();
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
        engine,
        root,
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
