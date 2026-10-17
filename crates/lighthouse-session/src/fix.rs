//! Fixing: the findings a request selects are fixed by the fixers the
//! catalog names, verified, written and remembered. The engine's orchestrator
//! does the work; this adds what only a session knows: the catalog, the
//! judgments that keep findings out of reach, and the store.

use std::{collections::BTreeSet, path::PathBuf};

use lighthouse_engine::{AppliedFix, DeclinedFix, Engine, FixPlan, FixRun, unified_diff};
use lighthouse_model::Fingerprint;
use lighthouse_resource::slashed;
use lighthouse_spec::{Catalog, Decision};
use schemars::JsonSchema;
use serde::Serialize;

use crate::{Result, Selector, Session, findings, selector::long_enough};

/// Which findings to fix. At least one of `paths`, `fingerprints` and `rules`
/// narrows a run from the whole project.
#[derive(Default)]
pub struct Targets {
    pub paths: Vec<PathBuf>,
    /// Fingerprints or unambiguous prefixes.
    pub fingerprints: Vec<String>,
    pub rules: Vec<String>,
}

/// What to fix, and how.
#[derive(Default)]
pub struct Selection {
    pub targets: Targets,
    /// Do everything but leave the files as they were.
    pub dry_run: bool,
    /// Also apply suggested fixes.
    pub unsafe_fixes: bool,
    /// Use this registered fixer instead of the catalog's.
    pub fixer: Option<String>,
    /// Record the run and its fixes, and leave out what judgments hide.
    pub store: bool,
}

/// A finished fix run: the `FixReport` kind.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fixed {
    pub dry_run: bool,
    /// The unified diff of every changed file, in path order.
    pub diff: String,
    pub applied: Vec<Applied>,
    pub declined: Vec<Declined>,
    /// How many times fixing was repeated until nothing was left.
    pub rounds: usize,
    /// What the user should know, in the order it happened.
    pub messages: Vec<String>,
}

/// A fix that was applied.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Applied {
    pub fingerprint: String,
    pub decision: String,
    pub fixer: String,
    pub safety: String,
    pub description: String,
    pub files: Vec<String>,
}

/// A finding that was left as it was, and why.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Declined {
    pub fingerprint: String,
    pub decision: String,
    pub path: String,
    pub line: u32,
    pub reason: String,
}

impl Fixed {
    /// What a person is told about the run, a line each: the messages, every
    /// fix applied (or that would be) and every finding left alone, and the
    /// count.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = self.messages.clone();
        let verb = if self.dry_run { "would fix" } else { "fixed" };
        for fix in &self.applied {
            lines.push(format!(
                "{verb} {} [{}] {} ({}): {}",
                fix.decision,
                fix.safety,
                fix.files.join(", "),
                fix.fixer,
                fix.description
            ));
        }
        for fix in &self.declined {
            lines.push(format!(
                "not fixed {}:{} {}: {}",
                fix.path, fix.line, fix.decision, fix.reason
            ));
        }
        lines.push(format!(
            "{} fix(es) {}, {} left alone",
            self.applied.len(),
            if self.dry_run { "proposed" } else { "applied" },
            self.declined.len()
        ));
        lines
    }
}

impl From<&AppliedFix> for Applied {
    fn from(fix: &AppliedFix) -> Self {
        Self {
            fingerprint: fix.fingerprint.as_str().to_owned(),
            decision: fix.rule_id.clone(),
            fixer: fix.fixer.clone(),
            safety: fix.safety.to_string(),
            description: fix.description.clone(),
            files: fix.files.iter().map(|f| slashed(f)).collect(),
        }
    }
}

impl From<&DeclinedFix> for Declined {
    fn from(fix: &DeclinedFix) -> Self {
        Self {
            fingerprint: fix.fingerprint.as_str().to_owned(),
            decision: fix.rule_id.clone(),
            path: slashed(&fix.file),
            line: fix.line,
            reason: fix.reason.clone(),
        }
    }
}

/// Fixes what `request` selects. With `dry_run` nothing is left changed and
/// nothing is recorded; otherwise every applied fix is recorded and the
/// findings it removed are resolved in the store.
pub fn run(session: &Session, request: &Selection) -> Result<Fixed> {
    for prefix in &request.targets.fingerprints {
        long_enough(prefix)?;
    }
    let catalog = &session.catalog;
    let root = &session.root;
    let plan = FixPlan::from_catalog(catalog);
    let (engine, notices) = session.engine()?;
    let mut messages = notices;
    let skip = suppressed(&engine, root, catalog, request.store)?;
    let run = FixRun {
        paths: request.targets.paths.clone(),
        rules: Selector::new(&request.targets.rules)?.expand(
            catalog
                .decisions()
                .map(Decision::id)
                .filter(|id| engine.active(id)),
        )?,
        fingerprints: request.targets.fingerprints.clone(),
        skip,
        dry_run: request.dry_run,
        unsafe_fixes: request.unsafe_fixes,
        fixer: request.fixer.clone(),
    };
    let mut report = engine.fix(&plan, &run)?;
    messages.extend(report.notes.iter().cloned());
    if request.store && !request.dry_run {
        let remembered = findings::remember(root, catalog, &mut report.finished);
        messages.extend(remembered.messages);
    }
    let diff = report
        .changes
        .iter()
        .map(|c| unified_diff(&c.path, &c.before, &c.after))
        .collect();
    Ok(Fixed {
        dry_run: request.dry_run,
        diff,
        applied: report.applied.iter().map(Applied::from).collect(),
        declined: report.declined.iter().map(Declined::from).collect(),
        rounds: report.rounds,
        messages,
    })
}

/// Reads the project as it is before any fix, recording it when the run uses
/// the store, and returns the
/// findings that judgments keep out of reports: a fixer leaves those alone.
fn suppressed(
    engine: &Engine,
    root: &std::path::Path,
    catalog: &Catalog,
    store: bool,
) -> Result<BTreeSet<Fingerprint>> {
    let mut outcome = engine.check(&[], &[])?;
    let all: BTreeSet<Fingerprint> = outcome
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.clone())
        .collect();
    if store {
        findings::remember(root, catalog, &mut outcome);
    } else {
        findings::apply_judgments(root, catalog, &mut outcome)?;
    }
    let kept: BTreeSet<Fingerprint> = outcome
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.clone())
        .collect();
    Ok(all.difference(&kept).cloned().collect())
}
