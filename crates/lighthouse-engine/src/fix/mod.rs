//! The fix orchestrator: the only component that writes. A rule judges, a
//! fixer proposes, the orchestrator executes: it asks the fixer the catalog
//! names for each selected finding, lowers the proposals to text edits, drops
//! overlapping ones, applies the rest, formats, re-indexes and re-checks, rolls
//! back every file that got worse and repeats until nothing is left to do.

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
use lighthouse_spec::{Catalog, Decision, FixKind};

pub use edits::unified_diff;
pub use model::ModelCounts;
use model::ModelFix;

use crate::Run;

/// How many apply-and-recheck rounds a run takes before it stops.
pub const MAX_ROUNDS: usize = 5;

/// What makes a fix: a registered fixer, derived from the rule, or the
/// project's chat model, which guesses and is held to the checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixedBy {
    /// The registered fixer with this id.
    Fixer(String),
    /// The model `[models] chat` binds.
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
    /// What the rules whose fix is by a model add to the prompt.
    models: BTreeMap<String, ModelFix>,
}

impl FixPlan {
    /// The fixer of every rule whose decision has a `fix`: the decision's id,
    /// under which its fixer is registered, with the cap its `safety` puts on
    /// what the fixer may claim.
    pub fn from_catalog(catalog: &Catalog) -> Self {
        let mut plan = Self::default();
        for decision in catalog.decisions() {
            let Some(fix) = &decision.fix else { continue };
            let model = ModelFix::of(Some(fix), &decision.examples, false);
            plan.insert(
                decision.id(),
                FixBinding {
                    fixer: if model.is_some() {
                        FixedBy::Model
                    } else {
                        FixedBy::Fixer(decision.id().to_owned())
                    },
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
            plan.models
                .extend(model.map(|m| (decision.id().to_owned(), m)));
        }
        plan
    }

    /// Also gives every rule that has a check and no fix of its own the fix
    /// by a model, which is asked when the project binds a chat model.
    #[must_use]
    pub fn with_model_fixes(mut self, catalog: &Catalog) -> Self {
        for decision in catalog.decisions() {
            if decision.check.is_none() || decision.fix.is_some() {
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
            self.models.extend(
                ModelFix::of(None, &decision.examples, true).map(|m| (decision.id().to_owned(), m)),
            );
        }
        self
    }

    /// Names `binding` as the fix of `rule`.
    pub fn insert(&mut self, rule: impl Into<String>, binding: FixBinding) {
        self.bindings.insert(rule.into(), binding);
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
