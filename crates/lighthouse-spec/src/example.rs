use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Whether an example must produce no diagnostics (`valid`) or exactly those
/// listed in `expect` (`invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExampleKind {
    Valid,
    Invalid,
}

impl fmt::Display for ExampleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Valid => "valid",
            Self::Invalid => "invalid",
        })
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RawFile {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

/// One file of an example project, addressed by `path` inside the example.
/// Its text is a `body` written in the decision file, or a `source` file next
/// to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "RawFile", into = "RawFile")]
#[schemars(with = "RawFile")]
pub struct ExampleFile {
    pub path: String,
    text: String,
    source: Option<String>,
}

impl ExampleFile {
    /// A file whose text is `body`, written in the decision file itself.
    pub fn inline(path: &str, body: &str) -> Self {
        Self {
            path: path.to_owned(),
            text: body.to_owned(),
            source: None,
        }
    }

    /// The file's text; a `source` file's is loaded by the catalog.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The file that holds the text, relative to the directory of the
    /// decision's file; `None` for a text written inline.
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub(crate) fn with_text(self, text: String) -> Self {
        Self { text, ..self }
    }
}

impl TryFrom<RawFile> for ExampleFile {
    type Error = String;

    fn try_from(raw: RawFile) -> Result<Self, String> {
        match (raw.body, raw.source) {
            (Some(text), None) => Ok(Self {
                path: raw.path,
                text,
                source: None,
            }),
            (None, Some(source)) => Ok(Self {
                path: raw.path,
                text: String::new(),
                source: Some(source),
            }),
            _ => Err(format!(
                "`{}` needs exactly one of `body` or `source`",
                raw.path
            )),
        }
    }
}

impl From<ExampleFile> for RawFile {
    fn from(file: ExampleFile) -> Self {
        Self {
            path: file.path,
            body: file.source.is_none().then_some(file.text),
            source: file.source,
        }
    }
}

/// An expected diagnostic of an invalid example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A fixture project for one language and what a rule must report on it:
/// nothing for `valid`, the diagnostics of `expect` for `invalid`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Example {
    pub name: String,
    pub language: String,
    pub kind: ExampleKind,
    pub files: Vec<ExampleFile>,
    /// The example that best shows the decision in its language; at most one
    /// per language and kind. Agent output prefers it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub canonical: bool,
    /// Diagnostics an invalid example must produce.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expect: Vec<Expect>,
    /// Rule options the example runs with.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,
    /// What an invalid example's files become when the decision's fix is
    /// applied: the files that change, each with its whole new text. Applying
    /// the fix again must change nothing, and the rule must no longer fire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<ExampleFile>,
}
