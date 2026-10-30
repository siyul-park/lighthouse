//! The project: what `lighthouse.toml` holds, and the shareable projects
//! `extends` names. A project is a `Project` document; the `recommended` and
//! `strict` projects of a pack derive from its decisions, and a catalog may
//! hold more.

mod environment;
mod events;
mod fixing;
mod layers;
mod models;
mod provider;
mod resolved;
mod rules;
mod sandbox;
mod signals;
mod spec;
mod standard;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time,
};

pub use globset::GlobSet;

pub use environment::{EnvironmentRule, EnvironmentSpec};
pub use events::EventsSpec;
pub use fixing::{FixEnvironment, FixPolicy, FixSpec};
use globset::{GlobBuilder, GlobSetBuilder};
pub use layers::{BUNDLED, Entry, Layer, Layers};
use lighthouse_model::{Duration, Options, Severity, documents, resource};
pub use lighthouse_model::{Format, Metadata, Resource};
pub use lighthouse_process::Policy;
use lighthouse_protocol::Selector;
pub use models::{Evaluation, Models, Routing};
pub use provider::{CommandProtocol, ModelProvider, ProviderSpec, RESERVED_PROVIDERS, Task};
use resolved::Resolved;
pub use rules::{Level, RuleConfig, RuleSetting, Rules};
pub use sandbox::SandboxSpec;
pub use signals::{SignalsSpec, Surface};
use spec::PluginEntry;
pub use spec::{
    GeneratedCheck, OverrideSpec, ProjectSpec, SourceLanguageSpec, SourceRuleSpec, TestNamingSpec,
};

pub(crate) use standard::standard;

use crate::Error;

/// Name of the configuration file discovered in a project directory.
pub const FILE_NAME: &str = "lighthouse.toml";

/// Names a project's configuration may have, in the order they are tried in
/// a directory; the format follows the extension.
pub const FILE_NAMES: [&str; 4] = [
    FILE_NAME,
    "lighthouse.yaml",
    "lighthouse.yml",
    "lighthouse.json",
];

/// A listed plugin. `path` (relative to the config directory) names the
/// plugin directory explicitly instead of searching the plugin locations;
/// `timeout` is the per-request limit for process plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRef {
    pub id: String,
    pub path: Option<PathBuf>,
    timeout: Option<Duration>,
}

/// Rules for the files and languages that match.
#[derive(Debug, Clone)]
struct Override {
    files: Option<GlobSet>,
    languages: Vec<String>,
    rules: Rules,
}

/// What a project says about one language.
#[derive(Debug, Clone)]
struct LanguageConfig {
    /// Handed to the language's provider.
    options: Options,
    constructors: Option<Vec<String>>,
    test_naming: Option<TestNamingSpec>,
}

impl PluginRef {
    /// A listed plugin with no explicit path and no timeout override.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            path: None,
            timeout: None,
        }
    }

    /// The per-request limit, if one was configured.
    pub fn timeout(&self) -> Option<time::Duration> {
        self.timeout.map(Duration::get)
    }

    fn from_entry(entry: PluginEntry) -> Self {
        match entry {
            PluginEntry::Id(id) => Self::new(id),
            PluginEntry::Detailed(detail) => Self {
                id: detail.id,
                path: detail.path,
                timeout: detail.timeout,
            },
        }
    }
}

impl Override {
    fn matches(&self, path: &Path, lang: &str) -> bool {
        self.files.as_ref().is_none_or(|g| g.is_match(path))
            && (self.languages.is_empty() || self.languages.iter().any(|l| l == lang))
    }
}

/// The projects `extends` can name, by name. Each is built, and so validated,
/// the way a project that is run is.
#[derive(Debug, Clone, Default)]
pub struct Projects {
    /// Identifies this set of projects, so that what is resolved against one
    /// is not taken for what another resolves to.
    id: u64,
    by_name: BTreeMap<String, Config>,
}

impl Projects {
    /// The projects the documents describe. Their `plugins` and `languages`
    /// are not used: only a project that is run has those.
    pub fn new(documents: impl IntoIterator<Item = Resource<ProjectSpec>>) -> Result<Self, Error> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let mut by_name = BTreeMap::new();
        for project in documents {
            let name = project.metadata.name.clone();
            if by_name
                .insert(name.clone(), Config::from_resource(project)?)
                .is_some()
            {
                return Err(Error::DuplicateProject(name));
            }
        }
        Ok(Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            by_name,
        })
    }

    /// The names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// The project named `name`.
    pub fn get(&self, name: &str) -> Option<&Config> {
        self.by_name.get(name)
    }
}

