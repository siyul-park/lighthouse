//! The rule a decision's `check` compiles into: a CEL expression over the code
//! model, a standard operation (`order`, `proximity`, `cycle`), a program, or
//! the name of an annotation rule the engine serves.
//! The decision supplies id, severity, scope, options and the examples that
//! `lighthouse decision test` runs.

use std::sync::Arc;

use lighthouse_model::{
    Diagnostic, Options, Reach,
    annotation::{ANNOTATION_REASON, UNUSED_ALLOW},
    hash,
};
use lighthouse_plugin::{Caching, Ctx, Error as PluginError, Rule, RuleManifest};
use lighthouse_spec::{BuiltinCheck, BuiltinOp, CheckKind, Decision};
use serde_json::{Map, Value};

pub(crate) use crate::eval::{render, to_json};
use crate::{
    Error,
    cel_rule::CelRule,
    command::CommandRule,
    manifest::{rule_manifest, rule_options},
    ops,
};

/// The decisions whose named check the engine serves.
const ANNOTATION_RULES: [&str; 2] = [ANNOTATION_REASON, UNUSED_ALLOW];

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
    /// The rules about allow annotations: the engine reads the annotations of
    /// the whole project and reports these findings itself, because whether
    /// an annotation is used depends on every other rule's findings. The rule
    /// exists so that configuration, presets and the catalog treat it like any
    /// other.
    Annotation,
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
            CheckKind::Builtin(BuiltinCheck::Named(named))
                if ANNOTATION_RULES.contains(&named.id.as_str()) =>
            {
                (Kind::Annotation, Vec::new())
            }
            CheckKind::Builtin(BuiltinCheck::Named(_))
            | CheckKind::Rpc(_)
            | CheckKind::Model(_) => {
                return Ok(None);
            }
        };
        let mut meta = rule_manifest(decision)
            .ok_or_else(|| Error::invalid(id, "the decision has no severity or check"))?;
        meta.analyzers = analyzers;
        meta.capabilities.clone_from(&check.requires);
        let revision = serde_json::to_vec(&(
            decision.uid(),
            &decision.check,
            &decision.scope,
            decision.severity(),
            &decision.options,
            &decision.languages,
        ))
        .map(hash::sha256)
        .map_err(|e| Error::invalid(id, format!("the decision cannot be hashed: {e}")))?;
        let (reach, positions) = match &kind {
            Kind::Cel(rule) => rule.reach(meta.scope),
            Kind::Order(rule) => rule.reach(),
            Kind::Proximity(rule) => rule.reach(),
            Kind::Cycle(_) => (Reach::Global, false),
            Kind::Command(_) | Kind::Annotation => (Reach::Global, false),
        };
        // The findings of a program that reads the code model are stored;
        // those of a command, which reads whatever the program reads, and the
        // annotation rules, which the engine reports itself, are not.
        meta.caching = match kind {
            Kind::Command(_) | Kind::Annotation => None,
            _ => Some(Caching {
                reach,
                positions,
                revision,
            }),
        };
        Ok(Some(Self {
            meta,
            decision: decision.clone(),
            kind: Arc::new(kind),
        }))
    }

    /// The options of the focused file's language over the decision's defaults.
    fn resolved(&self, configured: &Options, ctx: &Ctx) -> Result<Map<String, Value>, PluginError> {
        let language = ctx.file.map(|(file, _)| file.lang.as_str());
        rule_options(&self.decision, configured, language)
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
        rule_options(&self.decision, options, None).map(drop)
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
            Kind::Annotation => Ok(Vec::new()),
        }
    }
}
