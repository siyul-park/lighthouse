//! Fixing: the findings a request selects are fixed by the fixers the
//! catalog names, verified, written and remembered. The engine's orchestrator
//! does the work; this adds what only a session knows: the catalog, the
//! verdicts that keep findings out of reach, and the store.

use std::{collections::BTreeSet, path::PathBuf};

use lighthouse_engine::{
    AppliedFix, DeclinedFix, Engine, FixPlan, FixReport, FixRun, unified_diff,
};
use lighthouse_model::Fingerprint;
use lighthouse_spec::Catalog;
use lighthouse_store::{NewFix, Store};
use serde::Serialize;

use crate::{Result, Session, findings, git};

/// What to fix. At least one of `paths`, `fingerprints` and `rules` narrows a
/// run from the whole project.
#[derive(Default)]
pub struct FixSelection {
    pub paths: Vec<PathBuf>,
    /// Fingerprints or unambiguous prefixes.
    pub fingerprints: Vec<String>,
    pub rules: Vec<String>,
    /// Do everything but leave the files as they were.
    pub dry_run: bool,
    /// Also apply suggested fixes.
    pub unsafe_fixes: bool,
    /// Use this registered fixer instead of the catalog's.
    pub fixer: Option<String>,
    /// Record the run and its fixes, and leave out what verdicts suppress.
    pub store: bool,
}

/// A finished fix run.
pub struct Fixed {
    pub report: FixReport,
    /// The unified diff of every changed file, in path order.
    pub diff: String,
    /// What the user should know, in the order it happened.
    pub messages: Vec<String>,
    pub dry_run: bool,
}

/// The applied fix as a frontend reports it.
#[derive(Debug, Serialize)]
pub struct AppliedRow {
    pub fingerprint: String,
    pub rule: String,
    pub fixer: String,
    pub safety: String,
    pub description: String,
    pub files: Vec<String>,
}

/// The finding that was left as it was, as a frontend reports it.
#[derive(Debug, Serialize)]
pub struct DeclinedRow {
    pub fingerprint: String,
    pub rule: String,
    pub path: String,
    pub line: u32,
    pub reason: String,
}

impl Fixed {
    /// The fixes that were applied, as a frontend reports them.
    pub fn applied(&self) -> Vec<AppliedRow> {
        self.report.applied.iter().map(applied_row).collect()
    }

    /// The findings that were left alone, as a frontend reports them.
    pub fn declined(&self) -> Vec<DeclinedRow> {
        self.report.declined.iter().map(declined_row).collect()
    }
}

/// The fixer of every rule whose decision has a `fix`; see
/// [`FixPlan::from_catalog`].
pub fn fix_plan(catalog: &Catalog) -> FixPlan {
    FixPlan::from_catalog(catalog)
}

/// Fixes what `request` selects. With `dry_run` nothing is left changed and
/// nothing is recorded; otherwise every applied fix is recorded and the
/// findings it removed are resolved in the store.
pub fn fix(session: Session, request: &FixSelection) -> Result<Fixed> {
    let (registry, plugins) = session.registry()?;
    let trusted = session.trusted();
    let root = session.root.clone();
    let catalog = session.catalog()?;
    let plan = fix_plan(&catalog);
    let engine = Engine::new(registry, session.config, &catalog, &root)?
        .with_incomplete(plugins.incomplete)
        .with_trust(trusted);
    let mut messages = plugins.notices.clone();
    let skip = suppressed(&engine, &root, &catalog, request.store)?;
    let run = FixRun {
        paths: request.paths.clone(),
        rules: request.rules.clone(),
        fingerprints: request.fingerprints.clone(),
        skip,
        dry_run: request.dry_run,
        unsafe_fixes: request.unsafe_fixes,
        fixer: request.fixer.clone(),
        trusted,
    };
    let mut report = engine.fix(&plan, &run)?;
    messages.extend(report.notes.iter().cloned());
    if request.store && !request.dry_run {
        record(&root, &catalog, &mut report, &mut messages);
    }
    let diff = report
        .changes
        .iter()
        .map(|c| unified_diff(&c.path, &c.before, &c.after))
        .collect();
    Ok(Fixed {
        report,
        diff,
        messages,
        dry_run: request.dry_run,
    })
}

fn applied_row(fix: &AppliedFix) -> AppliedRow {
    AppliedRow {
        fingerprint: fix.fingerprint.as_str().to_owned(),
        rule: fix.rule_id.clone(),
        fixer: fix.fixer.clone(),
        safety: fix.safety.to_string(),
        description: fix.description.clone(),
        files: fix.files.iter().map(|f| slashed(f)).collect(),
    }
}

fn declined_row(fix: &DeclinedFix) -> DeclinedRow {
    DeclinedRow {
        fingerprint: fix.fingerprint.as_str().to_owned(),
        rule: fix.rule_id.clone(),
        path: slashed(&fix.file),
        line: fix.line,
        reason: fix.reason.clone(),
    }
}

fn slashed(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Reads the project as it is before any fix, recording it when the run uses
/// the store, and returns the
/// findings that verdicts keep out of reports: a fixer leaves those alone.
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
        findings::apply_verdicts(root, &mut outcome);
    }
    let kept: BTreeSet<Fingerprint> = outcome
        .diagnostics
        .iter()
        .map(|d| d.fingerprint.clone())
        .collect();
    Ok(all.difference(&kept).cloned().collect())
}

/// Records each applied fix, then the run that ended the fixing, which
/// resolves the findings the fixes removed. A store that cannot be used is
/// reported, never fatal: the files are already fixed.
fn record(
    root: &std::path::Path,
    catalog: &Catalog,
    report: &mut FixReport,
    messages: &mut Vec<String>,
) {
    let written = (|| -> Result<()> {
        let mut store = Store::open(root)?;
        for fix in &report.applied {
            store.record_fix(&NewFix {
                fingerprint: fix.fingerprint.as_str().to_owned(),
                rule_id: fix.rule_id.clone(),
                fixer: fix.fixer.clone(),
                safety: fix.safety.to_string(),
                description: fix.description.clone(),
                files: fix.files.iter().map(|f| slashed(f)).collect(),
                commit: git::head(root),
                lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
            })?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        messages.push(format!("fixes not recorded: {e}"));
        return;
    }
    let remembered = findings::remember(root, catalog, &mut report.finished);
    messages.extend(remembered.messages);
}
