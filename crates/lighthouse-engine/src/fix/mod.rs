//! The fix orchestrator: the only component that writes. A rule judges, a
//! fixer proposes, the orchestrator executes: it asks the fixer the catalog
//! names for each selected finding, lowers the proposals to text edits, orders
//! them by priority and drops overlapping ones, applies the rest, re-indexes
//! and re-checks, applies the safe fixes of the findings the change
//! introduced, rolls back every file that got worse and repeats until nothing
//! is left to do.

mod batch;
pub mod brief;
mod command;
mod context;
mod edits;
mod kept;
mod lower;
mod model;
mod preview;
mod propose;
mod rewrite;
mod run;
mod scan;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use lighthouse_model::{Fingerprint, Preview, Safety, ucm::Capability};
use lighthouse_plugin::FixDecision;
use lighthouse_spec::{Catalog, Decision, FixKind, FixScope, Program, Task};
use serde::{Deserialize, Serialize};

pub use edits::unified_diff;
pub use model::ModelCounts;
use model::ModelFix;

use crate::{Needs, Run};

/// How many apply-and-recheck rounds a run takes before it stops.
pub const MAX_ROUNDS: usize = 5;

/// What makes a fix: a registered fixer, derived from the rule, or the
/// project's agent that edits, which guesses and is held to the checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixedBy {
    /// The registered fixer with this id.
    Fixer(String),
    /// The agent `[models] agent` binds.
    Model,
}

/// How one rule's findings are fixed, as the catalog says.
#[derive(Debug, Clone)]
pub struct FixBinding {
    /// What serves the rule.
    pub fixer: FixedBy,
    /// The effective safety of the fix at best, [`DecisionSpec::fix_safety`]: what
    /// the fixer claims can only be as safe as this, unless the project gives
    /// the rule a safety of its own (see [`FixPlan::with_safety`]).
    pub cap: Safety,
    /// The languages of the examples of the decision: where its fix is known
    /// to apply.
    pub languages: Vec<String>,
    /// What the fix needs to run: when [`Availability`] says it cannot, the
    /// rule's findings are declined with the reason.
    pub needs: Needs,
    pub decision: FixDecision,
}

/// The fix by a command of a rule: the program the fix run itself runs for a
/// finding, through the rewrite path it shares with an agent.
#[derive(Debug, Clone)]
pub(super) struct CommandFix {
    program: Program,
    /// How far its change may go.
    scope: FixScope,
    /// What the language of the file must provide for the fix to be made.
    requires: Vec<Capability>,
    /// What the fix does, in the words of the decision's title.
    description: String,
}

/// The fixer of each rule, by rule id. Built from the catalog by the caller:
/// the engine knows no decisions.
#[derive(Debug, Clone, Default)]
pub struct FixPlan {
    bindings: BTreeMap<String, FixBinding>,
    /// What the rules whose fix is by a model add to the brief, and how far
    /// they may go.
    models: BTreeMap<String, ModelFix>,
    /// The rules whose fix is a command the run runs itself.
    commands: BTreeMap<String, CommandFix>,
    /// The priority of the fix of the rules that set one: the higher goes
    /// first when proposals of a round overlap.
    priorities: BTreeMap<String, i32>,
    /// The safety the project gives the fixes of a rule, whoever makes them.
    configured: BTreeMap<String, Safety>,
}