/// Parsed `Project`: the content of `lighthouse.toml`, or of a shareable
/// project; the resolved `Project`.
#[derive(Debug, Clone)]
pub struct Config {
    name: String,
    plugins: Vec<PluginRef>,
    languages: BTreeMap<String, LanguageConfig>,
    sources: Vec<SourceRuleSpec>,
    generated: Option<GlobSet>,
    generated_check: Option<GeneratedCheck>,
    models: Models,
    sandbox: Policy,
    fix: FixPolicy,
    signals: SignalsSpec,
    extends: Vec<String>,
    rules: Rules,
    /// The model bindings of single decisions the personal layers set: they
    /// win over every rule of the project.
    environment: BTreeMap<String, EnvironmentRule>,
    overrides: Vec<Override>,
    resolved: Resolved,
}

impl Config {
    /// Parses `text` as a `lighthouse.toml`: a `Project` document. Unknown
    /// fields and invalid globs are errors.
    pub fn parse(text: &str) -> Result<Self, Error> {
        Self::parse_as(Format::Toml, FILE_NAME, text)
    }

    /// Parses `text` in `format`; `path` only names it in errors.
    pub fn parse_as(format: Format, path: &str, text: &str) -> Result<Self, Error> {
        Self::from_resource(parse_project(format, path, text)?)
    }

    /// Parses a bare spec written in TOML, without the envelope: for
    /// configurations built in code, such as the one a decision's example
    /// runs under. The project is named `inline`.
    pub fn parse_inline(text: &str) -> Result<Self, Error> {
        let spec: ProjectSpec =
            toml::from_str(text).map_err(|e| lighthouse_model::Error::Invalid {
                path: "<inline>".to_owned(),
                message: e.to_string(),
            })?;
        Self::from_resource(Resource::new(Metadata::named("inline"), spec))
    }

    /// Builds the configuration a parsed `Project` describes.
    pub fn from_resource(project: Resource<ProjectSpec>) -> Result<Self, Error> {
        let Resource { metadata, spec } = project;
        if let Some(share) = spec.models.explore
            && !(0.0..=1.0).contains(&share)
        {
            return Err(Error::Config(format!(
                "`models.explore` is {share}, not a share from 0 to 1"
            )));
        }
        if let Some(problem) = spec.fix.problem() {
            return Err(Error::Config(problem));
        }
        let rules = |settings: BTreeMap<String, RuleSetting>| -> Rules {
            settings
                .into_iter()
                .map(|(id, setting)| (id, setting.into()))
                .collect()
        };
        for (at, o) in spec.overrides.iter().enumerate() {
            if let Some(key) = environment::binding_in(&o.rules) {
                return Err(Error::Config(format!(
                    "`overrides[{at}].rules.{key}` is a model binding: bindings apply to a decision, not to some paths; set it in `rules`"
                )));
            }
        }
        let overrides = spec
            .overrides
            .into_iter()
            .map(|o| {
                Ok(Override {
                    files: globs(&o.files)?,
                    languages: o.languages,
                    rules: rules(o.rules),
                })
            })
            .collect::<Result<_, Error>>()?;
        validate_sources(&spec.sources)?;
        let mut languages = BTreeMap::new();
        for (id, language) in spec.languages {
            languages.insert(
                id,
                LanguageConfig {
                    options: language.options,
                    constructors: language.constructor_prefixes,
                    test_naming: language.test_naming,
                },
            );
        }
        Ok(Self {
            name: metadata.name,
            plugins: spec
                .plugins
                .into_iter()
                .map(PluginRef::from_entry)
                .collect(),
            languages,
            sources: spec.sources,
            generated: globs(&spec.generated.files)?,
            generated_check: spec.generated.check,
            models: Models::new(&spec.models),
            sandbox: spec.sandbox.into(),
            fix: FixPolicy::new(spec.fix),
            signals: spec.signals,
            extends: spec.extends,
            rules: rules(spec.rules),
            environment: BTreeMap::new(),
            overrides,
            resolved: Resolved::default(),
        })
    }

