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

pub const FILE_NAME: &str = "lighthouse.toml";

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
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            path: None,
            timeout_secs: None,
        }
    }

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
    extends: Vec<String>,
    rules: Rules,
    overrides: Vec<Override>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, Error> {
        let raw: Raw = toml::from_str(text)?;
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
            extends: raw.extends,
            rules: raw.rules,
            overrides,
        })
    }

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

    pub fn plugins(&self) -> &[PluginRef] {
        &self.plugins
    }

    /// Whether `id` is listed in `plugins`.
    pub fn lists(&self, id: &str) -> bool {
        self.plugins.iter().any(|p| p.id == id)
    }

    /// `[languages.<id>]` options handed to the language's provider.
    pub fn languages(&self) -> &BTreeMap<String, Options> {
        &self.languages
    }

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

fn globs(patterns: &[String]) -> Result<Option<GlobSet>, Error> {
    if patterns.is_empty() {
        return Ok(None);
    }
    glob_set(patterns).map(Some)
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
