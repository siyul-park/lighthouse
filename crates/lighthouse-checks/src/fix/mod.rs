//! Fixes as data: a decision's `fix:` block compiled into a provider of the
//! single `Fixer` interface, the way a declarative check compiles into a
//! `Rule`. A `cel` fix evaluates one expression that returns edits, lowered
//! to the edit IR. A `command` fix is run by the fix run itself, through the
//! one rewrite path it shares with an agent.

mod cel;
mod edits;

use std::sync::Arc;

use lighthouse_model::{Safety, ucm::FixOutcome};
use lighthouse_plugin::{Error as PluginError, FixRequest, FixerManifest};
use lighthouse_spec::{Decision, FixKind};

use crate::{Composition, Error};

/// The reason a fix gives when its expression returns no edit.
const NO_EDIT: &str = "the fix proposes no edit";

/// A fixer built from a decision's `fix:` block.
#[derive(Clone)]
pub(crate) struct Fixer {
    meta: FixerManifest,
    description: String,
    safety: Safety,
    fix: Arc<cel::CelFix>,
}

impl Fixer {
    /// `None` when the decision has no fix, or one of a kind another fixer
    /// serves or that is not yet supported; an error when it cannot compile.
    pub(crate) fn new(comp: &Composition, decision: &Decision) -> Result<Option<Self>, Error> {
        let Some(fix) = &decision.fix else {
            return Ok(None);
        };
        let compiled = match &fix.kind {
            FixKind::Cel { edits, .. } => cel::CelFix::new(comp, decision, edits)?,
            // The fix run itself serves a command and a model.
            FixKind::Command(_) | FixKind::Model { .. } => return Ok(None),
            // Not supported yet: the decision keeps its fix, no fixer is built, and
            // the plan declines its findings with that reason. The rest of the
            // pack is unaffected.
            FixKind::Rpc { .. } => return Ok(None),
        };
        Ok(Some(Self {
            meta: FixerManifest {
                id: decision.id().to_owned(),
                requires: fix.needs(comp.cel()),
            },
            description: decision.title.clone(),
            safety: fix.safety,
            fix: Arc::new(compiled),
        }))
    }
}

impl lighthouse_plugin::Fixer for Fixer {
    fn manifest(&self) -> &FixerManifest {
        &self.meta
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        Ok(match self.fix.lower(&self.meta.id, request)? {
            Ok(ops) if ops.is_empty() => FixOutcome::declined(NO_EDIT),
            Ok(ops) => FixOutcome::Proposed {
                description: self.description.clone(),
                ops,
                safety: self.safety,
            },
            Err(reason) => FixOutcome::declined(reason),
        })
    }
}
