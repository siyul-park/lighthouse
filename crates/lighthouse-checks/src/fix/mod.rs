//! Fixes as data: a decision's `fix:` block compiled into a provider of the
//! single `Fixer` interface, the way a declarative check compiles into a
//! `Rule`. `ops` evaluates generic operations with CEL. A `command` fix is
//! run by the fix run itself, through the one rewrite path it shares with an
//! agent.

mod ops;

use std::sync::Arc;

use lighthouse_model::{Safety, ucm::FixOutcome};
use lighthouse_plugin::{Error as PluginError, FixRequest, FixerManifest};
use lighthouse_spec::{Decision, FixKind};

use crate::Error;

/// A fixer built from a decision's `fix:` block.
#[derive(Clone)]
pub(crate) struct Fixer {
    meta: FixerManifest,
    description: String,
    safety: Safety,
    steps: Arc<ops::Steps>,
}

impl Fixer {
    /// `None` when the decision has no fix, or one of a kind another fixer
    /// serves or that is not yet supported; an error when it cannot compile.
    pub(crate) fn new(decision: &Decision) -> Result<Option<Self>, Error> {
        let Some(fix) = &decision.fix else {
            return Ok(None);
        };
        let steps = match &fix.kind {
            FixKind::Ops { ops: list } => ops::Steps::compile(decision.id(), list)?,
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
                requires: fix.requires.clone(),
            },
            description: decision.title.clone(),
            safety: fix.safety,
            steps: Arc::new(steps),
        }))
    }
}

impl lighthouse_plugin::Fixer for Fixer {
    fn manifest(&self) -> &FixerManifest {
        &self.meta
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        let evaluated = self.steps.evaluate(&self.meta.id, request)?;
        Ok(match evaluated {
            Ok(ops) => FixOutcome::Proposed {
                description: self.description.clone(),
                ops,
                safety: self.safety,
            },
            Err(reason) => FixOutcome::declined(reason),
        })
    }
}