    /// Reads and parses the file at `path`, in the format its extension says.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(Error::io(path))?;
        Self::parse_as(
            Format::of_path(path).unwrap_or(Format::Toml),
            &path.display().to_string(),
            &text,
        )
    }

    /// Walks up from `start` and returns the first config file with its path.
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, Self)>, Error> {
        for dir in start.ancestors() {
            if let Some(path) = Self::file_in(dir) {
                let config = Self::load(&path)?;
                return Ok(Some((path, config)));
            }
        }
        Ok(None)
    }

    /// The configuration file in `dir`, if it has one.
    pub fn file_in(dir: &Path) -> Option<PathBuf> {
        lighthouse_model::file_in(dir, &FILE_NAMES)
    }

    /// The `metadata.name` of the project.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Listed plugins in declaration order; bare ids become [`PluginRef::new`].
    pub fn plugins(&self) -> &[PluginRef] {
        &self.plugins
    }

    /// Embedded source rules; host formats are inferred by normal file routing.
    pub fn sources(&self) -> &[SourceRuleSpec] {
        &self.sources
    }

    /// Whether `id` is listed in `plugins`.
    pub fn lists(&self, id: &str) -> bool {
        self.plugins.iter().any(|p| p.id == id)
    }

    /// The same configuration with the plugins `ids` enabled besides the
    /// listed ones: the plugins that need no listing (the bundled ones, the
    /// project's own decisions, the packs of the plugins it lists).
    pub fn enabling<'a>(mut self, ids: impl IntoIterator<Item = &'a str>) -> Self {
        for id in ids {
            if !self.lists(id) {
                self.plugins.push(PluginRef::new(id));
            }
        }
        self.resolved = Resolved::default();
        self
    }

    /// The model providers the project defines itself, by name.
    pub fn providers(&self) -> &BTreeMap<String, ModelProvider> {
        &self.models.providers
    }

    /// `languages.<id>` options handed to the language's provider; the
    /// `constructorPrefixes` key is the host's and is not among them.
    pub fn languages(&self) -> impl Iterator<Item = (&str, &Options)> {
        self.languages
            .iter()
            .map(|(id, l)| (id.as_str(), &l.options))
    }

    /// The constructor prefixes the project sets per language id, which replace
    /// the ones the language's provider declares.
    pub fn constructor_prefixes(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.languages
            .iter()
            .filter_map(|(id, l)| Some((id.as_str(), l.constructors.as_deref()?)))
    }

    /// The parts of the test naming convention the project replaces, per
    /// language id.
    pub fn test_naming(&self) -> impl Iterator<Item = (&str, &TestNamingSpec)> {
        self.languages
            .iter()
            .filter_map(|(id, l)| Some((id.as_str(), l.test_naming.as_ref()?)))
    }

    /// Whether the project's `generated.files` name `path`.
    pub fn is_generated(&self, path: &Path) -> bool {
        self.generated.as_ref().is_some_and(|g| g.is_match(path))
    }

    /// The project's `generated.check`: whether every decision checks
    /// generated code, or none does; `None` leaves it to each decision.
    pub fn generated_check(&self) -> Option<GeneratedCheck> {
        self.generated_check
    }

    /// The same configuration with `id` enabled at `level`, and its pack's
    /// plugin listed: how a candidate decision is run beside the project's.
    pub fn with_rule(mut self, id: &str, level: Severity) -> Self {
        let plugin = id.split_once('/').map_or(id, |(plugin, _)| plugin);
        if !self.lists(plugin) {
            self.plugins.push(PluginRef::new(plugin));
        }
        self.rules
            .insert(id.to_owned(), RuleConfig::new(Some(level)));
        self.resolved = Resolved::default();
        self
    }

    /// The models the project binds to its decisions: its own `models`, and
    /// the routes and evaluation policies of the rules of it and of the
    /// projects it extends.
    pub fn models(&self, projects: &Projects) -> Result<Models, Error> {
        let rules = self.resolve(Path::new(""), "", projects)?;
        Ok(self.models.with_rules(rules))
    }

    /// How the programs of the project run: in the OS sandbox or, in a trusted
    /// project that says so, with the user's full access; and the hosts and
    /// places they may reach, besides what each program lists.
    pub fn sandbox(&self) -> &Policy {
        &self.sandbox
    }

    /// What the project allows of fixes: whether a verified one that is not
    /// safe is written, the decisions a model may fix, the widest change, and
    /// the programs that must pass after them.
    pub fn fix(&self) -> &FixPolicy {
        &self.fix
    }

    /// What the project says about session signals.
    pub fn signals(&self) -> &SignalsSpec {
        &self.signals
    }

    /// Project names named in `extends`, in declaration order.
    pub fn extends(&self) -> &[String] {
        &self.extends
    }

    /// Entries named in `rules` and in every override, unmerged.
    pub fn configured(&self) -> impl Iterator<Item = (&str, &RuleConfig)> {
        self.rules
            .iter()
            .chain(self.overrides.iter().flat_map(|o| o.rules.iter()))
            .map(|(id, config)| (id.as_str(), config))
    }

    /// Like [`Config::configured`], over this project and every project it
    /// extends, the extended ones first.
    pub fn entries<'a>(
        &'a self,
        projects: &'a Projects,
    ) -> Result<Vec<(&'a str, &'a RuleConfig)>, Error> {
        Ok(self
            .chain(projects)?
            .into_iter()
            .flat_map(Config::configured)
            .collect())
    }

    /// Effective rules for a file: the projects it extends, then `rules`,
    /// then matching overrides in order. `path` is relative to the project
    /// root. Files that match the same overrides share one result.
    pub fn resolve(
        &self,
        path: &Path,
        lang: &str,
        projects: &Projects,
    ) -> Result<Arc<Rules>, Error> {
        let chain = self.chain(projects)?;
        let matched: Vec<bool> = chain
            .iter()
            .flat_map(|c| &c.overrides)
            .map(|o| o.matches(path, lang))
            .collect();
        Ok(self.resolved.get_or_merge(projects.id, matched, |matched| {
            let mut flags = matched.iter();
            let mut out = Rules::new();
            for config in &chain {
                rules::merge(&mut out, &config.rules);
                for o in &config.overrides {
                    if flags.next().copied().unwrap_or(false) {
                        rules::merge(&mut out, &o.rules);
                    }
                }
            }
            environment::bind(&mut out, &self.environment);
            out
        }))
    }

    /// This project and every project it extends, the extended ones first.
    fn chain<'a>(&'a self, projects: &'a Projects) -> Result<Vec<&'a Config>, Error> {
        let mut out = Vec::new();
        self.collect(projects, &mut Vec::new(), &mut out)?;
        Ok(out)
    }

    fn collect<'a>(
        &'a self,
        projects: &'a Projects,
        stack: &mut Vec<&'a str>,
        out: &mut Vec<&'a Config>,
    ) -> Result<(), Error> {
        for name in &self.extends {
            if stack.contains(&name.as_str()) {
                return Err(Error::ProjectCycle(name.clone()));
            }
            let base = projects
                .get(name)
                .ok_or_else(|| Error::UnknownProject(name.clone()))?;
            stack.push(name);
            base.collect(projects, stack, out)?;
            stack.pop();
        }
        out.push(self);
        Ok(())
    }
}

