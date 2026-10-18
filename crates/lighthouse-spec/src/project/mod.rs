//! The project: what `lighthouse.toml` holds, and the shareable projects
//! `extends` names. A project is a `Project` document; the `recommended` and
//! `strict` projects of a pack derive from its decisions, and a catalog may
//! hold more.

mod fixing;
mod formatter;
mod models;
mod provider;
mod resolved;
mod rules;
mod spec;
mod standard;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time,
};

pub use globset::GlobSet;

use fixing::DEFAULT_VERIFY_TIMEOUT;
pub use fixing::{Approval, ApprovalSpec, FixSpec};
use globset::{GlobBuilder, GlobSetBuilder};
use lighthouse_model::{Options, Severity};
pub use lighthouse_process::Formatter;
use lighthouse_resource::{Duration, documents, resource};
pub use lighthouse_resource::{Format, Metadata, Resource};
pub use models::{Evaluation, Models, Routing};
pub use provider::{CommandProtocol, ModelProvider, ProviderSpec, RESERVED_PROVIDERS, Task};
use resolved::Resolved;
pub use rules::{Level, RuleConfig, RuleSetting, Rules};
use spec::PluginEntry;
pub use spec::{GeneratedCheck, OverrideSpec, ProjectSpec};

pub(crate) use standard::standard;

use crate::{Error, FixScope};

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
    formatter: Option<Formatter>,
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
    generated: Option<GlobSet>,
    generated_check: Option<GeneratedCheck>,
    models: Models,
    approval: Approval,
    fix_decisions: Vec<String>,
    fix_scope: FixScope,
    verify: Vec<Vec<String>>,
    verify_timeout: Duration,
    extends: Vec<String>,
    rules: Rules,
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
        let mut docs = documents(format, path, text)?;
        if docs.len() != 1 {
            return Err(Error::Documents {
                path: path.to_owned(),
                found: docs.len(),
            });
        }
        let project = resource::<ProjectSpec>(path, &docs.remove(0))?;
        Self::from_resource(project)
    }

    /// Parses a bare spec written in TOML, without the envelope: for
    /// configurations built in code, such as the one a decision's example
    /// runs under. The project is named `inline`.
    pub fn parse_inline(text: &str) -> Result<Self, Error> {
        let spec: ProjectSpec =
            toml::from_str(text).map_err(|e| lighthouse_resource::Error::Invalid {
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
        if let Some(at) = spec
            .fix
            .verify
            .iter()
            .position(|argv| argv.first().is_none_or(|program| program.trim().is_empty()))
        {
            return Err(Error::Config(format!("`fix.verify[{at}]` needs a program")));
        }
        let rules = |settings: BTreeMap<String, RuleSetting>| -> Rules {
            settings
                .into_iter()
                .map(|(id, setting)| (id, setting.into()))
                .collect()
        };
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
        let mut languages = BTreeMap::new();
        for (id, language) in spec.languages {
            let formatter = language.formatter.map(Formatter::from);
            if formatter.as_ref().is_some_and(|f| f.argv.is_empty()) {
                return Err(Error::Config(format!(
                    "`languages.{id}.formatter` must have a non-empty `argv`"
                )));
            }
            languages.insert(
                id,
                LanguageConfig {
                    options: language.options,
                    constructors: language.constructor_prefixes,
                    formatter,
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
            generated: globs(&spec.generated.files)?,
            generated_check: spec.generated.check,
            models: Models::new(&spec.models),
            approval: spec.approval.fix.unwrap_or_default(),
            fix_decisions: spec.fix.decisions,
            fix_scope: spec.fix.scope.unwrap_or(FixScope::Files),
            verify: spec.fix.verify,
            verify_timeout: spec.fix.timeout.unwrap_or(DEFAULT_VERIFY_TIMEOUT),
            extends: spec.extends,
            rules: rules(spec.rules),
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
        lighthouse_resource::file_in(dir, &FILE_NAMES)
    }

    /// The `metadata.name` of the project.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Listed plugins in declaration order; bare ids become [`PluginRef::new`].
    pub fn plugins(&self) -> &[PluginRef] {
        &self.plugins
    }

    /// Whether `id` is listed in `plugins`.
    pub fn lists(&self, id: &str) -> bool {
        self.plugins.iter().any(|p| p.id == id)
    }

    /// Every language's formatter, by language id.
    pub fn formatters(&self) -> impl Iterator<Item = (&str, &Formatter)> {
        self.languages
            .iter()
            .filter_map(|(id, l)| Some((id.as_str(), l.formatter.as_ref()?)))
    }

    /// The formatter of a language, if its configuration names one.
    pub fn formatter(&self, language: &str) -> Option<&Formatter> {
        self.languages.get(language)?.formatter.as_ref()
    }

    /// `languages.<id>` options handed to the language's provider; the
    /// `formatter` key is the host's and is not among them.
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

    /// Whether a verified fix by a model is written or left as a preview.
    pub fn approval(&self) -> Approval {
        self.approval
    }

    /// The decisions a model may fix: ids or globs over them; none when the
    /// project lists none.
    pub fn fix_decisions(&self) -> &[String] {
        &self.fix_decisions
    }

    /// The widest change any fix by a model may make.
    pub fn fix_scope(&self) -> FixScope {
        self.fix_scope
    }

    /// The commands that must pass with the fixes by models in place.
    pub fn verify(&self) -> &[Vec<String>] {
        &self.verify
    }

    /// How long each of those commands may run.
    pub fn verify_timeout(&self) -> time::Duration {
        self.verify_timeout.get()
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
