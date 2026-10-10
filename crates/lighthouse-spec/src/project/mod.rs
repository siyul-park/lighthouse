//! The project: what `lighthouse.toml` holds, and the shareable projects
//! `extends` names. A project is a `Project` document; the `recommended` and
//! `strict` projects of a pack derive from its decisions, and a catalog may
//! hold more.

mod formatter;
mod rules;
mod spec;
mod standard;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub use formatter::{Formatter, FormatterOutput, FormatterSpec, FormatterStdin};
pub use globset::GlobSet;
use globset::{GlobBuilder, GlobSetBuilder};
use lighthouse_model::Options;
pub use lighthouse_resource::{Format, Metadata, Resource};
use lighthouse_resource::{documents, parse_duration, resource};
pub use rules::{Level, RuleConfig, RuleDetail, RuleSetting, Rules};
pub use spec::{
    GeneratedCheck, GeneratedSpec, OverrideSpec, PluginEntry, PluginRefSpec, ProjectLanguage,
    ProjectSpec,
};
pub(crate) use standard::standard;
use thiserror::Error;

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

/// Failure to read, parse or apply a configuration; every variant names the offending input.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid configuration: {0}")]
    Parse(#[from] lighthouse_resource::Error),
    #[error("{path}: expected one Project document, found {found}")]
    Documents { path: String, found: usize },
    #[error("invalid glob `{pattern}`: {source}")]
    Glob {
        pattern: String,
        source: globset::Error,
    },
    #[error("unknown project `{0}`")]
    UnknownProject(String),
    #[error("project `{0}` extends itself")]
    ProjectCycle(String),
    #[error("project `{0}` is defined twice")]
    DuplicateProject(String),
    #[error("invalid configuration: `languages.{language}.formatter` must have a non-empty `argv`")]
    Formatter { language: String },
    #[error("invalid configuration: `{text}` is not a duration such as `30s` (plugin `{plugin}`)")]
    Duration { plugin: String, text: String },
}

/// A listed plugin. `path` (relative to the config directory) names the
/// plugin directory explicitly instead of searching the plugin locations;
/// `timeout` is the per-request limit for process plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRef {
    pub id: String,
    pub path: Option<PathBuf>,
    timeout: Option<Duration>,
}

#[derive(Debug, Clone)]
struct Override {
    files: Option<GlobSet>,
    languages: Vec<String>,
    rules: Rules,
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
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    fn of(entry: PluginEntry) -> Result<Self, ProjectError> {
        match entry {
            PluginEntry::Id(id) => Ok(Self::new(id)),
            PluginEntry::Detailed(detail) => {
                let timeout = detail
                    .timeout
                    .map(|text| {
                        parse_duration(&text).ok_or_else(|| ProjectError::Duration {
                            plugin: detail.id.clone(),
                            text,
                        })
                    })
                    .transpose()?;
                Ok(Self {
                    id: detail.id,
                    path: detail.path,
                    timeout,
                })
            }
        }
    }
}

impl Override {
    fn matches(&self, path: &Path, lang: &str) -> bool {
        self.files.as_ref().is_none_or(|g| g.is_match(path))
            && (self.languages.is_empty() || self.languages.iter().any(|l| l == lang))
    }
}

/// A rule id a configuration sets under the name a decision had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renamed {
    pub old: String,
    pub new: String,
    /// The configuration set the new id too, and that setting was kept.
    pub both: bool,
}

/// What a project says about rules: the projects it extends, its own `rules`
/// and its `overrides`. An extended project is one of these and nothing more.
#[derive(Debug, Clone, Default)]
pub struct Layer {
    extends: Vec<String>,
    rules: Rules,
    overrides: Vec<Override>,
}

impl Layer {
    pub(crate) fn of(spec: &ProjectSpec) -> Result<Self, ProjectError> {
        let rules = |settings: &BTreeMap<String, RuleSetting>| -> Rules {
            settings
                .iter()
                .map(|(id, s)| (id.clone(), s.clone().into()))
                .collect()
        };
        let overrides = spec
            .overrides
            .iter()
            .map(|o| {
                Ok(Override {
                    files: globs(&o.files)?,
                    languages: o.languages.clone(),
                    rules: rules(&o.rules),
                })
            })
            .collect::<Result<_, ProjectError>>()?;
        Ok(Self {
            extends: spec.extends.clone(),
            rules: rules(&spec.rules),
            overrides,
        })
    }

    /// Project names named in `extends`, in declaration order.
    pub fn extends(&self) -> &[String] {
        &self.extends
    }

