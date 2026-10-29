use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a rule or an analyzer looks at in one run; a decision's subject maps
/// onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunScope {
    /// Once per file; the focused file is set.
    File,
    /// Once per run over the merged project; there is no focused file.
    Project,
}

/// How much of the project a rule's findings about one subject depend on,
/// which is what a stored result may be reused across. The order is the order
/// of reach: a rule's reach is the greatest of what it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reach {
    /// The subject's own symbol, file text and summary.
    Local,
    /// Symbols one edge, one owner or one member away from the subject.
    Neighbors,
    /// Anything in the project.
    Global,
}

vocabulary!(Reach, "reach" {
    Local => "local",
    Neighbors => "neighbors",
    Global => "global",
});

/// Which test code a decision applies to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TestScope {
    /// Production code only.
    #[default]
    Exclude,
    /// Production and test code.
    Include,
    /// Test code only.
    Only,
}

impl TestScope {
    /// Whether this is the default, left out of written files.
    pub fn is_default(&self) -> bool {
        *self == Self::Exclude
    }
}

/// Which example code a decision applies to: the code that documentation
/// shows, which is analyzed like any other and is not part of the project.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ExampleScope {
    /// Project code only.
    Exclude,
    /// Project code and examples.
    #[default]
    Include,
    /// Examples only.
    Only,
}

impl ExampleScope {
    /// Whether this is the default, left out of written files.
    pub fn is_default(&self) -> bool {
        *self == Self::Include
    }

    /// Whether code that is an example (or is not) is a subject.
    pub fn admits(self, example: bool) -> bool {
        match self {
            Self::Exclude => !example,
            Self::Include => true,
            Self::Only => example,
        }
    }
}
