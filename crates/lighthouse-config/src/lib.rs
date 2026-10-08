mod formatter;
mod project;
mod rules;

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
pub use project::{
    LanguageSpec, OverrideSpec, PluginEntry, PluginRefSpec, PresetSpec, ProjectSpec,
};
pub use rules::{Level, RuleConfig, RuleDetail, RuleSetting, Rules};
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

/// The keys a `lighthouse.toml` from before the resource model had.
const LEGACY_KEYS: [&str; 5] = ["plugins", "languages", "extends", "rules", "overrides"];

/// Failure to read, parse or apply a configuration; every variant names the offending input.
#[derive(Debug, Error)]
pub enum Error {
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
    #[error("unknown preset `{0}`")]
    UnknownPreset(String),
    #[error("preset `{0}` extends itself")]
    PresetCycle(String),
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

#[derive(Debug)]
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

    fn of(entry: PluginEntry) -> Result<Self, Error> {
        match entry {
            PluginEntry::Id(id) => Ok(Self::new(id)),
            PluginEntry::Detailed(detail) => {
                let timeout = detail
                    .timeout
                    .map(|text| {
                        parse_duration(&text).ok_or_else(|| Error::Duration {
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

/// Parsed `Project`: the content of `lighthouse.toml`.
#[derive(Debug)]
pub struct Config {
    name: String,
    plugins: Vec<PluginRef>,
    languages: BTreeMap<String, Options>,
    formatters: BTreeMap<String, Formatter>,
    extends: Vec<String>,
    rules: Rules,
    overrides: Vec<Override>,
}

impl Override {
    fn matches(&self, path: &Path, lang: &str) -> bool {
        self.files.as_ref().is_none_or(|g| g.is_match(path))
            && (self.languages.is_empty() || self.languages.iter().any(|l| l == lang))
    }
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
        let mut languages = BTreeMap::new();
        let mut formatters = BTreeMap::new();
        for (id, language) in spec.languages {
            if let Some(formatter) = language.formatter {
                let formatter = Formatter::from(formatter);
                if formatter.argv.is_empty() {
                    return Err(Error::Formatter { language: id });
                }
                formatters.insert(id.clone(), formatter);
            }
            languages.insert(id, language.options);
        }
        let rules = |settings: BTreeMap<String, RuleSetting>| -> Rules {
            settings.into_iter().map(|(id, s)| (id, s.into())).collect()
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
        Ok(Self {
            name: metadata.name,
            plugins: spec
                .plugins
                .into_iter()
                .map(PluginRef::of)
                .collect::<Result<_, _>>()?,
            languages,
            formatters,
            extends: spec.extends,
            rules: rules(spec.rules),
            overrides,
        })
    }

    /// Reads and parses the file at `path`, in the format its extension says.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(|source| Error::Io {
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
        FILE_NAMES.iter().map(|n| dir.join(n)).find(|p| p.is_file())
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

    /// Preset ids named in `extends`, in declaration order.
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

    /// Effective rules for a file: extends, then `rules`, then matching
    /// overrides in order. `path` is relative to the config directory.
    /// `presets` supplies the rules of a preset id, already flattened.
    pub fn resolve(
        &self,
        path: &Path,
        lang: &str,
        presets: &dyn Fn(&str) -> Option<Rules>,
    ) -> Result<Rules, Error> {
        let mut out = Rules::new();
        for id in &self.extends {
            let preset = presets(id).ok_or_else(|| Error::UnknownPreset(id.clone()))?;
            rules::merge(&mut out, &preset);
        }
        rules::merge(&mut out, &self.rules);
        for o in self.overrides.iter().filter(|o| o.matches(path, lang)) {
            rules::merge(&mut out, &o.rules);
        }
        Ok(out)
    }
}

/// The JSON Schema of the kinds this crate defines.
pub fn descriptors() -> Vec<lighthouse_resource::Descriptor> {
    use lighthouse_resource::Descriptor;
    vec![
        Descriptor::of::<ProjectSpec>(),
        Descriptor::of::<PresetSpec>(),
    ]
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

/// Whether `old` is shaped like a configuration from before the resource
/// model: a table with at least one of its keys. Any other table is not
/// Lighthouse's.
pub fn is_legacy(old: &serde_json::Value) -> bool {
    old.as_object()
        .is_some_and(|table| LEGACY_KEYS.iter().any(|key| table.contains_key(*key)))
}

/// A `Project` document named `name` from a `lighthouse.toml` (or its YAML or
/// JSON equivalent) from before the resource model: `timeout` numbers become
/// durations, rule options move under `options`, `review` becomes `info` and
/// `inPlace` becomes `in-place`.
pub fn migrate(old: &serde_json::Value, name: &str) -> Result<serde_json::Value, String> {
    use serde_json::{Map, Value, json};
    let old = old.as_object().ok_or("a configuration is a table")?;
    let mut spec = Map::new();
    for (key, value) in old {
        let value = match key.as_str() {
            "plugins" => migrate_plugins(value)?,
            "languages" => migrate_languages(value),
            "rules" => migrate_rules(value)?,
            "overrides" => migrate_overrides(value)?,
            "extends" => value.clone(),
            other => return Err(format!("unknown key `{other}`")),
        };
        spec.insert(key.clone(), value);
    }
    Ok(json!({
        "apiVersion": lighthouse_resource::API_VERSION,
        "kind": "Project",
        "metadata": { "name": name },
        "spec": Value::Object(spec),
    }))
}

fn globs(patterns: &[String]) -> Result<Option<GlobSet>, Error> {
    if patterns.is_empty() {
        return Ok(None);
    }
    glob_set(patterns).map(Some)
}

fn migrate_plugins(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let list = value.as_array().ok_or("`plugins` is a list")?;
    let migrated = list.iter().map(|plugin| match plugin {
        Value::Object(table) => {
            let mut table = table.clone();
            if let Some(Value::Number(seconds)) = table.get("timeout") {
                let text = format!("{seconds}s");
                table.insert("timeout".to_owned(), Value::String(text));
            }
            Value::Object(table)
        }
        other => other.clone(),
    });
    Ok(Value::Array(migrated.collect()))
}

fn migrate_languages(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let Value::Object(languages) = value else {
        return value.clone();
    };
    let mut out = languages.clone();
    for language in out.values_mut() {
        let Some(Value::Object(table)) = language.get_mut("formatter") else {
            continue;
        };
        if table.get("output").and_then(Value::as_str) == Some("inPlace") {
            table.insert("output".to_owned(), Value::String("in-place".to_owned()));
        }
    }
    Value::Object(out)
}

fn migrate_rules(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::{Map, Value};
    let rules = value.as_object().ok_or("`rules` is a table")?;
    let mut out = Map::new();
    for (id, rule) in rules {
        let migrated = match rule {
            Value::String(level) => Value::String(migrate_level(level)),
            Value::Object(table) => {
                let mut table = table.clone();
                let level = match table.remove("level") {
                    Some(Value::String(level)) => migrate_level(&level),
                    _ => return Err(format!("rule `{id}` is a table without a string `level`")),
                };
                let mut detail = Map::new();
                detail.insert("level".to_owned(), Value::String(level));
                if !table.is_empty() {
                    detail.insert("options".to_owned(), Value::Object(table));
                }
                Value::Object(detail)
            }
            _ => return Err(format!("rule `{id}` is a level string or a table")),
        };
        out.insert(id.clone(), migrated);
    }
    Ok(Value::Object(out))
}

fn migrate_overrides(value: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let list = value.as_array().ok_or("`overrides` is a list")?;
    let mut out = Vec::new();
    for item in list {
        let mut item = item.as_object().ok_or("an override is a table")?.clone();
        if let Some(rules) = item.get("rules") {
            let migrated = migrate_rules(rules)?;
            item.insert("rules".to_owned(), migrated);
        }
        out.push(Value::Object(item));
    }
    Ok(Value::Array(out))
}

/// `review` was a level; it is `info` now.
fn migrate_level(level: &str) -> String {
    if level == "review" { "info" } else { level }.to_owned()
}
