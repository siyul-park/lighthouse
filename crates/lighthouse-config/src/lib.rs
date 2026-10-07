mod rules;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub use globset::GlobSet;
use globset::{GlobBuilder, GlobSetBuilder};
use lighthouse_model::Options;
use serde::Deserialize;
use thiserror::Error;

pub use rules::{RuleConfig, Rules};

/// Name of the configuration file discovered in a project directory.
pub const FILE_NAME: &str = "lighthouse.toml";

/// Failure to read, parse or apply a configuration; every variant names the offending input.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid {FILE_NAME}: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid glob `{pattern}`: {source}")]
    Glob {
        pattern: String,
        source: globset::Error,
    },
    #[error("unknown preset `{0}`")]
    UnknownPreset(String),
    #[error(
        "invalid {FILE_NAME}: `[languages.{language}] formatter` must be a non-empty list of strings, or a table with `argv` and optional `stdin` (none|file), `output` (inPlace|text) and `env`"
    )]
    Formatter { language: String },
}

/// What a formatter is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FormatterStdin {
    #[default]
    None,
    /// The text of the file.
    File,
}

/// Where a formatter leaves the formatted text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FormatterOutput {
    /// In the scratch copy of the file, which is appended to `argv` (or fills
    /// an `{file}` argument).
    #[default]
    InPlace,
    /// On stdout, which carries only the text.
    Text,
}

/// The command that formats a file of a language, by the command contract of
/// fixes: exit `0` succeeds, stdout carries only the result, stderr is for
/// people. `["gofmt", "-w"]` is the short form: scratch copy, path appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatter {
    pub argv: Vec<String>,
    pub stdin: FormatterStdin,
    pub output: FormatterOutput,
    /// Extra environment variables; the rest is cleared to an allowlist.
    pub env: BTreeMap<String, String>,
}

impl Formatter {
    fn parse(value: &serde_json::Value) -> Option<Self> {
        let words = |v: &serde_json::Value| -> Option<Vec<String>> {
            v.as_array()?
                .iter()
                .map(|w| w.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .filter(|w| !w.is_empty())
        };
        if let Some(argv) = words(value) {
            return Some(Self {
                argv,
                stdin: FormatterStdin::None,
                output: FormatterOutput::InPlace,
                env: BTreeMap::new(),
            });
        }
        let table = value.as_object()?;
        if table
            .keys()
            .any(|k| !["argv", "stdin", "output", "env"].contains(&k.as_str()))
        {
            return None;
        }
        let stdin = match table.get("stdin").map(|v| v.as_str()) {
            None | Some(Some("none")) => FormatterStdin::None,
            Some(Some("file")) => FormatterStdin::File,
            _ => return None,
        };
        let output = match table.get("output").map(|v| v.as_str()) {
            None | Some(Some("inPlace")) => FormatterOutput::InPlace,
            Some(Some("text")) => FormatterOutput::Text,
            _ => return None,
        };
        let env = match table.get("env") {
            None => BTreeMap::new(),
            Some(v) => v
                .as_object()?
                .iter()
                .map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                .collect::<Option<_>>()?,
        };
        Some(Self {
            argv: words(table.get("argv")?)?,
            stdin,
            output,
            env,
        })
    }
}

/// A plugin listed in `plugins`: a bare id, or `{ id, path, timeout }`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum RawPlugin {
    Id(String),
    Detailed(PluginRef),
}

/// A listed plugin. `path` (relative to the config directory) names the
/// plugin directory explicitly instead of searching the plugin locations;
/// `timeout` is the per-request limit in seconds for process plugins.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRef {
    pub id: String,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default, rename = "timeout")]
    timeout_secs: Option<u64>,
}

impl PluginRef {
    /// A listed plugin with no explicit path and no timeout override.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            path: None,
            timeout_secs: None,
        }
    }

    /// The per-request limit, if one was configured.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout_secs.map(Duration::from_secs)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    #[serde(default)]
    plugins: Vec<RawPlugin>,
    #[serde(default)]
    languages: BTreeMap<String, Options>,
    #[serde(default)]
    extends: Vec<String>,
    #[serde(default)]
    rules: Rules,
    #[serde(default)]
    overrides: Vec<RawOverride>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOverride {
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    rules: Rules,
}

