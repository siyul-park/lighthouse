use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{File, Project, Symbol};

/// What a rule or an analyzer looks at in one run; a decision's subject maps
/// onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunScope {
    /// Once per file; the focused file is set.
    File,
    /// Once per run over the merged project; there is no focused file.
    Project,
}

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

/// What a decision's subjects may be, besides being of its subject kind:
/// generated and test code are told apart by the host, once, so no check has
/// to guard against them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Applicability {
    /// Generated code is a subject too.
    pub generated: bool,
    pub tests: TestScope,
}

impl Applicability {
    /// Whether every subject of `file` is out: it is generated code the
    /// decision skips, or test code it leaves to other decisions.
    pub fn excludes_file(&self, file: &File) -> bool {
        (!self.generated && file.generated) || (self.tests == TestScope::Exclude && file.test)
    }

    /// Whether a file is a subject of a decision about files.
    pub fn admits_file(&self, file: &File) -> bool {
        if !self.generated && file.generated {
            return false;
        }
        match self.tests {
            TestScope::Exclude => !file.test,
            TestScope::Include => true,
            TestScope::Only => file.test,
        }
    }

    /// Whether a comment of `file` is a subject: only the file's generation
    /// counts, a comment is not test code of its own. A comment of a file the
    /// model does not know counts as generated.
    pub fn admits_comment(&self, file: Option<&File>) -> bool {
        self.generated || file.is_some_and(|f| !f.generated)
    }

    /// Whether a symbol is a subject: it is not in generated code the
    /// decision skips, and it is test code as `tests` demands. A symbol is
    /// test code when its file is a test file or its module exists to test
    /// another.
    pub fn admits_symbol(&self, project: &Project, symbol: &Symbol) -> bool {
        if !self.generated && project.file(&symbol.file).is_some_and(|f| f.generated) {
            return false;
        }
        match self.tests {
            TestScope::Include => true,
            TestScope::Exclude => !project.in_test(&symbol.id),
            TestScope::Only => project.in_test(&symbol.id),
        }
    }
}
