//! The spec of the `Project` kind: what `lighthouse.toml` holds, and what a
//! shareable project is.

use std::{collections::BTreeMap, path::PathBuf};

use lighthouse_model::{Duration, GeneratedSpec, Options, Spec};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;

use super::{
    fixing::FixSpec, models::ModelsSpec, rules::RuleSetting, sandbox::SandboxSpec,
    signals::SignalsSpec,
};

/// A project's configuration, and the shape of a shareable one: `extends`
/// names any project, the way an ESLint config extends a shareable config.
/// An extended project contributes its `extends`, `rules`, `overrides`,
/// `injections`, `ignores`, `generated` and `plugins`, the way an ESLint shareable config brings its
/// `plugins`. Its `languages` belong to the project that is run.
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
    /// Regions of host files that are written in another language: a CEL
    /// expression in a decision, code in a Markdown fence. The host language
    /// is inferred by normal file routing. Entries merge over `extends`,
    /// extended first; a later entry with the same `id` replaces the earlier.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<InjectionSpec>,
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
    /// How long session signals are kept, and whether the agent hears about
    /// them.
    #[serde(default, skip_serializing_if = "SignalsSpec::is_empty")]
    pub signals: SignalsSpec,
    /// Files that are not the project's: gitignore patterns, rooted at the
    /// project, that keep files out of the analysis altogether (never
    /// walked, indexed, checked, copied or diffed), such as fixtures that are
    /// broken on purpose. A plugin's `ignores` and those of the projects
    /// `extends` names come first; a later pattern wins, as in a
    /// `.gitignore`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignores: Vec<String>,
    /// Which files are generated code, beyond what `.gitattributes` and the
    /// listed plugins say.
    #[serde(default, skip_serializing_if = "GeneratedSpec::is_empty")]
    pub generated: GeneratedSpec,
}

/// A region of host files analyzed as another language, shaped like a
/// tree-sitter injection: `select` is `injection.content`, `language` the
/// fixed `injection.language`, `languageFrom` the dynamic
/// `@injection.language` capture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InjectionSpec {
    /// Stable identity within `spec.injections`.
    pub id: String,
    /// Project-relative physical host-file globs.
    pub files: Vec<String>,
    /// One JSONPath (RFC 9535) over the host provider's selectable tree,
    /// picking the content of each region.
    pub select: String,
    /// The fixed language of every region: an id or alias of a provider.
    /// Exactly one of `language` and `languageFrom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Region-relative selector returning the name of the language of each
    /// region, as a document writes it: one of the aliases a provider
    /// declares, in any case. A region whose name is absent or unknown is
    /// not analyzed; it is text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_from: Option<String>,
    /// Optional interpretation owned by the language's provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax: Option<String>,
    /// Optional provider-specific values and selector projections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
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
