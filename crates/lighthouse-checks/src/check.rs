//! The check a decision's `check` block compiles into: a CEL expression over the code
//! model, or a program.
//! The decision supplies id, severity, scope, options and the examples that
//! `lighthouse decision test` runs.

use std::{collections::BTreeMap, sync::Arc};

use lighthouse_model::{Diagnostic, Options, Reach, RunScope};

use lighthouse_plugin::{Caching, Context, Error as PluginError, Rule, RuleManifest};
use lighthouse_protocol::hash;
use lighthouse_spec::{CheckKind, Decision, Select};
use serde_json::{Map, Value};

use crate::{
    Composition, Error,
    cel_rule::CelRule,
    command::CommandRule,
    library,
    manifest::{rule_manifest, rule_options, validate_options},
};

/// A rule built from a decision and its check.
#[derive(Clone)]
pub(crate) struct Check {
    meta: RuleManifest,
    decision: Decision,
    kind: Arc<Kind>,
    libraries: BTreeMap<String, String>,
}

enum Kind {
    Cel(Box<CelRule>),
    Command(Box<CommandRule>),
}

impl Check {
    /// Compiles the check of `decision`; `None` for a decision whose check no
    /// program runs here (`judged`, `rpc`) or that has none.
    pub(crate) fn new(comp: &Composition, decision: &Decision) -> Result<Option<Self>, Error> {
        let id = decision.id();
        let Some(check) = &decision.check else {
            return Ok(None);
        };
        let kind = match &check.kind {
            CheckKind::Cel(cel) => Kind::Cel(Box::new(CelRule::new(comp, decision, cel)?)),
            CheckKind::Command(command) => Kind::Command(Box::new(CommandRule::new(
                command,
                command.programs(decision),
                &decision.title,
                decision.bundled(
                    comp.bundled()
                        .map_err(|e| Error::invalid(id, e.to_string()))?,
                ),
            )?)),
            CheckKind::Model(model) => {
                // A model check only ranges over subjects of a file; the
                // others stay with agent review.
                let Some(cel) = model.as_cel(decision.scope.subject) else {
                    return Ok(None);
                };
                if rule_manifest(decision).is_none() {
                    return Ok(None);
                }
                if cel.selects(decision.scope.subject).map(Select::scope) != Some(RunScope::File) {
                    return Ok(None);
                }
                Kind::Cel(Box::new(CelRule::new(comp, decision, &cel)?))
            }
        };
        let mut meta = rule_manifest(decision)
            .ok_or_else(|| Error::invalid(id, "the decision has no severity or check"))?;
        if let Kind::Cel(rule) = &kind {
            meta.analyzers = rule.analyzers();
        }
        meta.capabilities.clone_from(&check.requires);
        let mut libraries = match &kind {
            Kind::Cel(rule) => rule.libraries().clone(),
            _ => BTreeMap::new(),
        };
        let check_source = serde_json::to_string(&decision.check).unwrap_or_default();
        let mut macro_source = check_source;
        if let Some(fix) = &decision.fix
            && let lighthouse_spec::FixKind::Cel { edits, .. } = &fix.kind
        {
            macro_source.push_str(edits);
            if let Ok(program) = comp.cel().compile_shared(edits) {
                libraries.extend(library::libraries_called(comp, [program.as_ref()]));
            }
        }
        // `cel.bind` is expanded during parsing, so it is absent from the
        // compiled AST used by the normal library-call walk.
        if macro_source.contains("cel.bind")
            && let Some(entry) = comp
                .functions()
                .iter()
                .find(|entry| entry.function.name == "cel.bind")
        {
            libraries.insert(entry.library.clone(), entry.version.clone());
        }
        let revision = revision(decision, &libraries)
            .map_err(|e| Error::invalid(id, format!("the decision cannot be hashed: {e}")))?;
        let (reach, positions) = match &kind {
            Kind::Cel(rule) => rule.reach(meta.scope),
            Kind::Command(_) => (Reach::Global, false),
        };
        // The findings of a program that reads the code model are stored;
        // those of a command, which reads whatever the program reads, and of
        // a rule that reads the directives, which depend on what every other
        // rule found, are not. A bundled command that judges a file by its
        // text alone is.
        meta.caching = match &kind {
            Kind::Cel(rule) if rule.after_suppression() => None,
            Kind::Command(rule) if rule.local(meta.scope) => Some(Caching {
                reach: Reach::Local,
                positions: false,
                revision,
            }),
            Kind::Command(_) => None,
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
            libraries,
        }))
    }

    /// The id and version of each CEL library the check's expressions call.
    pub(crate) fn libraries(&self) -> BTreeMap<String, String> {
        self.libraries.clone()
    }

    /// The options of the focused file's language over the decision's defaults.
    fn resolved(
        &self,
        configured: &Options,
        ctx: &Context,
    ) -> Result<Map<String, Value>, PluginError> {
        let language = ctx.file.map(|(file, _)| file.analysis_language());
        rule_options(&self.decision, configured, language)
    }
}

impl Rule for Check {
    /// The metadata of the decision the rule was built from.
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }

    fn reviews(&self) -> bool {
        matches!(&*self.kind, Kind::Cel(rule) if rule.reviews())
    }

    fn after_suppression(&self) -> bool {
        matches!(&*self.kind, Kind::Cel(rule) if rule.after_suppression())
    }

    /// Accepts the options when they resolve against the decision's declared
    /// options: known keys of the declared types.
    fn validate(&self, options: &Options) -> Result<(), PluginError> {
        validate_options(&self.decision, options)
    }

    /// Runs once per file or once over the project, following the decision's scope.
    fn check(&self, ctx: &Context, options: &Options) -> Result<Vec<Diagnostic>, PluginError> {
        let resolved = self.resolved(options, ctx)?;
        let meta = &self.meta;
        match self.kind.as_ref() {
            Kind::Cel(rule) => rule.check(meta, ctx, &resolved),
            Kind::Command(rule) => rule.check(meta, ctx, &resolved),
        }
    }
}

/// What the findings of `decision` depend on besides the code: the way it
/// judges (check, scope, severity, options, language values) and the ids and
/// versions of the CEL libraries its expressions call.
pub(crate) fn revision(
    decision: &Decision,
    libraries: &BTreeMap<String, String>,
) -> Result<String, serde_json::Error> {
    serde_json::to_vec(&(
        libraries,
        decision.uid(),
        &decision.check,
        &decision.scope,
        decision.severity,
        &decision.options,
        &decision.languages,
    ))
    .map(hash::sha256)
}
