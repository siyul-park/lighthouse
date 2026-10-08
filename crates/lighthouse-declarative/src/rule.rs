//! The rule a decision's `check` compiles into: a CEL expression over the code
//! model, a standard operation (`order`, `proximity`, `cycle`) or a program.
//! The decision supplies id, severity, scope, options and the examples that
//! `lighthouse decision test` runs.

use std::sync::Arc;

use lighthouse_model::{Diagnostic, Options};
use lighthouse_plugin::{Ctx, Error as PluginError, Rule, RuleManifest};
use lighthouse_spec::{BuiltinCheck, BuiltinOp, CheckKind, Decision};
use serde_json::{Map, Value};

pub(crate) use crate::eval::{render, to_json};
use crate::{Error, cel_rule::CelRule, command::CommandRule, ops};

/// A rule built from a decision and its check.
#[derive(Clone)]
pub(crate) struct DeclarativeRule {
    meta: RuleManifest,
    decision: Decision,
    kind: Arc<Kind>,
}

enum Kind {
    Cel(Box<CelRule>),
    Order(ops::OrderRule),
    Proximity(ops::ProximityRule),
    Cycle(ops::CycleRule),
    Command(CommandRule),
}

impl DeclarativeRule {
    /// Compiles the check of `decision`; `None` for a decision whose check no
    /// program runs here (a named rule, `judged`, `rpc`) or that has none.
    pub(crate) fn new(decision: &Decision) -> Result<Option<Self>, Error> {
        let id = decision.id();
        let Some(check) = &decision.check else {
            return Ok(None);
        };
        let (kind, analyzers) = match &check.kind {
            CheckKind::Cel(cel) => {
                let rule = CelRule::new(decision, cel)?;
                let analyzers = rule.analyzers();
                (Kind::Cel(Box::new(rule)), analyzers)
            }
            CheckKind::Builtin(BuiltinCheck::Op(op)) => match op {
                BuiltinOp::Order { .. } => {
                    let rule = ops::OrderRule::new(id, op)?;
                    let analyzers = rule.analyzers();
                    (Kind::Order(rule), analyzers)
                }
                BuiltinOp::Proximity { .. } => {
                    let rule = ops::ProximityRule::new(id, op)?;
                    let analyzers = rule.analyzers();
                    (Kind::Proximity(rule), analyzers)
                }
                BuiltinOp::Cycle { .. } => (Kind::Cycle(ops::CycleRule::new(id, op)?), Vec::new()),
            },
            CheckKind::Command(command) => {
                (Kind::Command(CommandRule::new(check, command)), Vec::new())
            }
            CheckKind::Builtin(BuiltinCheck::Named(_))
            | CheckKind::Rpc(_)
            | CheckKind::Model(_) => {
                return Ok(None);
            }
        };
        let mut meta = decision
            .rule_manifest()
            .ok_or_else(|| Error::invalid(id, "the decision has no severity or check"))?;
        meta.analyzers = analyzers;
        meta.capabilities.clone_from(&check.requires);
        Ok(Some(Self {
            meta,
            decision: decision.clone(),
            kind: Arc::new(kind),
        }))
    }

    /// The options of the focused file's language over the decision's defaults.
    fn resolved(&self, configured: &Options, ctx: &Ctx) -> Result<Map<String, Value>, PluginError> {
        let language = ctx.file.map(|(file, _)| file.lang.as_str());
        self.decision
            .resolve_options(configured, language)
            .map_err(|e| PluginError::Options {
                rule: self.meta.id.clone(),
                message: e.to_string(),
            })
    }
}

impl Rule for DeclarativeRule {
    /// The metadata of the decision the rule was built from.
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }

    /// Accepts the options when they resolve against the decision's declared
    /// options: known keys of the declared types.
    fn validate(&self, options: &Options) -> Result<(), PluginError> {
        self.decision
            .resolve_options(options, None)
            .map(drop)
            .map_err(|e| PluginError::Options {
                rule: self.meta.id.clone(),
                message: e.to_string(),
            })
    }

    /// Runs once per file or once over the project, following the decision's scope.
    fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let resolved = self.resolved(options, ctx)?;
        let meta = &self.meta;
        match self.kind.as_ref() {
            Kind::Cel(rule) => rule.check(meta, ctx, &resolved),
            Kind::Order(rule) => rule.check(meta, ctx, &resolved),
            Kind::Proximity(rule) => rule.check(meta, ctx, &resolved),
            Kind::Cycle(rule) => rule.check(meta, ctx),
            Kind::Command(rule) => rule.check(meta, ctx, &resolved),
        }
    }
}