#[derive(Debug)]
struct Override {
    files: Option<GlobSet>,
    languages: Vec<String>,
    rules: Rules,
}

impl Override {
    fn matches(&self, path: &Path, lang: &str) -> bool {
        self.files.as_ref().is_none_or(|g| g.is_match(path))
            && (self.languages.is_empty() || self.languages.iter().any(|l| l == lang))
    }
}

/// Parsed `lighthouse.toml`.
#[derive(Debug)]
pub struct Config {
    plugins: Vec<PluginRef>,
    languages: BTreeMap<String, Options>,
    formatters: BTreeMap<String, Formatter>,
    extends: Vec<String>,
    rules: Rules,
    overrides: Vec<Override>,
}

impl Config {
    /// Parses `text` as a `lighthouse.toml`; unknown fields and invalid globs are errors.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut raw: Raw = toml::from_str(text)?;
        let formatters = take_formatters(&mut raw.languages)?;
        let overrides = raw
            .overrides
            .into_iter()
            .map(|o| {
                Ok(Override {
                    files: globs(&o.files)?,
                    languages: o.languages,
                    rules: o.rules,
                })
            })
            .collect::<Result<_, Error>>()?;
        Ok(Self {
            plugins: raw
                .plugins
                .into_iter()
                .map(|p| match p {
                    RawPlugin::Id(id) => PluginRef::new(id),
                    RawPlugin::Detailed(plugin) => plugin,
                })
                .collect(),
            languages: raw.languages,
            formatters,
            extends: raw.extends,
            rules: raw.rules,
            overrides,
        })
    }

    /// Reads and parses the file at `path`.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Walks up from `start` and returns the first config file with its path.
    pub fn discover(start: &Path) -> Result<Option<(PathBuf, Self)>, Error> {
        for dir in start.ancestors() {
            let path = dir.join(FILE_NAME);
            if path.is_file() {
                return Ok(Some((path.clone(), Self::load(&path)?)));
            }
        }
        Ok(None)
    }

    /// Listed plugins in declaration order; bare ids become [`PluginRef::new`].
    pub fn plugins(&self) -> &[PluginRef] {
        &self.plugins
    }

    /// Whether `id` is listed in `plugins`.
    pub fn lists(&self, id: &str) -> bool {
        self.plugins.iter().any(|p| p.id == id)
    }

    /// The command that formats a file of the language, `[languages.<id>]
    /// formatter`.
    /// Every language's formatter, by language id.
    pub fn formatters(&self) -> impl Iterator<Item = (&str, &Formatter)> {
        self.formatters.iter().map(|(id, f)| (id.as_str(), f))
    }

    /// The formatter of a language, if its configuration names one.
    pub fn formatter(&self, language: &str) -> Option<&Formatter> {
        self.formatters.get(language)
    }

    /// `[languages.<id>]` options handed to the language's provider; the
    /// `formatter` key is the host's and is not among them.
    pub fn languages(&self) -> &BTreeMap<String, Options> {
        &self.languages
    }

    /// Preset ids named in `extends`, in declaration order.
    pub fn extends(&self) -> &[String] {
        &self.extends
    }

    /// Entries named in `[rules]` and in every override, unmerged.
    pub fn configured(&self) -> impl Iterator<Item = (&str, &RuleConfig)> {
        self.rules
            .iter()
            .chain(self.overrides.iter().flat_map(|o| o.rules.iter()))
            .map(|(id, config)| (id.as_str(), config))
    }

    /// Effective rules for a file: extends, then `[rules]`, then matching
    /// overrides in order. `path` is relative to the config directory.
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

/// Removes the host's `formatter` key from each language's options.
fn take_formatters(
    languages: &mut BTreeMap<String, Options>,
) -> Result<BTreeMap<String, Formatter>, Error> {
    let mut formatters = BTreeMap::new();
    for (language, options) in languages.iter_mut() {
        let Some(value) = options.remove("formatter") else {
            continue;
        };
        let bad = || Error::Formatter {
            language: language.clone(),
        };
        let formatter = Formatter::parse(&value).ok_or_else(bad)?;
        formatters.insert(language.clone(), formatter);
    }
    Ok(formatters)
}

fn globs(patterns: &[String]) -> Result<Option<GlobSet>, Error> {
    if patterns.is_empty() {
        return Ok(None);
    }
    glob_set(patterns).map(Some)
}
