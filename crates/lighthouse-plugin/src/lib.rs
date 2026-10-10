mod fix;
mod memo;
mod registry;

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::{
    Applicability, Capability, Diagnostic, File, Fragment, Incomplete, Options, Project, RunScope,
    Severity,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

pub use fix::{
    FixDecision, FixRequest, Fixer, FixerManifest, KeyCtx, NoKeys, OrderKey, OrderKeyManifest,
    OrderKeys,
};
pub use memo::Memo;
pub use registry::{Registry, plugin_of};

/// Why registering a plugin, resolving analyzers, reading facts or running a
/// rule failed. Messages name the offending id and are fit to show the user.
#[derive(Debug, Error)]
pub enum Error {
    #[error("`{0}` is registered twice")]
    Duplicate(String),
    #[error("`{id}` must be prefixed with `{plugin}/`")]
    Prefix { plugin: String, id: String },
    #[error("`{required_by}` requires unknown analyzer `{id}`")]
    MissingAnalyzer { id: String, required_by: String },
    #[error("analyzer cycle: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    #[error("invalid options for `{rule}`: {message}")]
    Options { rule: String, message: String },
    #[error("missing fact from `{0}`")]
    MissingFact(String),
    #[error("fact from `{analyzer}` has an unexpected shape: {message}")]
    BadFact { analyzer: String, message: String },
    #[error("{0}")]
    Failed(String),
    /// The check could not run to the end (a program crashed, timed out or
    /// exited with an error): what it covers was not checked, which is never
    /// the same as passing. The run reports the gap and goes on.
    #[error("{0}")]
    Incomplete(String),
}

/// Identity of a plugin: `id` prefixes every analyzer, rule and fixer
/// it contributes. Every plugin kind describes itself through a manifest, a
/// static value separate from the behavior of the kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
}

/// Where the checked project lives and how each language is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    /// Per-language options from the configuration, keyed by language id.
    pub languages: BTreeMap<String, Options>,
    /// Text that stands in for the file of the same project-relative path:
    /// what a provider must read instead of the disk. Empty for a normal run;
    /// a fix run checks candidate edits this way before anything is written.
    pub overlays: BTreeMap<PathBuf, String>,
}

/// Fact key: analyzer id and scope key (file path, or empty for project scope).
pub type Facts = BTreeMap<(String, String), Value>;

impl Workspace {
    /// A workspace at `root` with no language options.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            languages: BTreeMap::new(),
            overlays: BTreeMap::new(),
        }
    }
}

/// Everything an analyzer or rule may read in one run.
pub struct Ctx<'a> {
    pub ws: &'a Workspace,
    pub project: &'a Project,
    /// The focused file and its text; `None` for project scope.
    pub file: Option<(&'a File, &'a str)>,
    pub facts: &'a Facts,
    /// The order keys of the run's plugins, for checks that judge an order.
    pub keys: &'a dyn OrderKeys,
    /// The user trusts the project to run the commands it names. The user
    /// decides that, never the repository.
    pub trusted: bool,
    /// Values the rules of this run share, so that each is computed once.
    pub memo: &'a Memo,
    /// What code the rule being run may report on: its decision's scope as
    /// the project's configuration left it. The engine drops the files that
    /// are out before the rule runs; a rule drops the symbols that are.
    pub applies: Applicability,
}

/// Layout conventions a language declares; the engine applies them to every file of that language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conventions {
    pub test_globs: Vec<String>,
}

impl Ctx<'_> {
    /// The analyzer's fact for the focused file, else its project fact.
    pub fn fact<T: DeserializeOwned>(&self, analyzer: &str) -> Result<T, Error> {
        let at = |key| self.facts.get(&(analyzer.to_owned(), key));
        let value = at(self.key())
            .or_else(|| at(String::new()))
            .ok_or_else(|| Error::MissingFact(analyzer.to_owned()))?;
        T::deserialize(value).map_err(|e| Error::BadFact {
            analyzer: analyzer.to_owned(),
            message: e.to_string(),
        })
    }

    fn key(&self) -> String {
        self.file
            .map_or_else(String::new, |(f, _)| f.path.to_string_lossy().into_owned())
    }
}