/// Compiles path globs. `*` and `?` never cross `/`; `**` as a whole path
/// component matches any number of directories (so `**` matches every path).
pub fn glob_set<I>(patterns: I) -> Result<GlobSet, Error>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut set = GlobSetBuilder::new();
    let mut all = Vec::new();
    for pattern in patterns {
        let pattern = pattern.as_ref();
        all.push(pattern.to_owned());
        let glob = GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|source| Error::Glob {
                pattern: pattern.to_owned(),
                source,
            })?;
        set.add(glob);
    }
    set.build().map_err(|source| Error::Glob {
        pattern: all.join(", "),
        source,
    })
}

fn globs(patterns: &[String]) -> Result<Option<GlobSet>, Error> {
    if patterns.is_empty() {
        return Ok(None);
    }
    glob_set(patterns).map(Some)
}

/// The `Project` document `text` is, read in `format`; `path` only names it in
/// errors.
fn parse_project(format: Format, path: &str, text: &str) -> Result<Resource<ProjectSpec>, Error> {
    let mut docs = documents(format, path, text)?;
    if docs.len() != 1 {
        return Err(Error::Documents {
            path: path.to_owned(),
            found: docs.len(),
        });
    }
    Ok(resource::<ProjectSpec>(path, &docs.remove(0))?)
}

