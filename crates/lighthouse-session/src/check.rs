//! Running a check: analyze, narrow the report, remember the run.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use lighthouse_engine::{Engine, ModelOptions, Run, cache};
use lighthouse_model::{Diagnostic, Preview, Severity, Suppressed};
use lighthouse_report::{
    Briefing, Compact, Descriptor, Detail, Format, Full, Report, render, report,
};
use lighthouse_resource::slashed;
use lighthouse_spec::{Catalog, Decision};
use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    Fixing, Remembered, Result, Selector, Session, findings, model::attached, review, scope,
};

/// The environment variable that sets the size limit of the cache, in MB.
const CACHE_LIMIT_VAR: &str = "LIGHTHOUSE_CACHE_LIMIT_MB";
/// Findings of a JSON report when no limit is given.
const JSON_LIMIT: usize = 25;

/// What to report on. The whole project is always analyzed.
#[derive(Default)]
pub struct CheckRequest {
    /// Report only under these paths (also narrows `changed` and `diff`).
    pub paths: Vec<PathBuf>,
    /// Report only files changed in the working tree against HEAD.
    pub changed: bool,
    /// Report only files changed since the merge base with this ref.
    pub diff: Option<String>,
    /// Run only these decisions: fully qualified ids, or globs over them
    /// such as `design/*`.
    pub rules: Vec<String>,
    /// Record the run and apply judgments.
    pub store: bool,
    /// Take the findings of rules from the result cache in
    /// `.lighthouse/cache` when the code they read has not changed, and keep
    /// new ones there. The findings are the same either way.
    pub cache: bool,
    /// What models may do. The models revisions attached are the run's own
    /// (see `detached`), whatever `attached` says.
    pub models: ModelOptions,
    /// Record what models fail as judgments attributed to them
    /// (`--record-model-labels`), so that training can reproduce them.
    pub record_model_labels: bool,
    /// Leave out the models revisions attached: the run is the check without
    /// that layer, as grading it needs.
    pub detached: bool,
}

/// How a run ended. `Incomplete` wins over everything: "not checked" is never
/// "passed".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Clean,
    Findings,
    Incomplete,
}

/// The counts of a run, shared by every frontend that reports on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Summary {
    pub status: RunStatus,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    /// Findings of decisions that authored `warn` or `info` and that no
    /// judgment stands for: the ones that ask for review.
    pub reviews: usize,
    pub incomplete: usize,
    /// Findings that judgments kept out of the report.
    pub suppressed: usize,
    /// Findings that directives in the code suppress.
    pub allowed: usize,
}

/// The `CheckReport` kind: the findings of a run for a program, with the gaps,
/// the counts and what the run has to say.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum CheckReport {
    /// The findings grouped by decision.
    Compact {
        #[serde(flatten)]
        report: Compact,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        messages: Vec<String>,
    },
    /// One self-contained record per finding.
    Full {
        #[serde(flatten)]
        report: Full,
        status: RunStatus,
        summary: Summary,
        messages: Vec<String>,
    },
}

/// A finished run with what the store decided about it.
pub struct Checked {
    pub run: Run,
    pub catalog: Arc<Catalog>,
    pub remembered: Remembered,
    /// Every suppressed finding with its suppression, the ones a directive in
    /// the code declares and the ones recorded outside it.
    pub suppressions: Vec<Suppressed>,
    /// What the user should know, in the order it happened.
    pub messages: Vec<String>,
    /// The engine that ran, which builds the models the commands that train
    /// and evaluate them ask for.
    pub(crate) engine: Engine,
    /// The project root, to read the files SARIF columns are measured in.
    root: PathBuf,
    /// How the check and the fix of each decision with a finding run, by
    /// decision id.
    descriptors: BTreeMap<String, Descriptor>,
}

