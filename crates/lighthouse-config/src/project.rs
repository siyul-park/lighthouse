//! The spec of the `Project` and `Preset` kinds: what `lighthouse.toml`
//! holds, and what a preset is.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::Options;
use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{formatter::FormatterSpec, rules::RuleSetting};

/// A project's configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    /// Plugins that provide rules, languages and presets; a bare id searches
    /// the plugin locations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginEntry>,
    /// Presets whose rules apply first, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    /// Options per language id; `formatter` is the host's, the rest goes to
    /// the language's provider.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, LanguageSpec>,
    /// Level and options per decision id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleSetting>,
    /// Rules that apply to some files or languages only, after `rules`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideSpec>,
}

impl Spec for ProjectSpec {
    const KIND: &'static str = "Project";
}

/// A plugin in `plugins`: a bare id, or an id with where to find it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PluginEntry {
    Id(String),
    Detailed(PluginRefSpec),
}

/// A listed plugin. `path` (relative to the config directory) names the
/// plugin directory instead of searching the plugin locations; `timeout` is
/// the per-request limit for process plugins, such as `30s`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginRefSpec {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
}

/// The options of one language.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LanguageSpec {
    /// The command that formats a file of the language after a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatter: Option<FormatterSpec>,
    /// Everything else is handed to the language's provider.
    #[serde(flatten)]
    pub options: Options,
}

/// Rules for the files and languages that match.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverrideSpec {
    /// Globs of project-relative paths; empty matches every path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Language ids; empty matches every language.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleSetting>,
}

/// Named rule configuration, referenced from `extends`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresetSpec {
    /// Presets whose rules this one starts from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleSetting>,
}

impl Spec for PresetSpec {
    const KIND: &'static str = "Preset";
}