/// Why the source rules are not usable, when they are not.
fn validate_sources(sources: &[SourceRuleSpec]) -> Result<(), Error> {
    let mut source_ids = BTreeSet::new();
    for (at, source) in sources.iter().enumerate() {
        if source.id.is_empty() || source.select.is_empty() {
            return Err(Error::Config(format!(
                "sources[{at}] requires non-empty id and select"
            )));
        }
        let language = &source.language;
        let fixed = language.provider.is_some();
        let dynamic = language.from.is_some();
        if language.provider.as_deref().is_some_and(str::is_empty)
            || language.from.as_deref().is_some_and(str::is_empty)
            || fixed == dynamic
        {
            return Err(Error::Config(format!(
                "sources[{at}].language requires exactly one non-empty provider or from"
            )));
        }
        if language.syntax.as_deref().is_some_and(str::is_empty) {
            return Err(Error::Config(format!(
                "sources[{at}].language.syntax must not be empty"
            )));
        }
        validate_selector(&source.select, &format!("sources[{at}].select"))?;
        if let Some(selector) = &language.from {
            validate_selector(selector, &format!("sources[{at}].language.from"))?;
        }
        if let Some(context) = &language.context {
            validate_source_context(context, &format!("sources[{at}].language.context"))?;
        }
        if source.files.is_empty() {
            return Err(Error::Config(format!(
                "source `{}` requires at least one file glob",
                source.id
            )));
        }
        if !source_ids.insert(&source.id) {
            return Err(Error::Config(format!(
                "duplicate source id `{}`",
                source.id
            )));
        }
        globs(&source.files)?;
    }
    Ok(())
}

fn validate_selector(selector: &str, field: &str) -> Result<(), Error> {
    Selector::parse(selector)
        .map(|_| ())
        .map_err(|reason| Error::Config(format!("{} has an invalid selector: {}", field, reason)))
}

fn validate_source_context(context: &serde_json::Value, field: &str) -> Result<(), Error> {
    let object = context
        .as_object()
        .ok_or_else(|| Error::Config(format!("{} must be an object", field)))?;

    if let Some(value) = object.get("identity") {
        let nested = format!("{}.identity", field);
        let identity = value.as_object().ok_or_else(|| {
            Error::Config(format!("{} must map identity fields to selectors", nested))
        })?;
        if identity.is_empty() {
            return Err(Error::Config(format!("{} must not be empty", nested)));
        }
        for (name, selector) in identity {
            let selector = selector.as_str().ok_or_else(|| {
                Error::Config(format!("{}.{} must be a selector string", nested, name))
            })?;
            validate_selector(selector, &format!("{}.{}", nested, name))?;
        }
    }

    if let Some(value) = object.get("variable") {
        let nested = format!("{}.variable", field);
        let variable = value
            .as_object()
            .ok_or_else(|| Error::Config(format!("{} must be an object", nested)))?;
        if !variable.contains_key("value") {
            return Err(Error::Config(format!("{}.value is required", nested)));
        }
        for (name, setting) in variable {
            match name.as_str() {
                "value" | "name" | "params" => {
                    let selector = setting.as_str().ok_or_else(|| {
                        Error::Config(format!("{}.{} must be a selector string", nested, name))
                    })?;
                    validate_selector(selector, &format!("{}.{}", nested, name))?;
                }
                "shared" if setting.is_boolean() => {}
                "shared" => {
                    return Err(Error::Config(format!(
                        "{}.shared must be a boolean literal",
                        nested
                    )));
                }
                _ => {
                    return Err(Error::Config(format!(
                        "{}.{} is not a supported field",
                        nested, name
                    )));
                }
            }
        }
    }

    if object
        .get("example")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(Error::Config(format!(
            "{}.example must be a boolean literal",
            field
        )));
    }

    for (key, value) in object {
        if matches!(key.as_str(), "identity" | "variable" | "example") {
            continue;
        }
        let nested = format!("{}.{}", field, key);
        let Some(projection) = value
            .as_object()
            .filter(|projection| projection.contains_key("select"))
        else {
            continue;
        };
        if projection.len() != 1 {
            return Err(Error::Config(format!(
                "{} projection must contain only select",
                nested
            )));
        }
        let selector = projection
            .get("select")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Config(format!("{}.select must be a selector string", nested)))?;
        validate_selector(selector, &format!("{}.select", nested))?;
    }
    Ok(())
}