/// What a language provider declares about itself: which files are its own and
/// what it guarantees about them. The `initialize` result of a plugin process
/// carries one per language, field for field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderManifest {
    /// The language id, such as `go`.
    pub id: String,
    pub globs: Vec<String>,
    pub conventions: Conventions,
    pub capabilities: Vec<Capability>,
    /// A fallback provider claims a file only when no other provider does.
    pub fallback: bool,
    /// Among regular providers the higher priority claims a file first.
    pub priority: i32,
}

/// A file handed to a provider with its text as read by the engine.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pub file: &'a File,
    pub text: &'a str,
}

impl ProviderManifest {
    /// A regular provider of `id` for `globs`, with no conventions or
    /// capabilities; set those directly.
    pub fn new(id: impl Into<String>, globs: Vec<String>) -> Self {
        Self {
            id: id.into(),
            globs,
            conventions: Conventions::default(),
            capabilities: Vec::new(),
            fallback: false,
            priority: 0,
        }
    }
}

/// What a provider produced for a batch of files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indexed {
    pub fragments: Vec<Fragment>,
    /// Things the user should see that do not make the analysis incomplete.
    pub notices: Vec<String>,
    /// Files or scopes that could not be analyzed.
    pub incomplete: Vec<Incomplete>,
}

/// Turns the files of one language into UCM fragments. Implementations must
/// be thread-safe, and `index` must not read outside `files` and `ws`.
pub trait LanguageProvider: Send + Sync {
    fn manifest(&self) -> &ProviderManifest;
    /// Indexes every file of this provider in one call. An `Err` means the
    /// whole batch failed; every file is then incomplete.
    fn index(&self, ws: &Workspace, files: &[Source]) -> Result<Indexed, Error>;
}

/// Computes one fact, declared by `id`, from the project and the facts of its `requires`.
/// `run` must be deterministic for equal inputs; the engine runs it once per `scope`.
pub trait Analyzer: Send + Sync {
    fn manifest(&self) -> &AnalyzerManifest;
    fn run(&self, ctx: &Ctx) -> Result<Value, Error>;
}

/// What an analyzer declares: the fact it computes, what that fact is
/// computed from, and what it looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerManifest {
    /// Fully qualified `plugin/name`.
    pub id: String,
    /// Analyzers whose facts this one reads.
    pub requires: Vec<String>,
    pub scope: RunScope,
}

/// Static description of a rule: its identity, default severity, scope and
/// the analyzers and provider capabilities it depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleManifest {
    /// Fully qualified `plugin/name`.
    pub id: String,
    pub severity: Severity,
    pub scope: RunScope,
    pub description: String,
    pub docs: String,
    pub analyzers: Vec<String>,
    pub capabilities: Vec<Capability>,
    /// What code the rule's subjects may be in, from its decision's scope.
    pub applicability: Applicability,
}

/// A check that turns facts into diagnostics. `validate` rejects bad options
/// before any run; `check` must report only for the scope in `manifest()`.
pub trait Rule: Send + Sync {
    fn manifest(&self) -> &RuleManifest;
    fn validate(&self, options: &Options) -> Result<(), Error>;
    fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, Error>;
}

/// A bundle of language providers, analyzers, rules and fixers, all of whose
/// ids are qualified with `manifest().id`. Everything defaults to empty.
pub trait Plugin {
    fn manifest(&self) -> &PluginManifest;
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        Vec::new()
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        Vec::new()
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        Vec::new()
    }
    /// The fixers of the plugin's decisions; see [`Fixer`].
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        Vec::new()
    }
    /// Ways to order declarations that fix operations may name.
    fn order_keys(&self) -> Vec<Box<dyn OrderKey>> {
        Vec::new()
    }
}

/// Deserializes rule options; unknown keys are rejected when `T` denies them.
pub fn options<T: DeserializeOwned>(rule: &str, options: &Options) -> Result<T, Error> {
    serde_json::from_value(Value::Object(options.clone())).map_err(|e| Error::Options {
        rule: rule.to_owned(),
        message: e.to_string(),
    })
}
