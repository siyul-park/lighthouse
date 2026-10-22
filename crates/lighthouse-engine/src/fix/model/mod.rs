//! The fix by a model: an agent that edits. For the findings of a file, a
//! program runs in a scratch copy of the project with the brief as its
//! prompt (see [`edit`]); what it changed is a proposal, which the
//! orchestrator applies in memory and keeps only if the checks accept it (see
//! [`verify`]). A model's word is never taken for the result, and what it may
//! change is limited by the scope of the fix (see [`super::rewrite`], the path
//! it shares with a command fix).

mod account;
mod edit;
mod extent;
mod reach;
mod trace;
mod verify;

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

use lighthouse_spec::{Example, Fix, FixKind, FixScope};

use super::brief;
use crate::{
    Engine,
    engine::{Route, Usage},
};
pub use account::ModelCounts;
pub(super) use account::{Account, Class, Event};
pub(super) use extent::choose as choose_extent;
pub use trace::{BatchTrace, ToolCounts};
pub(super) use verify::{Verdict, judge};

/// How many findings of one file an agent is given at most.
pub(super) const MAX_FINDINGS: usize = 20;

/// What a decision tells the agent besides its requirement, and how far the
/// agent may go.
#[derive(Debug, Clone)]
pub(super) struct ModelFix {
    /// The `prompt` of the fix.
    instruction: Option<String>,
    /// Examples that show a fix, as the brief shows them.
    examples: Vec<Example>,
    /// The widest change allowed: the decision's own `scope`, narrowed by the
    /// project's.
    scope: FixScope,
}

impl ModelFix {
    /// The fix by a model of a decision whose `fix` is `fix`, if any, with
    /// `cap` the widest scope the project allows.
    pub(super) fn of(fix: Option<&Fix>, examples: &[Example], cap: FixScope) -> Self {
        let instruction = match fix.map(|f| &f.kind) {
            Some(FixKind::Model { prompt, .. }) => prompt.clone(),
            _ => None,
        };
        Self {
            instruction,
            examples: brief::examples_for(examples),
            scope: FixScope::of_fix(fix, cap),
        }
    }
}

/// The fixer of the findings given to a model: asks the agent each decision
/// is bound to (`[models] agent`, or its own `agent`) and counts what happens.
pub(super) struct ModelFixer<'e> {
    route: Route<'e>,
    fixes: &'e BTreeMap<String, ModelFix>,
    pub(super) account: Account,
    /// What the agents said, for the notes of the run.
    said: Mutex<Vec<String>>,
    /// What the agents did, by run, for the report.
    traces: Mutex<Vec<BatchTrace>>,
    /// The tokens of context a brief may carry around the findings.
    context: usize,
}

impl<'e> ModelFixer<'e> {
    /// The fixer of the model fixes of `fixes`; `None` when the run has none
    /// or no agent to ask.
    pub(super) fn new(engine: &'e Engine, fixes: &'e BTreeMap<String, ModelFix>) -> Option<Self> {
        if fixes.is_empty() {
            return None;
        }
        Some(Self {
            route: engine.agent_route()?,
            fixes,
            account: Account::default(),
            said: Mutex::default(),
            traces: Mutex::default(),
            context: engine.config.fix().context,
        })
    }

    /// The agents that fix `decision`, in the order they are asked: the ones
    /// it binds, else the project's. Empty when none is bound to it.
    pub(super) fn agent_for(&self, decision: &str) -> &[String] {
        self.route.agent_for(decision)
    }

    /// How far the fix of `decision` may go.
    pub(super) fn scope_of(&self, decision: &str) -> FixScope {
        self.fixes
            .get(decision)
            .map_or_else(FixScope::default, |fix| fix.scope)
    }

    /// What the requests that went to a model this run took.
    pub(super) fn usage(&self) -> Usage {
        self.route.usage()
    }

    /// What the agents did in the runs that were made now, in the order they
    /// finished; a kept answer has no events.
    pub(super) fn traces(&self) -> Vec<BatchTrace> {
        self.traces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What the user should know about the route and the agents, once the run
    /// is over.
    pub(super) fn finish(&self) -> Vec<String> {
        let said = self.said.lock().unwrap_or_else(PoisonError::into_inner);
        self.route
            .finish()
            .into_iter()
            .chain(said.iter().cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod chain_tests;

#[cfg(test)]
mod edit_tests;
