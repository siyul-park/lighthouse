//! Fixes as data: a decision's `fix:` block compiled into a provider of the
//! single `Fixer` interface, the way a declarative check compiles into a
//! `Rule`. `ops` evaluates generic operations with CEL; `command` runs an
//! external program on a scratch copy.

mod command;
mod ops;

use std::sync::Arc;

use lighthouse_model::{FixOutcome, Safety};
use lighthouse_plugin::{Error as PluginError, FixRequest, Fixer, FixerManifest};
use lighthouse_spec::{CommandSpec, Decision, FixKind};

use crate::Error;

enum Compiled {
    Ops(ops::Steps),
    Command(CommandSpec),
}

/// A fixer built from a decision's `fix:` block.
#[derive(Clone)]
pub(crate) struct SpecFixer {
    meta: FixerManifest,
    description: String,
    safety: Safety,
    compiled: Arc<Compiled>,
}

impl SpecFixer {
    /// `None` when the decision has no fix, or one of a kind that is not yet
    /// supported; an error when it cannot compile.
    pub(crate) fn new(decision: &Decision) -> Result<Option<Self>, Error> {
        let Some(fix) = &decision.fix else {
            return Ok(None);
        };
        let compiled = match &fix.kind {
            FixKind::Ops { ops: list } => Compiled::Ops(ops::Steps::compile(decision.id(), list)?),
            FixKind::Command(command) => Compiled::Command(command.clone()),
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
            compiled: Arc::new(compiled),
        }))
    }
}

impl Fixer for SpecFixer {
    fn manifest(&self) -> &FixerManifest {
        &self.meta
    }

    fn fix(&self, request: &FixRequest) -> Result<FixOutcome, PluginError> {
        match self.compiled.as_ref() {
            Compiled::Ops(steps) => {
                let ops = steps.evaluate(&self.meta.id, request)?;
                Ok(match ops {
                    Ok(ops) => FixOutcome::Proposed {
                        description: self.description.clone(),
                        ops,
                        safety: self.safety,
                    },
                    Err(reason) => FixOutcome::declined(reason),
                })
            }
            Compiled::Command(spec) => {
                if !request.trusted {
                    return Ok(FixOutcome::declined(format!(
                        "`{}` fixes by running `{}`; this project is not trusted to run commands (run `lighthouse trust`, or set LIGHTHOUSE_TRUST=1 in CI)",
                        self.meta.id,
                        spec.argv.join(" ")
                    )));
                }
                command::run(&self.meta.id, spec, request).map(|outcome| match outcome {
                    Ok(ops) => FixOutcome::Proposed {
                        description: self.description.clone(),
                        ops,
                        safety: self.safety,
                    },
                    Err(reason) => FixOutcome::declined(reason),
                })
            }
        }
    }
}
