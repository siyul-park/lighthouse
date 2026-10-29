use std::{collections::BTreeSet, sync::Arc};

use super::{File, Module, Project, Symbol};
use crate::TestScope;

/// What a decision's subjects may be, besides being of its subject kind:
/// generated and test code, and the language of the code, are told apart by
/// the host, once, so no check has to guard against them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Applicability {
    /// Generated code is a subject too.
    pub generated: bool,
    pub tests: TestScope,
    /// The languages whose code is a subject; `None` is every language.
    pub languages: Option<Arc<BTreeSet<String>>>,
}

impl Applicability {
    /// The same applicability restricted to `languages`; none is every language.
    #[must_use]
    pub fn only(self, languages: impl IntoIterator<Item = String>) -> Self {
        let languages: BTreeSet<String> = languages.into_iter().collect();
        Self {
            languages: (!languages.is_empty()).then(|| Arc::new(languages)),
            ..self
        }
    }

    /// Whether code in `language` is a subject.
    pub fn admits_language(&self, language: &str) -> bool {
        self.languages
            .as_ref()
            .is_none_or(|languages| languages.contains(language))
    }

    /// Whether the file is in a language the decision covers: the language of
    /// the file itself, or the one analyzed in it, as a decision file holds CEL.
    fn admits_host(&self, file: &File) -> bool {
        self.admits_language(&file.lang) || self.admits_language(file.analysis_language())
    }

    /// Whether no subject of `file` can be a subject of the decision, the
    /// project's symbols considered: it is in a language the decision does not
    /// cover, it is generated code the decision skips, test code it leaves to
    /// other decisions, or a production file without test code in a decision
    /// that is only about tests.
    pub fn excludes(&self, project: &Project, file: &File) -> bool {
        !self.admits_host(file)
            || (!self.generated && file.generated)
            || (self.tests == TestScope::Exclude && file.test)
            || (self.tests == TestScope::Only
                && !file.test
                && !project
                    .symbols_in(&file.path)
                    .any(|symbol| project.in_test(&symbol.id)))
    }

    /// Whether a file is a subject of a decision about files.
    pub fn admits_file(&self, file: &File) -> bool {
        if !self.admits_host(file) || (!self.generated && file.generated) {
            return false;
        }
        match self.tests {
            TestScope::Exclude => !file.test,
            TestScope::Include => true,
            TestScope::Only => file.test,
        }
    }

    /// Whether a comment of `file` is a subject: only the file's language and
    /// generation count, a comment is not test code of its own. A comment of a
    /// file the model does not know counts as generated.
    pub fn admits_comment(&self, file: Option<&File>) -> bool {
        match file {
            Some(file) => self.admits_host(file) && (self.generated || !file.generated),
            None => self.generated,
        }
    }

    /// Whether a module is a subject, given the files that declare its
    /// symbols: a module of generated files is generated, and a module that
    /// exists to test another (`test_of`) or has only test files is test code.
    /// It is in a language when one of its files is.
    pub fn admits_module(&self, module: &Module, files: &[&File]) -> bool {
        if !files.is_empty()
            && !files
                .iter()
                .any(|f| self.admits_language(f.analysis_language()))
        {
            return false;
        }
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

    /// Whether a symbol is a subject: it is in a language the decision covers
    /// and not in generated code the decision skips, and it is test code as
    /// `tests` demands. A symbol is test code when its file is a test file or
    /// its module exists to test another.
    pub fn admits_symbol(&self, project: &Project, symbol: &Symbol) -> bool {
        let file = project.file(&symbol.file);
        if file.is_some_and(|f| !self.admits_language(f.analysis_language()))
            || (!self.generated && file.is_some_and(|f| f.generated))
        {
            return false;
        }
        match self.tests {
            TestScope::Include => true,
            TestScope::Exclude => !project.in_test(&symbol.id),
            TestScope::Only => project.in_test(&symbol.id),
        }
    }
}