impl FixPlan {
    /// The fixer of every rule whose decision has a `fix` that is not by a
    /// model: the decision's id, under which its fixer is registered (a
    /// command is run by the fix run itself), with the cap its `safety` puts
    /// on what the fixer may claim.
    pub fn from_catalog(catalog: &Catalog) -> Self {
        let mut plan = Self::default();
        for decision in catalog.decisions() {
            let Some(fix) = &decision.fix else { continue };
            if matches!(fix.kind, FixKind::Model { .. }) {
                continue;
            }
            plan.insert(
                decision.id(),
                FixBinding {
                    fixer: FixedBy::Fixer(decision.id().to_owned()),
                    cap: decision.fix_safety(None),
                    languages: languages_of(decision),
                    needs: match fix.kind {
                        FixKind::Command(_) if decision.bundled() => Needs::Bundled,
                        FixKind::Command(_) => Needs::Program,
                        FixKind::Rpc { .. } => Needs::Unserved,
                        _ => Needs::Nothing,
                    },
                    decision: decision_of(decision),
                },
            );
            plan.prioritize(decision.id(), fix.priority);
            if let FixKind::Command(command) = &fix.kind {
                plan.commands.insert(
                    decision.id().to_owned(),
                    CommandFix {
                        program: command.program.clone(),
                        scope: FixScope::of_fix(decision.fix.as_ref(), FixScope::Files),
                        requires: fix.requires.clone(),
                        description: decision.title.clone(),
                    },
                );
            }
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
                    cap: decision.fix_safety(None),
                    languages: Vec::new(),
                    needs: Needs::Model {
                        decision: decision.id().to_owned(),
                        task: Task::Agent,
                    },
                    decision: decision_of(decision),
                },
            );
            self.models.insert(
                decision.id().to_owned(),
                ModelFix::of(decision.fix.as_ref(), &decision.examples, cap),
            );
            self.prioritize(
                decision.id(),
                decision.fix.as_ref().map_or(0, |f| f.priority),
            );
        }
        self
    }

    /// Gives the fixes of the rules of `safety` the safety the project says,
    /// whoever makes them and whatever the decision and the provider claim:
    /// the last word of the effective safety of a fix.
    #[must_use]
    pub fn with_safety(mut self, safety: impl IntoIterator<Item = (String, Safety)>) -> Self {
        for (rule, safe) in safety {
            if let Some(binding) = self.bindings.get_mut(&rule) {
                binding.cap = safe;
            }
            self.configured.insert(rule, safe);
        }
        self
    }

    fn prioritize(&mut self, rule: &str, priority: i32) {
        if priority != 0 {
            self.priorities.insert(rule.to_owned(), priority);
        }
    }

    /// The priority of the fix of `rule`: the higher goes first.
    pub(super) fn priority(&self, rule: &str) -> i32 {
        self.priorities.get(rule).copied().unwrap_or_default()
    }

    /// The safety of a fix of `rule` that its fixer says is `claimed`, under
    /// the `cap` of its decision: the project's word if it gave one, else the
    /// less safe of the two.
    pub(super) fn safety(&self, rule: &str, claimed: Safety, cap: Safety) -> Safety {
        self.configured
            .get(rule)
            .copied()
            .unwrap_or_else(|| claimed.max(cap))
    }

    /// The command the run itself runs to fix `rule`, if its fix is one.
    pub(super) fn command(&self, rule: &str) -> Option<&CommandFix> {
        self.commands.get(rule)
    }

    /// Names `binding` as the fix of `rule`.
    pub fn insert(&mut self, rule: impl Into<String>, binding: FixBinding) {
        self.bindings.insert(rule.into(), binding);
    }

    /// The rules with a fix, and how each is fixed, in the order of their ids.
    pub fn bindings(&self) -> impl Iterator<Item = (&str, &FixBinding)> {
        self.bindings.iter().map(|(rule, b)| (rule.as_str(), b))
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
    /// Whether `unsafe` fixes are written: `Some(true)` as `--unsafe-fixes`
    /// asks, `Some(false)` as `--no-unsafe-fixes` does, `None` as the
    /// project's `[fix] unsafe` says.
    pub unsafe_fixes: Option<bool>,
    /// Use this registered fixer for every selected finding instead of the
    /// catalog's; needs `rules` or `fingerprints`. Its proposals are
    /// `unsafe`.
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
    /// What the fix changes, kept only in a dry run, where no fix is written:
    /// `None` when it could not be shown.
    pub preview: Option<Preview>,
    /// Whether it is a safe fix of what the change of others introduced, in a
    /// file several of them edited: what it writes belongs to no one of them.
    /// In a file one fix edited it is part of that fix.
    pub follow_up: bool,
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

/// An `unsafe` fix that the checks accepted and the run was not permitted to
/// write, kept for a hand to apply: only if the files are still as it was
/// verified over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewedFix {
    pub rule_id: String,
    pub fixer: FixedBy,
    pub preview: Preview,
    /// The digest of each file the fix changes, as it was verified over.
    pub bases: BTreeMap<PathBuf, String>,
    /// The findings the fix was made for, this one among them: a run of a
    /// model is made for all the findings of a file, and applied for them
    /// all.
    pub batch: Vec<Fingerprint>,
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
    /// The `unsafe` fixes, by any provider, that the checks accepted and the
    /// run left for a hand to apply: no `[fix] unsafe` and no `unsafe_fixes`,
    /// or a failing `[fix] verify` program, withheld them. With `dry_run` they
    /// are the same.
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

/// The languages of the examples of `decision`, sorted.
fn languages_of(decision: &Decision) -> Vec<String> {
    let languages: BTreeSet<&str> = decision
        .examples
        .iter()
        .map(|e| e.language.as_str())
        .collect();
    languages.into_iter().map(str::to_owned).collect()
}

/// The decision as a fixer is told about it.
fn decision_of(decision: &Decision) -> FixDecision {
    FixDecision {
        id: decision.id().to_owned(),
        requirement: decision.requirement.clone(),
        context: decision.context.clone(),
    }
}
