use std::marker::PhantomData;

use lighthouse_model::{Diagnostic, Options};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Catalog, Decision};

impl Decision {
    /// Rule metadata of a checked decision; `None` while it has no check.
    /// Analyzers and capabilities belong to the check.
    pub fn rule_manifest(&self) -> Option<RuleManifest> {
        self.check.as_ref()?;
        Some(RuleManifest {
            id: self.id().to_owned(),
            severity: self.severity()?,
            scope: self.scope.subject.rule_scope(),
            description: self.title.clone(),
            docs: self.requirement.clone(),
            analyzers: Vec::new(),
            capabilities: Vec::new(),
            citation: self.citation.clone(),
            strict: self.strict,
        })
    }
}

/// A rule whose metadata and option defaults come from its catalog decision.
/// `check` receives the options resolved for the focused file's language.
pub struct DecisionRule<O, F> {
    meta: RuleManifest,
    decision: &'static Decision,
    check: F,
    options: PhantomData<fn() -> O>,
}

impl<O, F> DecisionRule<O, F>
where
    O: DeserializeOwned,
    F: Fn(&RuleManifest, &Ctx, O) -> Result<Vec<Diagnostic>, Error> + Send + Sync,
{
    /// Panics when the bundled catalog has no checked decision `id`.
    pub fn new(id: &str, analyzers: &[&str], check: F) -> Self {
        let decision = Catalog::bundled()
            .decision(id)
            .unwrap_or_else(|| panic!("bundled catalog defines {id}"));
        let mut meta = decision
            .rule_manifest()
            .unwrap_or_else(|| panic!("{id} has a check"));
        meta.analyzers = analyzers.iter().map(|a| (*a).to_owned()).collect();
        Self {
            meta,
            decision,
            check,
            options: PhantomData,
        }
    }

    fn resolve(&self, configured: &Options, language: Option<&str>) -> Result<O, Error> {
        let fail = |message: String| Error::Options {
            rule: self.meta.id.clone(),
            message,
        };
        let resolved = self
            .decision
            .resolve_options(configured, language)
            .map_err(|e| fail(e.to_string()))?;
        serde_json::from_value(Value::Object(resolved)).map_err(|e| fail(e.to_string()))
    }
}

impl<O, F> Rule for DecisionRule<O, F>
where
    O: DeserializeOwned + Send + Sync,
    F: Fn(&RuleManifest, &Ctx, O) -> Result<Vec<Diagnostic>, Error> + Send + Sync,
{
    /// The metadata of the decision the rule was built from.
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }

    /// Accepts the options when they resolve against the decision's declared
    /// options: known keys of the declared types.
    fn validate(&self, options: &Options) -> Result<(), Error> {
        self.resolve(options, None).map(drop)
    }

    /// Resolves the options for the focused file's language, then runs the check.
    fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, Error> {
        let language = ctx.file.map(|(file, _)| file.lang.as_str());
        (self.check)(&self.meta, ctx, self.resolve(options, language)?)
    }
}
