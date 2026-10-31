use std::fmt;

use lighthouse_model::{ExampleScope, RunScope, TestScope, ucm::Applicability};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where a decision applies: what it is about, and which code its subjects
/// may be in. Generated, test and example code are told apart by the host (the
/// provider's marker, `.gitattributes`, the project's `generated` globs), and
/// the engine leaves the subjects out that the scope excludes before any
/// check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub subject: Subject,
    /// Generated code is a subject too. Default: it is not.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub generated: bool,
    /// Which test code is a subject: `exclude` (the default), `include` or
    /// `only`.
    #[serde(default, skip_serializing_if = "TestScope::is_default")]
    pub tests: TestScope,
    /// Which example code is a subject, the code documentation shows:
    /// `include` (the default), `exclude` or `only`.
    #[serde(default, skip_serializing_if = "ExampleScope::is_default")]
    pub examples: ExampleScope,
}

impl Scope {
    /// A scope over `subject` that leaves out generated and test code.
    pub fn new(subject: Subject) -> Self {
        Self {
            subject,
            generated: false,
            tests: TestScope::Exclude,
            examples: ExampleScope::Include,
        }
    }

    /// What code the subjects may be in.
    pub fn applicability(self) -> Applicability {
        Applicability {
            generated: self.generated,
            tests: self.tests,
            examples: self.examples,
            languages: None,
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.subject.fmt(f)
    }
}

/// What a decision talks about. Mapping to the run scope of a rule is
/// `Subject::run_scope`.
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
    /// A decision document of the project.
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
}

impl fmt::Display for Subject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Symbol => "symbol",
            Self::File => "file",
            Self::Module => "module",
            Self::Edge => "edge",
            Self::Project => "project",
            Self::Test => "test",
            Self::Decision => "decision",
        })
    }
}
