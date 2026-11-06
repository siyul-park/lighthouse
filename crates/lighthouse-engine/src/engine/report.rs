//! The report of a run that nothing has moved since: when the project's root
//! hash and everything else a run depends on are those of the last run that
//! checked it, the run is what that one reported, and no rule is asked for it.
//! What the run depends on is hashed into a key kept with the report.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use lighthouse_protocol::hash::Hasher;
use serde::{Deserialize, Serialize};

use super::*;
use crate::{cache, tools};

/// How many reports are kept, in the cache directory, one file for each key:
/// a project edited and put back is a project that was reported.
const KEPT: usize = 4;

/// What a run reports, as it is kept.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    key: String,
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

/// What a run depends on besides the files it reads, hashed.
pub(super) struct Basis(String);

impl Basis {
    /// The key of the run on a project whose root hash is `tree`.
    pub(super) fn key(&self, tree: &str) -> String {
        let mut hasher = Hasher::new();
        hasher.update(&self.0);
        hasher.update(tree);
        hasher.hex()
    }
}

impl Engine {
    /// What the run of `selected` over `scopes` depends on besides the files,
    /// or `None` when its report is not a function of them: a rule asks a
    /// model, or the run is read with texts in place of files.
    pub(super) fn basis(
        &self,
        selected: &[&dyn Rule],
        (scopes, only): (&Scopes, &[String]),
        (models, run): (&dyn lighthouse_plugin::Models, &Run),
    ) -> Option<Basis> {
        let asks = selected
            .iter()
            .any(|rule| rule.reviews() && models.asks(&rule.manifest().id));
        if asks || self.models.collect {
            return None;
        }
        let mut hasher = Hasher::new();
        hasher.update(cache::build_identity());
        hasher.update(format!(
            "{scopes:?}{only:?}{:?}{:?}{:?}{}",
            run.incomplete, self.active, self.models, self.trusted
        ));
        hasher.update(format!("{:?}", self.config));
        let tools: BTreeSet<String> = selected
            .iter()
            .flat_map(|rule| rule.manifest().tools.iter().cloned())
            .collect();
        hasher.update(tools::state(&tools.into_iter().collect::<Vec<_>>()));
        for rule in selected {
            hasher.update(self.environment(*rule));
        }
        Some(Basis(hasher.hex()))
    }

    /// The report kept for `key`, put in `run`; whether there was one.
    pub(super) fn recall(&self, key: &str, run: &mut Run) -> bool {
        let Some(settings) = &self.cache else {
            return false;
        };
        let Ok(text) = fs::read(file_of(&settings.dir, key)) else {
            return false;
        };
        let Ok(kept) = serde_json::from_slice::<Snapshot>(&text) else {
            return false;
        };
        if kept.key != key {
            return false;
        }
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

    /// Keeps what `run` reports for `key`. A report that cannot be written is
    /// not kept, and the next run is asked.
    pub(super) fn remember(&self, key: &str, run: &Run) {
        let Some(settings) = &self.cache else { return };
        let kept = Snapshot {
            key: key.to_owned(),
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
        let Ok(text) = serde_json::to_vec(&kept) else {
            return;
        };
        let file = file_of(&settings.dir, key);
        let part = file.with_extension("part");
        if fs::write(&part, text).is_ok() && fs::rename(&part, &file).is_err() {
            let _ = fs::remove_file(&part);
        }
        prune(&settings.dir);
    }
}

/// The file the report of `key` is kept in.
fn file_of(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!(
        "report-{}.json",
        key.chars().take(16).collect::<String>()
    ))
}

/// Removes the reports beyond the newest [`KEPT`].
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("report-"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    kept.sort();
    let extra = kept.len().saturating_sub(KEPT);
    for (_, path) in kept.into_iter().take(extra) {
        let _ = fs::remove_file(path);
    }
}
