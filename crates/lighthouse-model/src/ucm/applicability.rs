use super::{File, Module, Project, Symbol};
use crate::TestScope;

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
    /// Whether no subject of `file` can be a subject of the decision, the
    /// project's symbols considered: it is generated code the decision skips,
    /// test code it leaves to other decisions, or a production file without
    /// test code in a decision that is only about tests.
    pub fn excludes(&self, project: &Project, file: &File) -> bool {
        (!self.generated && file.generated)
            || (self.tests == TestScope::Exclude && file.test)
            || (self.tests == TestScope::Only
                && !file.test
                && !project
                    .symbols_in(&file.path)
                    .any(|symbol| project.in_test(&symbol.id)))
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

    /// Whether a module is a subject, given the files that declare its
    /// symbols: a module of generated files is generated, and a module that
    /// exists to test another (`test_of`) or has only test files is test code.
    pub fn admits_module(&self, module: &Module, files: &[&File]) -> bool {
        let all = |flag: fn(&File) -> bool| !files.is_empty() && files.iter().all(|f| flag(f));
        if !self.generated && all(|f| f.generated) {
            return false;
        }
        let test = module.test_of.is_some() || all(|f| f.test);
        match self.tests {
            TestScope::Include => true,
            TestScope::Exclude => !test,
            TestScope::Only => test,
        }
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
