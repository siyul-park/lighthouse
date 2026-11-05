use std::fmt;

use crate::{
    Revision,
    node::{Cause, Locked, NodeId},
    shared::Shared,
};

/// Why a node ran on its last pull, from the node down to the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation(pub Vec<Step>);

/// One node of an [`Explanation`] and what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub node: String,
    pub reason: Reason,
}

/// What happened to a node on its last pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// There was nothing stored of it.
    Computed,
    /// Its value was dropped, and the pull needed it.
    Evicted,
    /// A dependency's hash moved.
    DepMoved(String),
    /// A dependency is of a kind that is not registered, or has no node.
    UnknownDep(String),
    /// An input set to a value of another hash.
    InputChanged(Revision),
    /// It was verified without running, and last moved at this revision.
    Unchanged(Revision),
}

impl fmt::Display for Explanation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for step in &self.0 {
            writeln!(f, "{}: {}", step.node, step.reason)?;
        }
        Ok(())
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Computed => write!(f, "computed for the first time"),
            Self::Evicted => write!(f, "its value was evicted"),
            Self::DepMoved(dep) => write!(f, "{dep} changed"),
            Self::UnknownDep(dep) => write!(f, "{dep} cannot be validated"),
            Self::InputChanged(revision) => write!(f, "set to a new value at {revision}"),
            Self::Unchanged(revision) => write!(f, "did not run; last changed at {revision}"),
        }
    }
}

/// The steps from `id` down to the input that moved.
pub(crate) fn walk(shared: &Shared, mut id: NodeId) -> Option<Explanation> {
    let mut steps = Vec::new();
    loop {
        let slot = shared.slot_if_known(&id);
        let Some(slot) = slot else {
            return (!steps.is_empty()).then_some(Explanation(steps));
        };
        let (cause, changed_at) = {
            let node = slot.node.locked();
            if !node.present {
                return None;
            }
            (node.cause.clone(), Revision(node.changed_at))
        };
        let node = shared.label(&id);
        let (reason, next) = reason_of(shared, cause, changed_at);
        steps.push(Step { node, reason });
        match next {
            Some(dep) => id = dep,
            None => return Some(Explanation(steps)),
        }
    }
}

fn reason_of(shared: &Shared, cause: Cause, changed_at: Revision) -> (Reason, Option<NodeId>) {
    match cause {
        Cause::Computed => (Reason::Computed, None),
        Cause::Evicted => (Reason::Evicted, None),
        Cause::InputChanged => (Reason::InputChanged(changed_at), None),
        Cause::Green => (Reason::Unchanged(changed_at), None),
        Cause::UnknownDep(dep) => (Reason::UnknownDep(shared.label(&dep)), None),
        Cause::DepMoved(dep) => (Reason::DepMoved(shared.label(&dep)), Some(dep)),
    }
}
