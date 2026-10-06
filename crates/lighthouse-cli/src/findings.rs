//! The quality store as `check` uses it: record what a run saw, then keep out
//! of the report the findings that review verdicts rejected.

use std::{collections::BTreeMap, path::Path};

use lighthouse_engine::Outcome;
use lighthouse_model::{Diagnostic, Fingerprint, Incomplete};
use lighthouse_spec::{Catalog, Pattern, tier};
use lighthouse_store::{Judgment, Observed, Run, Standing, Store, Unchecked};
use serde_json::{Value, json};

use crate::git;

/// What the store decided about a run's findings.
#[derive(Default)]
pub struct Remembered {
    /// Findings left out of the report by a rejected verdict.
    pub suppressed: usize,
    /// Why a finding is reported although a verdict was recorded on it.
    pub notes: BTreeMap<Fingerprint, String>,
}

/// Records the run in the project's store and removes the findings whose
/// latest verdict keeps them out of reports from `outcome`. A verdict whose
/// rule or evidence changed no longer applies, and a mechanical finding is
/// never suppressed; both stay in the report with a note. A store that cannot
/// be used never fails the check: it is reported, and verdicts already
/// recorded are still applied read-only.
pub fn remember(root: &Path, catalog: &Catalog, outcome: &mut Outcome) -> Remembered {
    let judged = match record(root, catalog, outcome) {
        Ok(judged) => judged,
        Err(e) => {
            eprintln!("lighthouse: findings not recorded: {e}");
            read_only(root)
        }
    };
    let mut remembered = Remembered::default();
    outcome.diagnostics.retain(|d| {
        let Some(judgment) = judged.get(d.fingerprint.as_str()) else {
            return true;
        };
        match note(judgment) {
            Some(note) => {
                remembered.notes.insert(d.fingerprint.clone(), note);
                true
            }
            None => {
                remembered.suppressed += 1;
                false
            }
        }
    });
    announce(&remembered);
    remembered
}

/// Why a judged finding is reported anyway; `None` when it is suppressed.
fn note(judgment: &Judgment) -> Option<String> {
    match judgment.standing {
        Standing::Suppressed => None,
        Standing::RuleChanged => Some("verdict expired: rule changed".to_owned()),
        Standing::EvidenceChanged => Some("verdict expired: evidence changed".to_owned()),
        Standing::Unsuppressible => Some(format!(
            "rejected as {} \u{2014} mechanical findings are not suppressible; fix the rule",
            judgment.reason
        )),
    }
}

fn announce(remembered: &Remembered) {
    if remembered.suppressed > 0 {
        eprintln!(
            "lighthouse: {} finding(s) suppressed by review verdicts (`lighthouse review list --status suppressed`)",
            remembered.suppressed
        );
    }
    let count = |text: &str| {
        remembered
            .notes
            .values()
            .filter(|n| n.contains(text))
            .count()
    };
    for (text, what) in [
        ("rule changed", "verdict expired: rule changed"),
        ("evidence changed", "verdict expired: evidence changed"),
        (
            "not suppressible",
            "rejected mechanical finding(s) stay reported: mechanical findings are not suppressible; fix the rule",
        ),
    ] {
        let found = count(text);
        if found > 0 {
            eprintln!("lighthouse: {found} finding(s) reported again, {what}");
        }
    }
}

fn record(
    root: &Path,
    catalog: &Catalog,
    outcome: &Outcome,
) -> Result<BTreeMap<String, Judgment>, lighthouse_store::Error> {
    let mut store = Store::open(root)?;
    store.record(&run_of(root, catalog, outcome))?;
    store.standings()
}

/// The verdicts already recorded, when recording this run was not possible.
fn read_only(root: &Path) -> BTreeMap<String, Judgment> {
    Store::open_existing(root)
        .ok()
        .flatten()
        .and_then(|store| store.standings().ok())
        .unwrap_or_default()
}

/// What the store needs to know about a run: its findings with their facts,
/// options and rule versions, the paths and rules it covered, what it could not
/// check, and where and with which tools it ran.
fn run_of(root: &Path, catalog: &Catalog, outcome: &Outcome) -> Run {
    Run {
        observed: outcome
            .diagnostics
            .iter()
            .map(|d| observed(d, catalog, outcome))
            .collect(),
        reported: outcome
            .reported
            .iter()
            .map(|p| slashed(&p.to_string_lossy()))
            .collect(),
        rules: outcome.rules.clone(),
        configured: outcome.configured.clone(),
        unchecked: unchecked(&outcome.incomplete),
        commit: git::head(root),
        dirty: git::dirty(root),
        catalog_version: Some(catalog.version()),
        lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

fn observed(d: &Diagnostic, catalog: &Catalog, outcome: &Outcome) -> Observed {
    let facts = outcome.facts.get(&d.fingerprint);
    let pattern = catalog.pattern(&d.rule_id);
    let mut record = Observed::from_diagnostic(d, facts.cloned().unwrap_or_else(|| json!({})));
    record.tier = tier(d.severity, pattern).to_owned();
    record.options = options(pattern, d, facts, outcome);
    record.rule_version = pattern.map(Pattern::semantic_version);
    record.pattern_hash = pattern.map(Pattern::version);
    record
}

/// The options the rule ran with: the configured ones over the pattern's
/// defaults for the file's language.
fn options(
    pattern: Option<&Pattern>,
    d: &Diagnostic,
    facts: Option<&Value>,
    outcome: &Outcome,
) -> Value {
    let configured = outcome
        .options
        .get(&d.fingerprint)
        .cloned()
        .unwrap_or_default();
    let language = facts
        .and_then(|f| f.get("language"))
        .and_then(Value::as_str);
    match pattern.map(|p| p.resolve_options(&configured, language)) {
        Some(Ok(resolved)) => Value::Object(resolved),
        _ => Value::Object(configured),
    }
}

/// Where the run could not check: directories of the files that were not
/// analyzed, or everything when a gap cannot be placed.
fn unchecked(incomplete: &[Incomplete]) -> Unchecked {
    let mut dirs = Vec::new();
    for gap in incomplete {
        let Some(path) = &gap.path else {
            return Unchecked::Everything;
        };
        let dir = path.parent().map(|p| slashed(&p.to_string_lossy()));
        dirs.push(dir.unwrap_or_default());
    }
    if dirs.is_empty() {
        Unchecked::Nothing
    } else {
        Unchecked::Paths(dirs)
    }
}

fn slashed(path: &str) -> String {
    path.replace('\\', "/")
}
