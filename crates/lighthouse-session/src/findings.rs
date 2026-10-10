//! The quality store as `check` uses it: record what a run saw, then keep out
//! of the report the findings that review verdicts rejected.

use std::{collections::BTreeMap, path::Path};

use lighthouse_engine::Outcome;
use lighthouse_model::{Diagnostic, Fingerprint, Incomplete};
use lighthouse_spec::{Catalog, Decision, authored_severity};
use lighthouse_store::{Observed, Rejection, Run, Standing, Store, Unchecked};
use serde_json::{Value, json};

use crate::git;

/// What the store decided about a run's findings.
#[derive(Default)]
pub struct Remembered {
    /// Findings left out of the report by a rejected verdict.
    pub suppressed: usize,
    /// Why a finding is reported although a verdict was recorded on it.
    pub notes: BTreeMap<Fingerprint, String>,
    /// What the user should know about how the store was used.
    pub messages: Vec<String>,
}

/// What identifies a decision's wording and check, hashed once per decision
/// however many findings it has.
struct Versions {
    meaning: String,
    legacy: String,
    check: String,
    wording: String,
}

/// The versions of the decisions seen so far in a run, by decision id.
#[derive(Default)]
struct Seen(BTreeMap<String, Versions>);

impl Seen {
    fn of(&mut self, decision: &Decision) -> &Versions {
        self.0
            .entry(decision.id().to_owned())
            .or_insert_with(|| Versions {
                meaning: decision.meaning_version(),
                legacy: decision.earlier_versions().join(","),
                check: decision.check_revision(),
                wording: decision.version(),
            })
    }
}

/// Records the run in the project's store and removes the findings whose
/// latest verdict keeps them out of reports from `outcome`. A verdict whose
/// rule or evidence changed no longer applies, and a mechanical finding is
/// never suppressed; both stay in the report with a note. A store that cannot
/// be used never fails the check: it is reported, and verdicts already
/// recorded are still applied read-only.
pub fn remember(root: &Path, catalog: &Catalog, outcome: &mut Outcome) -> Remembered {
    let mut remembered = Remembered::default();
    let judged = match record(root, catalog, outcome) {
        Ok((judged, notices)) => {
            remembered.messages.extend(notices);
            judged
        }
        Err(e) => {
            remembered
                .messages
                .push(format!("findings not recorded: {e}"));
            read_only(root)
        }
    };
    outcome.diagnostics.retain(|d| {
        let Some(judgment) = judgment_of(&judged, d) else {
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
    announce(&mut remembered);
    remembered
}

/// Removes from `outcome` the findings that the committed decision log keeps
/// out of reports, without recording anything: for runs that do not use the
/// store but still honor what the team decided.
pub fn apply_verdicts(root: &Path, outcome: &mut Outcome) -> usize {
    let judged = read_only(root);
    let before = outcome.diagnostics.len();
    outcome
        .diagnostics
        .retain(|d| judgment_of(&judged, d).is_none_or(|j| note(j).is_some()));
    before - outcome.diagnostics.len()
}

/// The standing of the finding's latest rejection. A verdict recorded under
/// the fingerprint the decision's name seeded still applies in a run that has
/// not yet moved it to the one its uid seeds.
fn judgment_of<'j>(
    judged: &'j BTreeMap<String, Rejection>,
    d: &Diagnostic,
) -> Option<&'j Rejection> {
    judged.get(d.fingerprint.as_str()).or_else(|| {
        d.legacy_fingerprints
            .iter()
            .find_map(|legacy| judged.get(legacy.as_str()))
    })
}

/// Why a judged finding is reported anyway; `None` when it is suppressed.
fn note(rejection: &Rejection) -> Option<String> {
    match rejection.standing {
        Standing::Suppressed => None,
        Standing::RuleChanged => Some("verdict expired: rule changed".to_owned()),
        Standing::EvidenceChanged => Some("verdict expired: evidence changed".to_owned()),
        Standing::Unsuppressible => Some(format!(
            "rejected as {} \u{2014} mechanical findings are not suppressible; fix the rule",
            rejection.reason
        )),
    }
}

fn announce(remembered: &mut Remembered) {
    if remembered.suppressed > 0 {
        remembered.messages.push(format!(
            "{} finding(s) suppressed by review verdicts (`lighthouse review list --status suppressed`)",
            remembered.suppressed
        ));
    }
    let count = |text: &str| {
        remembered
            .notes
            .values()
            .filter(|n| n.contains(text))
            .count()
    };
    let mut messages = Vec::new();
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
            messages.push(format!("{found} finding(s) reported again, {what}"));
        }
    }
    remembered.messages.extend(messages);
}

fn record(
    root: &Path,
    catalog: &Catalog,
    outcome: &Outcome,
) -> Result<(BTreeMap<String, Rejection>, Vec<String>), lighthouse_store::Error> {
    let mut store = Store::open(root)?;
    store.identify(&catalog.identities())?;
    store.record(&run_of(root, catalog, outcome))?;
    Ok((store.standings()?, store.notices().to_vec()))
}

/// The verdicts already recorded, when recording this run was not possible.
fn read_only(root: &Path) -> BTreeMap<String, Rejection> {
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
        observed: {
            let mut seen = Seen::default();
            outcome
                .diagnostics
                .iter()
                .map(|d| observed(d, catalog, outcome, &mut seen))
                .collect()
        },
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

fn observed(d: &Diagnostic, catalog: &Catalog, outcome: &Outcome, seen: &mut Seen) -> Observed {
    let facts = outcome.facts.get(&d.fingerprint);
    let decision = catalog.decision(&d.rule_id);
    let mut record = Observed::from_diagnostic(d, facts.cloned().unwrap_or_else(|| json!({})));
    record.decision_uid = decision.and_then(|d| d.uid()).map(str::to_owned);
    record.authored_severity = authored_severity(d.severity, decision).to_string();
    record.options = options(decision, d, facts, outcome);
    let versions = decision.map(|d| seen.of(d));
    record.rule_version = versions.map(|v| v.meaning.clone());
    record.legacy_rule_version = versions.map(|v| v.legacy.clone());
    record.check_revision = versions.map(|v| v.check.clone());
    record.decision_hash = versions.map(|v| v.wording.clone());
    record
}

/// The options the rule ran with: the configured ones over the decision's
/// defaults for the file's language.
fn options(
    decision: Option<&Decision>,
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
    match decision.map(|d| d.resolve_options(&configured, language)) {
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
