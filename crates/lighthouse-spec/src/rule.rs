use std::marker::PhantomData;

use lighthouse_model::{Diagnostic, Options};
use lighthouse_plugin::{Ctx, Error, Rule, RuleManifest, Scope as RunScope};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Catalog, Pattern, Scope};

impl Scope {
    /// Symbol, file and test patterns are checked once per file; module and
    /// project patterns once over the merged project.
    pub fn rule_scope(self) -> RunScope {
        match self {
            Self::Symbol | Self::File | Self::Test => RunScope::File,
            Self::Module | Self::Project => RunScope::Project,
        }
    }
}

impl Pattern {
    /// Rule metadata of an implemented pattern; `None` while it has no
    /// implementation. Analyzers and capabilities belong to the implementation.
    pub fn rule_manifest(&self) -> Option<RuleManifest> {
        self.implementation.as_ref()?;
        Some(RuleManifest {
            id: self.id.clone(),
            severity: self.severity()?,
            scope: self.scope.rule_scope(),
            description: self.title.clone(),
            docs: self.requirement.clone(),
            analyzers: Vec::new(),
            capabilities: Vec::new(),
            citation: self.citation.clone(),
            strict: self.strict,
        })
    }
}

/// A rule whose metadata and option defaults come from its catalog pattern.
/// `check` receives the options resolved for the focused file's language.
pub struct PatternRule<O, F> {
    meta: RuleManifest,
    pattern: &'static Pattern,
    check: F,
    options: PhantomData<fn() -> O>,
}

impl<O, F> PatternRule<O, F>
where
    O: DeserializeOwned,
    F: Fn(&RuleManifest, &Ctx, O) -> Result<Vec<Diagnostic>, Error> + Send + Sync,
{
    /// Panics when the bundled catalog has no implemented pattern `id`.
    pub fn new(id: &str, analyzers: &[&str], check: F) -> Self {
        let pattern = Catalog::bundled()
            .pattern(id)
            .unwrap_or_else(|| panic!("bundled catalog defines {id}"));
        let mut meta = pattern
            .rule_manifest()
            .unwrap_or_else(|| panic!("{id} is implemented"));
        meta.analyzers = analyzers.iter().map(|a| (*a).to_owned()).collect();
        Self {
            meta,
            pattern,
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
            .pattern
            .resolve_options(configured, language)
            .map_err(|e| fail(e.to_string()))?;
        serde_json::from_value(Value::Object(resolved)).map_err(|e| fail(e.to_string()))
    }
}

impl<O, F> Rule for PatternRule<O, F>
where
    O: DeserializeOwned + Send + Sync,
    F: Fn(&RuleManifest, &Ctx, O) -> Result<Vec<Diagnostic>, Error> + Send + Sync,
{
    /// The metadata of the pattern the rule was built from.
    fn manifest(&self) -> &RuleManifest {
        &self.meta
    }

    /// Accepts the options when they resolve against the pattern's declared
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
