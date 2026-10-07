//! Fixes as data: a pattern's `fix:` block compiled into a provider of the
//! single `Fixer` interface, the way a declarative rule file compiles into a
//! `Rule`. `ops` evaluates generic operations with CEL; `command` runs an
//! external program on a scratch copy.

mod command;
mod ops;

use std::sync::Arc;

use lighthouse_model::{FixOutcome, Safety};
use lighthouse_plugin::{Error as PluginError, FixRequest, Fixer, FixerManifest};
use lighthouse_spec::{CommandSpec, FixKind, Pattern};

use crate::Error;

enum Compiled {
    Ops(ops::Steps),
    Command(CommandSpec),
}

/// A fixer built from a pattern's `fix:` block.
pub(crate) struct SpecFixer {
    meta: FixerManifest,
    description: String,
    safety: Safety,
    compiled: Arc<Compiled>,
}

impl SpecFixer {
    /// `None` when the pattern has no fix, or one of a kind that is not yet
    /// supported; an error when it cannot compile.
    pub(crate) fn new(pattern: &Pattern) -> Result<Option<Self>, Error> {
        let Some(fix) = &pattern.fix else {
            return Ok(None);
        };
        let compiled = match &fix.kind {
            FixKind::Ops(list) => Compiled::Ops(ops::Steps::compile(&pattern.id, list)?),
            FixKind::Command(command) => Compiled::Command(command.clone()),
            // Not supported yet: the pattern keeps its fix, no fixer is built, and
            // the plan declines its findings with that reason. The rest of the
            // pack is unaffected.
            FixKind::Rpc(_) => return Ok(None),
        };
        Ok(Some(Self {
            meta: FixerManifest {
                id: pattern.id.clone(),
                requires: fix.requires.clone(),
            },
            description: pattern.title.clone(),
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
