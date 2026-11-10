//! The report of a run as the graph keeps it: what the run is besides the
//! files (its state), asked of `report(scope)` before any rule is, and stated
//! to it after the rules have run. A run that no input has moved since is the
//! report the graph holds; one that something moved runs as it always did.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_plugin::{Read, quiet_report, set_run, state_report};
use lighthouse_protocol::hash::Hasher;
use serde::{Deserialize, Serialize};

use super::*;
use crate::{cache, tools};

/// What a run reports, as it is kept.
#[derive(Serialize, Deserialize)]
struct Body {
    diagnostics: Vec<Diagnostic>,
    notices: BTreeSet<String>,
    incomplete: Vec<Incomplete>,
    reported: Vec<PathBuf>,
    ran: Vec<String>,
    facts: Vec<(Fingerprint, Value)>,
    options: Vec<(Fingerprint, Options)>,
    active: Vec<String>,
    reviews: Vec<Diagnostic>,
    suppressed: Vec<Suppressed>,
}

/// What a run is besides the files it reads, and the report it asks for.
pub(super) struct Reporting {
    /// The reported paths: the node of the report.
    scope: String,
    /// What the run is made of, apart from the files and the index.
    basis: String,
}

impl Engine {
    /// The report `selected` over `scopes` asks for, or `None` when what it
    /// reports is not a function of the graph: a rule asks a model.
    pub(super) fn reporting(
        &self,
        selected: &[&dyn Rule],
        (scopes, only): (&Scopes, &[String]),
        (models, run): (&dyn lighthouse_plugin::Models, &Run),
    ) -> Option<Reporting> {
        let asks = selected
            .iter()
            .any(|rule| rule.reviews() && models.asks(&rule.manifest().id));
        if asks || self.models.collect {
            return None;
        }
        let mut hasher = Hasher::new();
        hasher.update(cache::build_identity());
        hasher.update(format!(
            "{only:?}{:?}{:?}{:?}{}",
            run.incomplete, self.active, self.models, self.trusted
        ));
        hasher.update(&self.setup);
        let tools: BTreeSet<String> = selected
            .iter()
            .flat_map(|rule| rule.manifest().tools.iter().cloned())
            .collect();
        hasher.update(tools::state(&tools.into_iter().collect::<Vec<_>>()));
        for rule in selected {
            hasher.update(self.environment(*rule));
        }
        Some(Reporting {
            scope: format!("{scopes:?}"),
            basis: hasher.hex(),
        })
    }

    /// Sets the state of the run and puts in `run` the report the graph has
    /// for it, when no input has moved since that report was verified.
    pub(super) fn recall(&self, reporting: &Reporting, tree: &str, run: &mut Run) -> bool {
        let mut guard = self.lock_graph();
        let Some(graph) = guard.as_mut() else {
            return false;
        };
        let state = reporting.state(tree, &run.timings.answers);
        if set_run(graph, state).is_err() {
            return false;
        }
        let Ok(Some(body)) = quiet_report(graph, &reporting.scope) else {
            return false;
        };
        let Ok(kept) = serde_json::from_str::<Body>(&body) else {
            return false;
        };
        run.diagnostics = kept.diagnostics;
        run.notices = kept.notices;
        run.incomplete = kept.incomplete;
        run.reported = kept.reported;
        run.ran = kept.ran;
        run.facts = kept.facts.into_iter().collect::<BTreeMap<_, _>>();
        run.options = kept.options.into_iter().collect::<BTreeMap<_, _>>();
        run.active = kept.active;
        run.reviews = kept.reviews;
        run.suppressed = kept.suppressed;
        true
    }

    /// States what `run` reports to the graph, made of the findings `reads`.
    /// A report that cannot be stated is not kept, and the next run is asked.
    pub(super) fn remember(
        &self,
        graph: &Graph,
        reporting: &Reporting,
        reads: Vec<Read>,
        run: &Run,
    ) {
        let body = Body {
            diagnostics: run.diagnostics.clone(),
            notices: run.notices.clone(),
            incomplete: run.incomplete.clone(),
            reported: run.reported.clone(),
            ran: run.ran.clone(),
            facts: run.facts.clone().into_iter().collect(),
            options: run.options.clone().into_iter().collect(),
            active: run.active.clone(),
            reviews: run.reviews.clone(),
            suppressed: run.suppressed.clone(),
        };
        let Ok(text) = serde_json::to_string(&body) else {
            return;
        };
        let _ = state_report(graph, &reporting.scope, (reads, text));
    }
}

impl Reporting {
    /// The state of the run on a project of root hash `tree` whose providers
    /// answered as `index`.
    fn state(&self, tree: &str, index: &str) -> String {
        let mut hasher = Hasher::new();
        hasher.update(&self.basis);
        hasher.update(tree);
        hasher.update(index);
        hasher.hex()
    }
}