    /// Renames the rules this layer sets (in `rules` and in every override)
    /// by `names`, old id to new; returns what it renamed. A rule already set
    /// under the new id keeps its own setting, and the rename says so.
    pub fn rename_rules(&mut self, names: &BTreeMap<String, String>) -> Vec<Renamed> {
        let mut renamed: Vec<Renamed> = Vec::new();
        let sets =
            std::iter::once(&mut self.rules).chain(self.overrides.iter_mut().map(|o| &mut o.rules));
        for rules in sets {
            for (old, new) in names {
                let Some(config) = rules.remove(old) else {
                    continue;
                };
                let both = rules.contains_key(new);
                rules.entry(new.clone()).or_insert(config);
                match renamed.iter_mut().find(|r| r.old == *old) {
                    Some(seen) => seen.both |= both,
                    None => renamed.push(Renamed {
                        old: old.clone(),
                        new: new.clone(),
                        both,
                    }),
                }
            }
        }
        renamed
    }

    /// Entries named in `rules` and in every override, unmerged.
    pub fn configured(&self) -> impl Iterator<Item = (&str, &RuleConfig)> {
        self.rules
            .iter()
            .chain(self.overrides.iter().flat_map(|o| o.rules.iter()))
            .map(|(id, config)| (id.as_str(), config))
    }

    /// Like [`Layer::configured`], over this layer and every project it
    /// extends, the extended ones first.
    pub fn entries<'a>(
        &'a self,
        projects: &'a Projects,
    ) -> Result<Vec<(&'a str, &'a RuleConfig)>, ProjectError> {
        let mut out = Vec::new();
        self.collect(projects, &mut Vec::new(), &mut out)?;
        Ok(out)
    }

    fn collect<'a>(
        &'a self,
        projects: &'a Projects,
        stack: &mut Vec<&'a str>,
        out: &mut Vec<(&'a str, &'a RuleConfig)>,
    ) -> Result<(), ProjectError> {
        for name in &self.extends {
            let base = projects.extended(name, stack)?;
            stack.push(name);
            base.collect(projects, stack, out)?;
            stack.pop();
        }
        out.extend(self.configured());
        Ok(())
    }

    /// Effective rules for a file: the projects it extends, then `rules`,
    /// then matching overrides in order. `path` is relative to the project
    /// root.
    pub fn resolve(
        &self,
        path: &Path,
        lang: &str,
        projects: &Projects,
    ) -> Result<Rules, ProjectError> {
        let mut out = Rules::new();
        self.apply(path, lang, projects, &mut Vec::new(), &mut out)?;
        Ok(out)
    }

    fn apply<'a>(
        &'a self,
        path: &Path,
        lang: &str,
        projects: &'a Projects,
        stack: &mut Vec<&'a str>,
        out: &mut Rules,
    ) -> Result<(), ProjectError> {
        for name in &self.extends {
            let base = projects.extended(name, stack)?;
            stack.push(name);
            base.apply(path, lang, projects, stack, out)?;
            stack.pop();
        }
        rules::merge(out, &self.rules);
        for o in self.overrides.iter().filter(|o| o.matches(path, lang)) {
            rules::merge(out, &o.rules);
        }
        Ok(())
    }
}

/// The projects `extends` can name, by name.
#[derive(Debug, Clone, Default)]
pub struct Projects {
    by_name: BTreeMap<String, Layer>,
}

impl Projects {
    /// The projects the documents describe. Their `plugins` and `languages`
    /// are not used: only a project that is run has those.
    pub fn new(
        documents: impl IntoIterator<Item = Resource<ProjectSpec>>,
    ) -> Result<Self, ProjectError> {
        let mut by_name = BTreeMap::new();
        for Resource { metadata, spec } in documents {
            if by_name
                .insert(metadata.name.clone(), Layer::of(&spec)?)
                .is_some()
            {
                return Err(ProjectError::DuplicateProject(metadata.name));
            }
        }
        Ok(Self { by_name })
    }

    /// Renames the rules every project sets by `names`; see
    /// [`Layer::rename_rules`].
    pub fn rename_rules(&mut self, names: &BTreeMap<String, String>) -> Vec<Renamed> {
        let mut renamed: Vec<Renamed> = Vec::new();
        for layer in self.by_name.values_mut() {
            for r in layer.rename_rules(names) {
                match renamed.iter_mut().find(|seen| seen.old == r.old) {
                    Some(seen) => seen.both |= r.both,
                    None => renamed.push(r),
                }
            }
        }
        renamed
    }

    /// The names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    /// The project named `name`.
    pub fn get(&self, name: &str) -> Option<&Layer> {
        self.by_name.get(name)
    }

    /// The project `name` names as an extended one, unless it is unknown or
    /// already being extended (`stack`, outermost first).
    fn extended(&self, name: &str, stack: &[&str]) -> Result<&Layer, ProjectError> {
        if stack.contains(&name) {
            return Err(ProjectError::ProjectCycle(name.to_owned()));
        }
        self.get(name)
            .ok_or_else(|| ProjectError::UnknownProject(name.to_owned()))
    }
}

