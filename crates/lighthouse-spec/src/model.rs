use std::{collections::BTreeMap, fmt};

use lighthouse_model::Severity;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Error;

/// What a pattern talks about. Mapping to the run scope of a rule is
/// `Scope::rule_scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Symbol,
    File,
    Module,
    Project,
    Test,
}

/// How a pattern can be verified; it fixes the default severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Enforcement {
    Mechanical,
    Heuristic,
    Judgment,
    Doc,
}

impl Enforcement {
    /// `None` for `doc`: guidance without a verdict.
    pub fn default_severity(self) -> Option<Severity> {
        match self {
            Self::Mechanical => Some(Severity::Error),
            Self::Heuristic => Some(Severity::Warn),
            Self::Judgment => Some(Severity::Review),
            Self::Doc => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionType {
    Int,
    Float,
    Bool,
    String,
    List,
}

/// A tunable value of a pattern. Rules read defaults from here, not from code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionSpec {
    #[serde(rename = "type")]
    pub kind: OptionType,
    pub default: Value,
    pub description: String,
    /// Replaces `default` for one language.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub per_language: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawImplementation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    builtin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    declarative: Option<String>,
}

/// How a pattern is checked. Declarative rules arrive with the declarative
/// engine; only the schema exists today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawImplementation", into = "RawImplementation")]
pub enum Implementation {
    /// Id of a rule registered by a bundled plugin.
    Builtin(String),
    /// Path of a rule definition, relative to the catalog root.
    Declarative(String),
}

impl TryFrom<RawImplementation> for Implementation {
    type Error = String;

    fn try_from(raw: RawImplementation) -> Result<Self, String> {
        match (raw.builtin, raw.declarative) {
            (Some(rule), None) => Ok(Self::Builtin(rule)),
            (None, Some(path)) => Ok(Self::Declarative(path)),
            _ => {
                Err("an implementation needs exactly one of `builtin` or `declarative`".to_owned())
            }
        }
    }
}

impl From<Implementation> for RawImplementation {
    fn from(implementation: Implementation) -> Self {
        match implementation {
            Implementation::Builtin(rule) => Self {
                builtin: Some(rule),
                declarative: None,
            },
            Implementation::Declarative(path) => Self {
                builtin: None,
                declarative: Some(path),
            },
        }
    }
}

/// Where a fixture file's text comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Inline(String),
    /// Path relative to the pattern's section directory.
    File(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

/// One file of an example project, addressed by `path` inside the example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawFile", into = "RawFile")]
pub struct ExampleFile {
    pub path: String,
    pub content: Content,
    loaded: Option<String>,
}

impl ExampleFile {
    pub fn inline(path: &str, body: &str) -> Self {
        Self {
            path: path.to_owned(),
            content: Content::Inline(body.to_owned()),
            loaded: None,
        }
    }

    /// The file's text, with `Content::File` already loaded by the catalog.
    pub fn text(&self) -> &str {
        match &self.content {
            Content::Inline(body) => body,
            Content::File(_) => self.loaded.as_deref().unwrap_or_default(),
        }
    }

    pub(crate) fn with_loaded(self, text: String) -> Self {
        Self {
            loaded: Some(text),
            ..self
        }
    }
}

impl TryFrom<RawFile> for ExampleFile {
    type Error = String;

    fn try_from(raw: RawFile) -> Result<Self, String> {
        let content = match (raw.body, raw.source) {
            (Some(body), None) => Content::Inline(body),
            (None, Some(source)) => Content::File(source),
            _ => {
                return Err(format!(
                    "`{}` needs exactly one of `body` or `source`",
                    raw.path
                ));
            }
        };
        Ok(Self {
            path: raw.path,
            content,
            loaded: None,
        })
    }
}

impl From<ExampleFile> for RawFile {
    fn from(file: ExampleFile) -> Self {
        let (body, source) = match file.content {
            Content::Inline(body) => (Some(body), None),
            Content::File(source) => (None, Some(source)),
        };
        Self {
            path: file.path,
            body,
            source,
        }
    }
}

/// An expected diagnostic of an invalid example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub name: String,
    pub language: String,
    pub kind: Kind,
    pub files: Vec<ExampleFile>,
    /// Diagnostics an invalid example must produce.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<Expect>,
    /// Rule options the example runs with.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    /// `<pack>/<name>`.
    pub id: String,
    pub title: String,
    pub intent: String,
    pub scope: Scope,
    pub requirement: String,
    pub enforcement: Enforcement,
    #[serde(default, rename = "severity", skip_serializing_if = "Option::is_none")]
    pub severity_override: Option<Severity>,
    /// Fields a checker emits as diagnostic evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<Example>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exceptions: Option<String>,
    /// Per-language wording.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tuning: BTreeMap<String, String>,
    /// Tunable values, keyed by option name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, OptionSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation: Option<Implementation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<String>,
}

impl Pattern {
    pub fn severity(&self) -> Option<Severity> {
        self.severity_override
            .or_else(|| self.enforcement.default_severity())
    }

    /// Defaults filled in and configured keys checked against the declared
    /// options; `language` selects per-language defaults.
    pub fn resolve_options(
        &self,
        configured: &Map<String, Value>,
        language: Option<&str>,
    ) -> Result<Map<String, Value>, Error> {
        for (key, value) in configured {
            let spec = self
                .options
                .get(key)
                .ok_or_else(|| Error::invalid(&self.id, format!("unknown option `{key}`")))?;
            if !spec.kind.accepts(value) {
                return Err(Error::invalid(
                    &self.id,
                    format!("option `{key}` must be {}", spec.kind),
                ));
            }
        }
        let mut resolved = Map::new();
        for (key, spec) in &self.options {
            let value = configured
                .get(key)
                .or_else(|| language.and_then(|l| spec.per_language.get(l)))
                .unwrap_or(&spec.default);
            resolved.insert(key.clone(), value.clone());
        }
        Ok(resolved)
    }
}

impl OptionType {
    pub(crate) fn accepts(self, value: &Value) -> bool {
        match self {
            Self::Int => value.is_i64() || value.is_u64(),
            Self::Float => value.is_number(),
            Self::Bool => value.is_boolean(),
            Self::String => value.is_string(),
            Self::List => value.is_array(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub patterns: Vec<Pattern>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pack {
    pub id: String,
    pub title: String,
    pub intro: String,
    pub sections: Vec<Section>,
}

macro_rules! display {
    ($ty:ty { $($variant:ident => $text:literal),+ $(,)? }) => {
        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self { $(Self::$variant => $text),+ })
            }
        }
    };
}

display!(Scope { Symbol => "symbol", File => "file", Module => "module", Project => "project", Test => "test" });
display!(Enforcement { Mechanical => "mechanical", Heuristic => "heuristic", Judgment => "judgment", Doc => "doc" });
display!(Kind { Valid => "valid", Invalid => "invalid" });
display!(OptionType { Int => "int", Float => "float", Bool => "bool", String => "string", List => "list" });
