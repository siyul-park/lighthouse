//! The fix orchestrator: the only component that writes. A rule judges, a
//! fixer proposes, the orchestrator executes: it asks the fixer the catalog
//! names for each selected finding, lowers the proposals to text edits, drops
//! overlapping ones, applies the rest, formats, re-indexes and re-checks, rolls
//! back every file that got worse and repeats until nothing is left to do.

mod batch;
pub mod brief;
mod edits;
mod lower;
mod model;
mod preview;
mod propose;
mod run;
mod scan;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
};

use lighthouse_model::Severity;
use lighthouse_model::{Fingerprint, Preview, Safety};
use lighthouse_plugin::FixDecision;
use lighthouse_spec::{Catalog, Decision, FixKind, FixScope};

pub use edits::unified_diff;
pub use model::ModelCounts;
use model::ModelFix;

use crate::Run;

/// How many apply-and-recheck rounds a run takes before it stops.
pub const MAX_ROUNDS: usize = 5;

/// What makes a fix: a registered fixer, derived from the rule, or the
/// project's agent that edits, which guesses and is held to the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixedBy {
    /// The registered fixer with this id.
    Fixer(String),
    /// The agent `[models] edit` binds.
    Model,
}

impl fmt::Display for FixedBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixer(id) => f.write_str(id),
            Self::Model => f.write_str("lighthouse/model"),
        }
    }
}

/// How one rule's findings are fixed, as the catalog says.
#[derive(Debug, Clone)]
pub struct FixBinding {
    /// What serves the rule.
    pub fixer: FixedBy,
    /// What the fixer may claim at most.
    pub cap: Safety,
    /// The rule is mechanical: its safe fixes apply without asking.
    pub mechanical: bool,
    /// The fixer runs a program, which needs the project's trust.
    pub runs_command: bool,
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
    /// What the rules whose fix is by a model add to the brief, and how far
    /// they may go.
    models: BTreeMap<String, ModelFix>,
    /// The decisions with a safe fix of their own, and the languages of their
    /// examples: the project applies those fixes to what an agent leaves.
    safe: BTreeMap<String, BTreeSet<String>>,
}

impl FixPlan {
    /// The fixer of every rule whose decision has a `fix` that is not by a
    /// model: the decision's id, under which its fixer is registered, with
    /// the cap its `safety` puts on what the fixer may claim.
    pub fn from_catalog(catalog: &Catalog) -> Self {
        let mut plan = Self::default();
        for decision in catalog.decisions() {
            let Some(fix) = &decision.fix else { continue };
            if matches!(fix.kind, FixKind::Model { .. }) {
                continue;
            }
            if fix.safety == Safety::Safe && !matches!(fix.kind, FixKind::Rpc { .. }) {
                let languages = decision.examples.iter().map(|e| e.language.clone());
                plan.safe
                    .insert(decision.id().to_owned(), languages.collect());
            }
            plan.insert(
                decision.id(),
                FixBinding {
                    fixer: FixedBy::Fixer(decision.id().to_owned()),
                    cap: fix.safety,
                    mechanical: decision.severity == Some(Severity::Error),
                    runs_command: matches!(fix.kind, FixKind::Command(_)),
                    decision: decision_of(decision),
                    unsupported: matches!(fix.kind, FixKind::Rpc { .. }).then(|| {
                        "the fix of this rule is of kind `rpc`, which is not yet supported"
                            .to_owned()
                    }),
                },
            );
        }
        plan
    }

    /// Also gives the rules of `permitted` (the decisions the project lets a
    /// model fix) the fix by a model, when they have a check and no fix of
    /// their own, or a fix of type `model`. `cap` is the widest scope the
    /// project allows any of them.
    #[must_use]
    pub fn with_model_fixes(
        mut self,
        catalog: &Catalog,
        permitted: &BTreeSet<String>,
        cap: FixScope,
    ) -> Self {
        for decision in catalog.decisions() {
            let by_model = decision
                .fix
                .as_ref()
                .is_none_or(|fix| matches!(fix.kind, FixKind::Model { .. }));
            if decision.check.is_none() || !by_model || !permitted.contains(decision.id()) {
                continue;
            }
            self.insert(
                decision.id(),
                FixBinding {
                    fixer: FixedBy::Model,
                    cap: Safety::Suggested,
                    mechanical: false,
                    runs_command: false,
                    decision: decision_of(decision),
                    unsupported: None,
                },
            );
            self.models.insert(
                decision.id().to_owned(),
                ModelFix::of(decision.fix.as_ref(), &decision.examples, cap),
            );
        }
        self
    }

    /// Names `binding` as the fix of `rule`.
    pub fn insert(&mut self, rule: impl Into<String>, binding: FixBinding) {
        self.bindings.insert(rule.into(), binding);
    }

    /// The decisions whose safe fixes the project applies after an agent, for
    /// files in `language` (every language when `None`), in the order of
    /// their ids.
    pub fn safe_fixes(&self, language: Option<&str>) -> impl Iterator<Item = &str> {
        self.safe
            .iter()
            .filter(move |(_, languages)| language.is_none_or(|l| languages.contains(l)))
            .map(|(id, _)| id.as_str())
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
    /// Findings to leave alone, such as those a judgment hides.
    pub skip: BTreeSet<Fingerprint>,
    /// Do everything but leave the files as they were.
    pub dry_run: bool,
    /// Also apply suggested fixes.
    pub unsafe_fixes: bool,
    /// Use this registered fixer for every selected finding instead of the
    /// catalog's; needs `rules` or `fingerprints`. Its proposals are
    /// suggestions at most.
    pub fixer: Option<String>,
}

/// A fix that was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedFix {
    pub fingerprint: Fingerprint,
    pub rule_id: String,
    pub fixer: FixedBy,
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

/// A fix by a model that the checks accepted and the run left for a hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewedFix {
    pub rule_id: String,
    pub preview: Preview,
}

/// What the runs of an agent that edits took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditUsage {
    /// Runs of the agent; answers kept from earlier runs are not counted.
    pub runs: usize,
    pub seconds: f64,
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
    /// The fixes by a model that the checks accepted and the run left for a
    /// hand to apply: the project's approval, or a failing `[fix] verify`
    /// command, withheld them. With `dry_run` they are the same.
    pub previews: Vec<PreviewedFix>,
    /// What became of the findings given to a model, by decision; empty when
    /// no model was asked.
    pub models: BTreeMap<String, ModelCounts>,
    /// Requests that went to a model this run (kept answers are not counted),
    /// and what they cost.
    pub model_calls: usize,
    pub model_cost: f64,
    /// Present when the model is an agent that edits: its runs and the time
    /// they took.
    pub model_edits: Option<EditUsage>,
    /// The check of the state the run ended in. After a dry run it describes
    /// the fixed text, which is no longer on disk.
    pub finished: Run,
}

/// The decision as a fixer is told about it.
fn decision_of(decision: &Decision) -> FixDecision {
    FixDecision {
        id: decision.id().to_owned(),
        requirement: decision.requirement.clone(),
        context: decision.context.clone(),
    }
}