impl Checked {
    /// The status and counts of this run.
    pub fn summary(&self) -> Summary {
        let run = &self.run;
        let count = |severity| {
            run.diagnostics
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        let incomplete = run.incomplete.len();
        let status = if incomplete > 0 {
            RunStatus::Incomplete
        } else if run.diagnostics.is_empty() {
            RunStatus::Clean
        } else {
            RunStatus::Findings
        };
        Summary {
            status,
            errors: count(Severity::Error),
            warnings: count(Severity::Warn),
            infos: count(Severity::Info),
            reviews: {
                let briefing = self.briefing(None);
                run.diagnostics
                    .iter()
                    .filter(|d| briefing.needs_review(d))
                    .count()
            },
            incomplete,
            suppressed: self.remembered.suppressed,
            allowed: run.suppressed.len(),
        }
    }

    /// The findings to print: what the check found, then the subjects nothing
    /// decided, which are reported beside them as `review` results and never
    /// change the exit code.
    pub fn findings(&self) -> Vec<Diagnostic> {
        let run = &self.run;
        run.diagnostics
            .iter()
            .chain(&run.reviews)
            .cloned()
            .collect()
    }

    /// The report in `format`, at most `limit` findings of the agent format.
    /// The agent format is for hooks: it shows the findings that count, with
    /// the fixes it shows and the names of the tools that act on them.
    pub fn render(&self, format: Format, limit: Option<usize>) -> String {
        let run = &self.run;
        match format {
            Format::Text => render(
                format,
                &self.findings(),
                &run.incomplete,
                &self.briefing(limit),
            ),
            Format::Sarif => {
                let sources = self.sources();
                render(
                    format,
                    &self.findings(),
                    &run.incomplete,
                    &Briefing {
                        sources: Some(&sources),
                        ..self.briefing(limit)
                    },
                )
            }
            Format::Agent => render(
                format,
                &run.diagnostics,
                &run.incomplete,
                &Briefing {
                    tool_form: true,
                    ..self.briefing(limit)
                },
            ),
        }
    }

    /// The run in SARIF with `fixes`, the previews of a fix run by
    /// fingerprint, as the `fixes` of the findings they are for, and the
    /// `properties` of the run.
    pub fn sarif_with_fixes(
        &self,
        limit: Option<usize>,
        fixes: &BTreeMap<String, Preview>,
        properties: &serde_json::Map<String, serde_json::Value>,
    ) -> String {
        let sources = self.sources();
        render(
            Format::Sarif,
            &self.findings(),
            &self.run.incomplete,
            &Briefing {
                fixes: Some(fixes),
                sources: Some(&sources),
                properties: Some(properties).filter(|p| !p.is_empty()),
                ..self.briefing(limit)
            },
        )
    }

    /// The run as a [`CheckReport`] at `detail`, at most `limit` findings.
    pub fn report(&self, detail: Detail, limit: Option<usize>) -> CheckReport {
        let limit = Some(limit.unwrap_or(JSON_LIMIT));
        let report = report(
            &self.findings(),
            &self.run.incomplete,
            &Briefing {
                detail,
                sources: None,
                tool_form: true,
                ..self.briefing(limit)
            },
        );
        let messages = self.messages.clone();
        match report {
            Report::Compact(report) => CheckReport::Compact { report, messages },
            Report::Full(report) => {
                let summary = self.summary();
                CheckReport::Full {
                    report,
                    status: summary.status,
                    summary,
                    messages,
                }
            }
        }
    }

    /// The text of the files that have findings and non-ASCII characters,
    /// by project-relative path: what SARIF needs to count columns in UTF-16
    /// code units. A file that cannot be read is left out.
    pub fn sources(&self) -> BTreeMap<String, String> {
        let files: BTreeSet<&Path> = self
            .run
            .diagnostics
            .iter()
            .chain(&self.run.reviews)
            .chain(self.suppressions.iter().map(|s| &s.diagnostic))
            .map(|d| d.file.as_path())
            .collect();
        files
            .into_iter()
            .filter_map(|file| {
                let text = std::fs::read_to_string(self.root.join(file)).ok()?;
                (!text.is_ascii()).then(|| (slashed(file), text))
            })
            .collect()
    }

    /// The briefing the agent formats draw on. It always carries the
    /// catalog, so that the summary and every format agree on which findings
    /// ask for review.
    pub fn briefing(&self, limit: Option<usize>) -> Briefing<'_> {
        Briefing {
            catalog: Some(&*self.catalog),
            facts: Some(&self.run.facts),
            notes: Some(&self.remembered.notes),
            judged: Some(&self.remembered.judged),
            suppressed: self.remembered.suppressed,
            allowed: self.run.suppressed.len(),
            suppressions: Some(&self.suppressions),
            limit,
            detail: Detail::default(),
            fixes: None,
            sources: None,
            tool_form: false,
            descriptors: Some(&self.descriptors),
            properties: None,
        }
    }
}

