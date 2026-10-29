//! The spec of the `Project` kind: what `lighthouse.toml` holds, and what a
//! shareable project is.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::Options;
use lighthouse_resource::{Duration, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;

use super::{fixing::FixSpec, models::ModelsSpec, rules::RuleSetting, sandbox::SandboxSpec};

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
    /// Options per language id; `constructorPrefixes` is the host's, the rest
    /// goes to the language's provider.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub languages: BTreeMap<String, ProjectLanguage>,
    /// Embedded source rules. The host language is inferred by normal file routing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRuleSpec>,
    /// Level and options per decision id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, RuleSetting>,
    /// Rules that apply to some files or languages only, after `rules`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideSpec>,
    /// The models that judge the decisions rules cannot prove.
    #[serde(default, skip_serializing_if = "ModelsSpec::is_empty")]
    pub models: ModelsSpec,
    /// The sandbox the programs of the project run in.
    #[serde(default, skip_serializing_if = "SandboxSpec::is_empty")]
    pub sandbox: SandboxSpec,
    /// What fixes may do, and how fixes by models are checked beyond the rules.
    #[serde(default, skip_serializing_if = "FixSpec::is_empty")]
    pub fix: FixSpec,
    /// Which files are generated code, beyond what the providers and
    /// `.gitattributes` say, and whether it is checked.
    #[serde(default, skip_serializing_if = "GeneratedSpec::is_empty")]
    pub generated: GeneratedSpec,
}

/// A source selection analyzed through a registered provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceRuleSpec {
    /// Stable rule identity within spec.sources.
    pub id: String,
    /// Project-relative physical host-file globs.
    pub files: Vec<String>,
    /// One selector over the host provider's selectable tree.
    pub select: String,
    /// Provider selection and provider-specific interpretation/context.
    pub language: SourceLanguageSpec,
}

/// Analysis destination for a source rule. Exactly one of provider and from
/// is required; from reads the language of each candidate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceLanguageSpec {
    /// Fixed opaque key in the provider registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Candidate-relative selector returning the name of the language of each
    /// candidate, as a document writes it: one of the aliases a provider
    /// declares, in any case. A candidate whose name is absent or unknown is
    /// not analyzed; it is text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Optional interpretation owned by the selected provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    /// Optional provider-specific values and selector projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum PluginEntry {
    Id(String),
    Detailed(PluginRefSpec),
}

impl<'de> Deserialize<'de> for PluginEntry {
    /// Names what was expected, which an untagged enum's own error does not.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(id) => Ok(Self::Id(id)),
            table @ Value::Object(_) => PluginRefSpec::deserialize(table)
                .map(Self::Detailed)
                .map_err(D::Error::custom),
            other => Err(D::Error::custom(format!(
                "a plugin is an id or a table with an `id`, not {other}"
            ))),
        }
    }
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
    pub timeout: Option<Duration>,
}

/// The options of one language.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProjectLanguage {
    /// The name prefixes that make a function a constructor in this
    /// language, replacing what its provider declares. Every decision that
    /// tells a constructor apart shares them.
    #[serde(
        rename = "constructorPrefixes",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub constructor_prefixes: Option<Vec<String>>,
    /// The parts of the test naming convention this project replaces in this
    /// language; the others stay what its provider declares.
    #[serde(
        rename = "testNaming",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub test_naming: Option<TestNamingSpec>,
    /// Everything else is handed to the language's provider.
    #[serde(flatten)]
    pub options: Options,
}

/// The parts of a language's test naming convention a project replaces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TestNamingSpec {
    /// Prefix of an owner test's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Names are compared in snake case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snake_case: Option<bool>,
    /// `TestGet_Missing` is a second owner of `Get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<bool>,
    /// The tests of a module that tests an ancestor module count for the
    /// nested module too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestors: Option<bool>,
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
