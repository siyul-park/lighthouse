use std::fmt;

use lighthouse_model::{Applicability, RunScope, Severity, TestScope};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What kind of thing a decision is about: source code, or Lighthouse's own
/// documents (the decisions of a catalog).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Domain {
    #[default]
    Code,
    /// The documents of a project that configure Lighthouse.
    Spec,
}

/// What a decision talks about inside its domain. Mapping to the run scope
/// of a rule is `Subject::run_scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Subject {
    Symbol,
    File,
    Module,
    /// A relation between two symbols or modules, such as an import.
    Edge,
    Project,
    Test,
    /// A decision document, in the `spec` domain.
    Decision,
}

impl Subject {
    /// Symbol, file and test decisions are checked once per file; module,
    /// project and decision decisions once over the merged project.
    pub fn run_scope(self) -> RunScope {
        match self {
            Self::Symbol | Self::File | Self::Test => RunScope::File,
            Self::Module | Self::Edge | Self::Project | Self::Decision => RunScope::Project,
        }
    }

    /// The domain this subject belongs to.
    pub fn domain(self) -> Domain {
        match self {
            Self::Decision => Domain::Spec,
            _ => Domain::Code,
        }
    }
}

/// Where a decision applies: its domain, what it is about, and which code its
/// subjects may be in. Generated and test code are told apart by the host
/// (the provider's marker, `.gitattributes`, the project's `generated`
/// globs), and the engine leaves the subjects out that the scope excludes
/// before any check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(default)]
    pub domain: Domain,
    pub subject: Subject,
    /// Generated code is a subject too. Default: it is not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub generated: bool,
    /// Which test code is a subject: `exclude` (the default), `include` or
    /// `only`.
    #[serde(default, skip_serializing_if = "TestScope::is_default")]
    pub tests: TestScope,
}

impl Scope {
    /// A scope in the code domain.
    pub fn code(subject: Subject) -> Self {
        Self {
            domain: Domain::Code,
            subject,
            generated: false,
            tests: TestScope::Exclude,
        }
    }

    /// What code the subjects may be in.
    pub fn applicability(self) -> Applicability {
        Applicability {
            generated: self.generated,
            tests: self.tests,
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
pub enum DecisionStatus {
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

impl DecisionStatus {
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
/// has: what decides whether a judgment may hide it and a fix may be safe.
pub fn authored_severity(severity: Severity, decision: Option<&crate::Decision>) -> Severity {
    decision.and_then(|d| d.severity()).unwrap_or(severity)
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

display!(Subject { Symbol => "symbol", File => "file", Module => "module", Edge => "edge", Project => "project", Test => "test", Decision => "decision" });
display!(Domain { Code => "code", Spec => "spec" });
display!(DecisionStatus { Proposed => "proposed", Accepted => "accepted", Rejected => "rejected", Superseded => "superseded", Deprecated => "deprecated" });
display!(ExampleKind { Valid => "valid", Invalid => "invalid" });

fn is_false(value: &bool) -> bool {
    !value
}