/// Parsed `Project`: the content of `lighthouse.toml`.
#[derive(Debug, Clone)]
pub struct Config {
    name: String,
    plugins: Vec<PluginRef>,
    languages: BTreeMap<String, Options>,
    formatters: BTreeMap<String, Formatter>,
    generated: Option<GlobSet>,
    generated_check: Option<GeneratedCheck>,
    layer: Layer,
}

impl Config {
    /// Parses `text` as a `lighthouse.toml`: a `Project` document. Unknown
    /// fields and invalid globs are errors.
    pub fn parse(text: &str) -> Result<Self, ProjectError> {
        Self::parse_as(Format::Toml, FILE_NAME, text)
    }

    /// Parses `text` in `format`; `path` only names it in errors.
    pub fn parse_as(format: Format, path: &str, text: &str) -> Result<Self, ProjectError> {
        let mut docs = documents(format, path, text)?;
        if docs.len() != 1 {
            return Err(ProjectError::Documents {
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
    pub fn parse_inline(text: &str) -> Result<Self, ProjectError> {
        let spec: ProjectSpec =
            toml::from_str(text).map_err(|e| lighthouse_resource::Error::Invalid {
                path: "<inline>".to_owned(),
                message: e.to_string(),
            })?;
        Self::from_resource(Resource::new(Metadata::named("inline"), spec))
    }

    /// Builds the configuration a parsed `Project` describes.
    pub fn from_resource(project: Resource<ProjectSpec>) -> Result<Self, ProjectError> {
        let Resource { metadata, spec } = project;
        let layer = Layer::of(&spec)?;
        let generated = globs(&spec.generated.files)?;
        let generated_check = spec.generated.check;
        let mut languages = BTreeMap::new();
        let mut formatters = BTreeMap::new();
        for (id, language) in spec.languages {
            if let Some(formatter) = language.formatter {
                let formatter = Formatter::from(formatter);
                if formatter.argv.is_empty() {
                    return Err(ProjectError::Formatter { language: id });
                }
                formatters.insert(id.clone(), formatter);
            }
            languages.insert(id, language.options);
        }
        Ok(Self {
            name: metadata.name,
            plugins: spec
                .plugins
                .into_iter()
                .map(PluginRef::of)
                .collect::<Result<_, _>>()?,
            languages,
            formatters,
            generated,
            generated_check,
            layer,
        })
    }

    /// Reads and parses the file at `path`, in the format its extension says.
    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        let text = fs::read_to_string(path).map_err(|source| ProjectError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::parse_as(
            Format::of_path(path).unwrap_or(Format::Toml),
            &path.display().to_string(),
            &text,
        )
    }

    /// Walks up from `start` and returns the first config file with its path.
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, Self)>, ProjectError> {
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
        self.formatters.iter().map(|(id, f)| (id.as_str(), f))
    }

    /// The formatter of a language, if its configuration names one.
    pub fn formatter(&self, language: &str) -> Option<&Formatter> {
        self.formatters.get(language)
    }

    /// `languages.<id>` options handed to the language's provider; the
    /// `formatter` key is the host's and is not among them.
    pub fn languages(&self) -> &BTreeMap<String, Options> {
        &self.languages
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

    /// The rules this project sets and the projects it extends.
    pub fn layer(&self) -> &Layer {
        &self.layer
    }

    /// Renames the rules the project sets by `names`, old id to new; see
    /// [`Layer::rename_rules`].
    pub fn rename_rules(&mut self, names: &BTreeMap<String, String>) -> Vec<Renamed> {
        self.layer.rename_rules(names)
    }

    /// Project names named in `extends`, in declaration order.
    pub fn extends(&self) -> &[String] {
        self.layer.extends()
    }

    /// Entries named in `rules` and in every override, unmerged.
    pub fn configured(&self) -> impl Iterator<Item = (&str, &RuleConfig)> {
        self.layer.configured()
    }

    /// Effective rules for a file; see [`Layer::resolve`].
    pub fn resolve(
        &self,
        path: &Path,
        lang: &str,
        projects: &Projects,
    ) -> Result<Rules, ProjectError> {
        self.layer.resolve(path, lang, projects)
    }
}

/// Compiles path globs. `*` and `?` never cross `/`; `**` as a whole path
/// component matches any number of directories (so `**` matches every path).
pub fn glob_set<I>(patterns: I) -> Result<GlobSet, ProjectError>
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
            .map_err(|source| ProjectError::Glob {
                pattern: pattern.to_owned(),
                source,
            })?;
        set.add(glob);
    }
    set.build().map_err(|source| ProjectError::Glob {
        pattern: all.join(", "),
        source,
    })
}

fn globs(patterns: &[String]) -> Result<Option<GlobSet>, ProjectError> {
    if patterns.is_empty() {
        return Ok(None);
    }
    glob_set(patterns).map(Some)
}
