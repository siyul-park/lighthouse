//! The fix orchestrator: the only component that writes. A rule judges, a
//! fixer proposes, the orchestrator executes: it asks the fixer the catalog
//! names for each selected finding, lowers the proposals to text edits, drops
//! overlapping ones, applies the rest, formats, re-indexes and re-checks, rolls
//! back every file that got worse and repeats until nothing is left to do.

mod diff;
mod edits;
mod lower;
mod preview;
mod run;
mod scan;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::Severity;
use lighthouse_model::{Fingerprint, Safety};
use lighthouse_plugin::FixDecision;
use lighthouse_spec::{Catalog, FixKind};

pub use diff::unified_diff;
pub use preview::{FixPreview, PreviewEdit};

use crate::Outcome;

/// How many apply-and-recheck rounds a run takes before it stops.
pub const MAX_ROUNDS: usize = 5;

/// How one rule's findings are fixed, as the catalog says.
#[derive(Debug, Clone)]
pub struct FixBinding {
    /// The registered fixer that serves the rule.
    pub fixer: String,
    /// What the fixer may claim at most.
    pub cap: Safety,
    /// The rule is mechanical: its safe fixes apply without asking.
    pub mechanical: bool,
    pub decision: FixDecision,
    /// Why the fix cannot run at all, such as a kind this version does not
    /// support; the rule's findings are then declined with this text.
    pub unsupported: Option<String>,
}

/// The fixer of each rule, by rule id. Built from the catalog by the caller:
/// the engine knows no decisions.
#[derive(Debug, Clone, Default)]
pub struct FixPlan {
    bindings: BTreeMap<String, FixBinding>,
    /// The rules whose fixer runs a program, which needs the project's trust.
    commands: BTreeSet<String>,
}

impl FixPlan {
    /// The fixer of every rule whose decision has a `fix`: the decision's id,
    /// under which its fixer is registered, with the cap its `safety` puts on
    /// what the fixer may claim.
    pub fn from_catalog(catalog: &Catalog) -> Self {
        let mut plan = Self::default();
        for decision in catalog.decisions() {
            let Some(fix) = &decision.fix else { continue };
            if matches!(fix.kind, FixKind::Command(_)) {
                plan.commands.insert(decision.id().to_owned());
            }
            plan.insert(
                decision.id(),
                FixBinding {
                    fixer: decision.id().to_owned(),
                    cap: fix.safety,
                    mechanical: decision.severity() == Some(Severity::Error),
                    decision: FixDecision {
                        id: decision.id().to_owned(),
                        requirement: decision.requirement.clone(),
                        intent: decision.intent.clone(),
                    },
                    unsupported: matches!(fix.kind, FixKind::Rpc { .. }).then(|| {
                        "the fix of this rule is of kind `rpc`, which is not yet supported"
                            .to_owned()
                    }),
                },
            );
        }
        plan
    }

    /// Names `binding` as the fix of `rule`.
    pub fn insert(&mut self, rule: impl Into<String>, binding: FixBinding) {
        self.bindings.insert(rule.into(), binding);
    }

    /// Whether the fixer of `rule` runs a program.
    pub fn runs_command(&self, rule: &str) -> bool {
        self.commands.contains(rule)
    }

    /// How the findings of `rule` are fixed, if they are.
    pub fn get(&self, rule: &str) -> Option<&FixBinding> {
        self.bindings.get(rule)
    }
}

/// What a fix run is asked to do.
#[derive(Debug, Clone, Default)]
pub struct FixRun {
    /// Fix findings under these paths; empty means the whole project.
    pub paths: Vec<PathBuf>,
    /// Fix findings of these rules; empty means every rule.
    pub rules: Vec<String>,
    /// Fix the findings with these fingerprints or unambiguous prefixes.
    pub fingerprints: Vec<String>,
    /// Findings to leave alone, such as those a verdict suppresses.
    pub skip: BTreeSet<Fingerprint>,
    /// Do everything but leave the files as they were.
    pub dry_run: bool,
    /// Also apply suggested fixes.
    pub unsafe_fixes: bool,
    /// Use this registered fixer for every selected finding instead of the
    /// catalog's; needs `rules` or `fingerprints`. Its proposals are
    /// suggestions at most.
    pub fixer: Option<String>,
    /// The project is trusted to run commands: fixers that run a program, and
    /// formatters. Without it they are declined or skipped.
    pub trusted: bool,
}

/// A fix that was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedFix {
    pub fingerprint: Fingerprint,
    pub rule_id: String,
    pub fixer: String,
    pub description: String,
    pub safety: Safety,
    /// Every file the fix changed.
    pub files: Vec<PathBuf>,
    pub round: usize,
}

/// A finding that was left as it was, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclinedFix {
    pub fingerprint: Fingerprint,
    pub rule_id: String,
    pub file: PathBuf,
    pub line: u32,
    pub reason: String,
}

/// A file's text before and after the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
}

/// What a fix run did.
#[derive(Debug, Default)]
pub struct FixReport {
    pub applied: Vec<AppliedFix>,
    pub declined: Vec<DeclinedFix>,
    /// Files whose text differs from the start, sorted by path. With
    /// `dry_run` they are what the run would have written.
    pub changes: Vec<FileChange>,
    /// Rounds that applied something.
    pub rounds: usize,
    /// What the user should know: files rolled back, rounds exhausted.
    pub notes: Vec<String>,
    /// The check of the state the run ended in. After a dry run it describes
    /// the fixed text, which is no longer on disk.
    pub finished: Outcome,
}
