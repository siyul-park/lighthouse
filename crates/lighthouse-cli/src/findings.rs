//! The quality store as `check` uses it: record what a run saw, then keep out
//! of the report the findings that review verdicts rejected.

use std::{collections::BTreeSet, path::Path};

use lighthouse_engine::Outcome;
use lighthouse_store::{Observed, Run, Store};
use serde_json::json;

/// Records the run in the project's store and removes the findings whose
/// latest verdict is a rejection from `outcome`; returns how many it removed.
/// A store that cannot be used never fails the check: it is reported and the
/// run goes on without memory.
pub fn remember(root: &Path, outcome: &mut Outcome) -> usize {
    match record(root, outcome) {
        Ok(suppressed) => {
            let before = outcome.diagnostics.len();
            outcome
                .diagnostics
                .retain(|d| !suppressed.contains(d.fingerprint.as_str()));
            before - outcome.diagnostics.len()
        }
        Err(e) => {
            eprintln!("lighthouse: findings not recorded: {e}");
            0
        }
    }
}

fn record(root: &Path, outcome: &Outcome) -> Result<BTreeSet<String>, lighthouse_store::Error> {
    let mut store = Store::open(&Store::path_in(root))?;
    store.record(&run_of(outcome))?;
    store.suppressed()
}

/// What the store needs to know about a run: its findings with their facts,
/// the paths and rules it covered, and whether it saw everything.
fn run_of(outcome: &Outcome) -> Run {
    let observed = outcome.diagnostics.iter().map(|d| {
        let facts = outcome.facts.get(&d.fingerprint).cloned();
        Observed::from_diagnostic(d, facts.unwrap_or_else(|| json!({})))
    });
    Run {
        observed: observed.collect(),
        reported: outcome
            .reported
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
        rules: outcome.rules.clone(),
        complete: outcome.incomplete.is_empty(),
    }
}
