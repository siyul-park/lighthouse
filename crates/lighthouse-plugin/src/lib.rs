mod registry;

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_config::Rules;
use lighthouse_model::{
    Capability, Diagnostic, File, Fragment, Incomplete, Options, Project, Severity,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

pub use registry::{Registry, plugin_of};

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
}

/// What an analyzer or rule looks at in one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// Once per file; `Ctx::file` is set.
    File,
    /// Once per run over the merged project; `Ctx::file` is `None`.
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub version: String,
}

/// Where the checked project lives and how each language is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    /// Per-language options from the configuration, keyed by language id.
    pub languages: BTreeMap<String, Options>,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            languages: BTreeMap::new(),
        }
    }
}

/// Fact key: analyzer id and scope key (file path, or empty for project scope).
pub type Facts = BTreeMap<(String, String), Value>;

/// Everything an analyzer or rule may read in one run.
pub struct Ctx<'a> {
    pub ws: &'a Workspace,
    pub project: &'a Project,
    /// The focused file and its text; `None` for project scope.
    pub file: Option<(&'a File, &'a str)>,
    pub facts: &'a Facts,
}

impl Ctx<'_> {
    fn key(&self) -> String {
        self.file
            .map_or_else(String::new, |(f, _)| f.path.to_string_lossy().into_owned())
    }

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
}

/// Deserializes rule options; unknown keys are rejected when `T` denies them.
pub fn options<T: DeserializeOwned>(rule: &str, options: &Options) -> Result<T, Error> {
    serde_json::from_value(Value::Object(options.clone())).map_err(|e| Error::Options {
        rule: rule.to_owned(),
        message: e.to_string(),
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conventions {
    pub test_globs: Vec<String>,
}

/// A file handed to a provider with its text as read by the engine.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pub file: &'a File,
    pub text: &'a str,
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

pub trait LanguageProvider: Send + Sync {
    fn id(&self) -> &str;
    fn globs(&self) -> &[String];
    fn conventions(&self) -> Conventions;
    fn capabilities(&self) -> &[Capability];
    /// A fallback provider claims a file only when no other provider does.
    fn fallback(&self) -> bool {
        false
    }
    /// Among regular providers the higher priority claims a file first.
    fn priority(&self) -> i32 {
        0
    }
    /// Indexes every file of this provider in one call. An `Err` means the
    /// whole batch failed; every file is then incomplete.
    fn index(&self, ws: &Workspace, files: &[Source]) -> Result<Indexed, Error>;
}

pub trait Analyzer: Send + Sync {
    /// Fully qualified `plugin/name`.
    fn id(&self) -> &str;
    fn requires(&self) -> &[String];
    fn scope(&self) -> Scope;
    fn run(&self, ctx: &Ctx) -> Result<Value, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleMeta {
    /// Fully qualified `plugin/name`.
    pub id: String,
    pub severity: Severity,
    pub scope: Scope,
    pub description: String,
    pub docs: String,
    pub analyzers: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub citation: Option<String>,
}

pub trait Rule: Send + Sync {
    fn meta(&self) -> &RuleMeta;
    fn validate(&self, options: &Options) -> Result<(), Error>;
    fn check(&self, ctx: &Ctx, options: &Options) -> Result<Vec<Diagnostic>, Error>;
}

/// Named rule configuration, referenced from `extends`.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    /// Fully qualified `plugin/name`.
    pub id: String,
    pub rules: Rules,
}

pub trait Plugin {
    fn manifest(&self) -> Manifest;
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        Vec::new()
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        Vec::new()
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        Vec::new()
    }
    fn presets(&self) -> Vec<Preset> {
        Vec::new()
    }
}