/// Analyzes the project, reports what `request` selects and, when asked,
/// records the run and drops the findings that judgments keep out.
pub fn check(session: &Session, request: &CheckRequest) -> Result<Checked> {
    let started = Instant::now();
    let (engine, notices) = session.engine()?;
    let root = session.root.clone();
    let catalog = Arc::clone(&session.catalog);
    let attached = if request.detached {
        BTreeMap::new()
    } else {
        attached(&root, &catalog)
    };
    let engine = engine.with_models(ModelOptions {
        attached,
        ..request.models.clone()
    });
    let engine = if request.cache {
        engine.with_cache(cache::dir(&root), cache_limit())
    } else {
        engine
    };
    let setup = started.elapsed();
    let mut messages = Vec::new();
    let enabled = catalog
        .decisions()
        .map(Decision::id)
        .filter(|id| engine.active(id));
    let rules = Selector::new(&request.rules)?.expand(enabled)?;
    let mut run = analyze(&engine, &root, request, &rules, &mut messages)?;
    run.timings.setup = setup;
    messages.extend(notices.iter().chain(&run.notices).cloned());
    let started = Instant::now();
    let remembered = if request.store {
        findings::remember(&root, &catalog, &mut run)
    } else {
        Remembered::default()
    };
    if request.store && request.record_model_labels {
        messages.extend(record_model_labels(&root, &run));
    }
    run.timings.store = started.elapsed();
    messages.extend(remembered.messages.iter().cloned());
    if !run.suppressed.is_empty() {
        messages.push(format!(
            "{} finding(s) allowed by source annotations",
            run.suppressed.len()
        ));
    }
    let suppressions = run
        .suppressed
        .iter()
        .chain(&remembered.suppressions)
        .cloned()
        .collect();
    let fixing = Fixing::of(session)?;
    let descriptors = fixing.all(
        &catalog,
        run.diagnostics
            .iter()
            .chain(&run.reviews)
            .map(|d| d.rule_id.as_str()),
    );
    Ok(Checked {
        run,
        catalog,
        remembered,
        suppressions,
        messages,
        engine,
        root,
        descriptors,
    })
}

/// The size limit of the cache in bytes: `LIGHTHOUSE_CACHE_LIMIT_MB`, else
/// 256 MB.
pub(crate) fn cache_limit() -> u64 {
    std::env::var(CACHE_LIMIT_VAR)
        .ok()
        .and_then(|mb| mb.trim().parse::<u64>().ok())
        .map_or(cache::DEFAULT_LIMIT, |mb| mb.saturating_mul(1 << 20))
}

/// Records what models failed as judgments attributed to them, in the role of
/// a model: the reason names the model and its probability.
fn record_model_labels(root: &Path, run: &Run) -> Vec<String> {
    let reviews: Vec<_> = run
        .diagnostics
        .iter()
        .filter_map(|d| {
            let model = d.model_version.as_deref()?;
            let confidence = d
                .evidence
                .get("confidence")
                .and_then(|v| v.as_f64())
                .unwrap_or_default();
            Some(lighthouse_store::NewJudgment {
                fingerprint: d.fingerprint.as_str().to_owned(),
                judgment: lighthouse_model::Judgment::Fail,
                reason: Some(format!("model {model} P(fail)={confidence:.2}")),
                suppress: None,
                attribution: lighthouse_model::Attribution::model(model),
                commit: None,
                expect_seen: None,
                lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
            })
        })
        .collect();
    if reviews.is_empty() {
        return Vec::new();
    }
    let recorded = match review::resolve_all(root, &reviews) {
        Ok(recorded) => recorded,
        Err(e) => return vec![format!("model labels were not recorded: {e}")],
    };
    let mut notes: Vec<String> = recorded
        .iter()
        .filter_map(|r| r.as_ref().err())
        .map(|e| format!("a model label was not recorded: {e}"))
        .collect();
    let done = recorded.iter().filter(|r| r.is_ok()).count();
    if done > 0 {
        notes.push(format!("{done} model label(s) recorded"));
    }
    notes
}

fn analyze(
    engine: &Engine,
    root: &Path,
    request: &CheckRequest,
    rules: &[String],
    messages: &mut Vec<String>,
) -> Result<Run> {
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
        return Ok(engine.check_files(&files, rules)?);
    }
    let default = [PathBuf::from(".")];
    let paths = if request.paths.is_empty() {
        &default[..]
    } else {
        &request.paths
    };
    Ok(engine.check(paths, rules)?)
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
