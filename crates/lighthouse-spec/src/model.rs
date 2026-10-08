use std::fmt;

use lighthouse_model::Severity;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// What kind of thing a decision is about. Only code exists today.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Domain {
    #[default]
    Code,
}

/// What a decision talks about inside its domain. Mapping to the run scope
/// of a rule is `Subject::rule_scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Subject {
    Symbol,
    File,
    Module,
    Project,
    Test,
}

impl Subject {
    /// Symbol, file and test decisions are checked once per file; module and
    /// project decisions once over the merged project.
    pub fn rule_scope(self) -> lighthouse_plugin::Scope {
        use lighthouse_plugin::Scope as RunScope;
        match self {
            Self::Symbol | Self::File | Self::Test => RunScope::File,
            Self::Module | Self::Project => RunScope::Project,
        }
    }
}

/// Where a decision applies: its domain and what it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(default)]
    pub domain: Domain,
    pub subject: Subject,
}

impl Scope {
    /// A scope in the code domain.
    pub fn code(subject: Subject) -> Self {
        Self {
            domain: Domain::Code,
            subject,
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.subject.fmt(f)
    }
}

/// Where a decision stands in its life, as an architecture decision record
/// does. Only an accepted decision is enforced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Under discussion: written down, not yet enforced.
    Proposed,
    #[default]
    Accepted,
    /// Considered and turned down; kept so the question is not asked again.
    Rejected,
    /// Replaced by another decision (named in that decision's `supersedes`).
    Superseded,
    /// No longer wanted, and nothing replaces it.
    Deprecated,
}

impl Status {
    /// Whether decisions in this state are enforced.
    pub fn enforced(self) -> bool {
        self == Self::Accepted
    }

    /// Whether this is the default, left out of written files.
    pub fn is_default(&self) -> bool {
        *self == Self::Accepted
    }
}

/// Whether an example must produce no diagnostics (`valid`) or exactly those
/// listed in `expect` (`invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExampleKind {
    Valid,
    Invalid,
}

/// Where a fixture file's text comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Inline(String),
    /// Path relative to the directory of the decision's file.
    File(String),
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
    pub content: Content,
    loaded: Option<String>,
}

impl ExampleFile {
    /// A file whose text is `body`, written in the decision file itself.
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
    #[serde(default, skip_serializing_if = "is_false")]
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

/// The severity a finding's decision authored, else the severity the finding
/// has: what decides whether a verdict may hide it and a fix may be safe.
pub fn authored_severity(severity: Severity, decision: Option<&crate::Decision>) -> Severity {
    decision.and_then(|d| d.severity()).unwrap_or(severity)
}

/// Whether findings of a decision with this authored severity ask for a
/// verdict. An `error` is definitive: only an annotation in the code waives
/// it. A `warn` or `info` is a review task: a reviewer confirms or rejects it.
pub fn needs_verdict(authored: Severity) -> bool {
    authored != Severity::Error
}

/// The first eight bytes of the SHA-256 of `text`, in hex.
pub(crate) fn short_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
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

display!(Subject { Symbol => "symbol", File => "file", Module => "module", Project => "project", Test => "test" });
display!(Domain { Code => "code" });
display!(Status { Proposed => "proposed", Accepted => "accepted", Rejected => "rejected", Superseded => "superseded", Deprecated => "deprecated" });
display!(ExampleKind { Valid => "valid", Invalid => "invalid" });

fn is_false(value: &bool) -> bool {
    !value
}
