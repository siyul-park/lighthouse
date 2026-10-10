//! The quality store as `check` uses it: record what a run saw, then keep out
//! of the report the findings that judgments hide.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use lighthouse_engine::Outcome;
use lighthouse_model::{Diagnostic, Fingerprint, Incomplete, Suppressed, Suppression};
use lighthouse_spec::{Catalog, Decision, authored_severity};
use lighthouse_store::{Observed, Ruling, Run, Standing, Store, Unchecked};
use serde_json::{Value, json};

use crate::git;

/// What the store decided about a run's findings.
#[derive(Default)]
pub struct Remembered {
    /// Findings left out of the report by their latest judgment.
    pub suppressed: usize,
    /// The ones among them that a `fail` with an external suppression keeps in
    /// place: a report that lists suppressions lists these.
    pub suppressions: Vec<Suppressed>,
    /// Findings a judgment stands for and that stay in the report: they no
    /// longer ask for review.
    pub judged: BTreeSet<Fingerprint>,
    /// Why a finding is reported although a judgment was recorded on it.
    pub notes: BTreeMap<Fingerprint, String>,
    /// What the user should know about how the store was used.
    pub messages: Vec<String>,
}

/// What identifies a decision's wording and check, hashed once per decision
/// however many findings it has.
struct Versions {
    meaning: String,
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
                check: decision.check_revision(),
                wording: decision.version(),
            })
    }
}

/// Records the run in the project's store and removes from `outcome` the
/// findings that their latest judgment hides. A judgment whose decision or
/// evidence changed no longer applies, and an error is never hidden; both stay
/// in the report with a note. A store that cannot be used never fails the
/// check: it is reported, and judgments already recorded are still applied
/// read-only.
pub fn remember(root: &Path, catalog: &Catalog, outcome: &mut Outcome) -> Remembered {
    let mut remembered = Remembered::default();
    let rulings = match record(root, catalog, outcome) {
        Ok(rulings) => rulings,
        Err(e) => {
            remembered
                .messages
                .push(format!("findings not recorded: {e}"));
            read_only(root)
        }
    };
    let mut kept = Vec::with_capacity(outcome.diagnostics.len());
    for d in std::mem::take(&mut outcome.diagnostics) {
        let Some(ruling) = rulings.get(d.fingerprint.as_str()) else {
            kept.push(d);
            continue;
        };
        match ruling.standing {
            Standing::Suppressed => {
                remembered.suppressed += 1;
                if let Some(justification) = &ruling.justification {
                    remembered.suppressions.push(Suppressed {
                        diagnostic: d,
                        suppression: Suppression::external(justification.clone()),
                    });
                }
            }
            Standing::Judged => {
                remembered.judged.insert(d.fingerprint.clone());
                kept.push(d);
            }
            _ => {
                if let Some(note) = note(ruling) {
                    remembered.notes.insert(d.fingerprint.clone(), note);
                }
                kept.push(d);
            }
        }
    }
    outcome.diagnostics = kept;
    announce(&mut remembered);
    remembered
}

/// Removes from `outcome` the findings that the committed decision log hides,
/// without recording anything: for runs that do not use the store but still
/// honor what the team decided.
pub fn apply_judgments(root: &Path, outcome: &mut Outcome) -> usize {
    let rulings = read_only(root);
    let before = outcome.diagnostics.len();
    outcome.diagnostics.retain(|d| {
        rulings
            .get(d.fingerprint.as_str())
            .is_none_or(|r| r.standing != Standing::Suppressed)
    });
    before - outcome.diagnostics.len()
}

/// Why a judged finding is reported anyway; `None` when nothing needs saying.
fn note(ruling: &Ruling) -> Option<String> {
    match ruling.standing {
        Standing::Suppressed | Standing::Judged => None,
        Standing::RuleChanged => Some("judgment expired: decision changed".to_owned()),
        Standing::EvidenceChanged => Some("judgment expired: evidence changed".to_owned()),
        Standing::Unsuppressible => Some(format!(
            "judged {} \u{2014} errors are definitive and no judgment hides them; fix the code or suppress it in the code",
            ruling.judgment
        )),
    }
}

fn announce(remembered: &mut Remembered) {
    if remembered.suppressed > 0 {
        remembered.messages.push(format!(
            "{} finding(s) kept out of the report by judgments (`lighthouse review list --status suppressed`)",
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
        ("decision changed", "judgment expired: decision changed"),
        ("evidence changed", "judgment expired: evidence changed"),
        (
            "definitive",
            "judged error(s) stay reported: errors are definitive and no judgment hides them; fix the code",
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
) -> Result<BTreeMap<String, Ruling>, lighthouse_store::Error> {
    let mut store = Store::open(root)?;
    store.record(&run_of(root, catalog, outcome))?;
    store.standings()
}

/// The judgments already recorded, when recording this run was not possible.
fn read_only(root: &Path) -> BTreeMap<String, Ruling> {
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
    record.authored_severity = authored_severity(d.severity, decision);
    record.options = options(decision, d, facts, outcome);
    let versions = decision.map(|d| seen.of(d));
    record.meaning_version = versions.map(|v| v.meaning.clone());
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
