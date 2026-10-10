//! The spec of the `Project` kind: what `lighthouse.toml` holds, and what a
//! shareable project is.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::Options;
use lighthouse_resource::Spec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{formatter::FormatterSpec, rules::RuleSetting};

/// A project's configuration, and the shape of a shareable one: `extends`
/// names any project, the way an ESLint config extends a shareable config.
/// An extended project contributes its `extends`, `rules` and `overrides`.
/// Its `plugins` and `languages` belong to the project that is run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectSpec {
    /// Plugins that provide rules and languages; a bare id searches the
    /// plugin locations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginEntry>,
    /// Projects whose rules apply first, in order: the `recommended` and
    /// `strict` projects of each pack (`core/recommended`), or any `Project`
    /// document of the catalog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    /// Options per language id; `formatter` is the host's, the rest goes to
    /// the language's provider.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, ProjectLanguage>,
    /// Level and options per decision id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleSetting>,
    /// Rules that apply to some files or languages only, after `rules`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideSpec>,
    /// Which files are generated code, beyond what the providers and
    /// `.gitattributes` say, and whether it is checked.
    #[serde(default, skip_serializing_if = "GeneratedSpec::is_empty")]
    pub generated: GeneratedSpec,
}

/// What a project says about generated code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneratedSpec {
    /// Globs of project-relative paths that are generated code.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// `skip`: no decision checks generated code; `include`: every decision
    /// does. Unset, each decision's own scope says. A rule's `generated`
    /// setting wins over both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<GeneratedCheck>,
}

impl GeneratedSpec {
    /// Whether the project says nothing about generated code.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.check.is_none()
    }
}

/// Whether decisions check generated code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GeneratedCheck {
    Skip,
    Include,
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
pub struct ProjectLanguage {
    /// The command that formats a file of the language after a fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formatter: Option<FormatterSpec>,
    /// The name prefixes that make a function a constructor in this
    /// language, replacing what its provider declares. Every decision that
    /// tells a constructor apart shares them.
    #[serde(
        rename = "constructorPrefixes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub constructor_prefixes: Option<Vec<String>>,
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
