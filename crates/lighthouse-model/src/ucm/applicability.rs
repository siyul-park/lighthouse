use std::{collections::BTreeSet, sync::Arc};

use super::{File, Module, Project, Symbol};
use crate::{ExampleScope, TestScope};

/// What a decision's subjects may be, besides being of its subject kind:
/// generated and test code, example code, and the language of the code, are
/// told apart by the host, once, so no check has to guard against them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Applicability {
    /// Generated code is a subject too.
    pub generated: bool,
    pub tests: TestScope,
    pub examples: ExampleScope,
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

    /// Whether no subject of `file` can be a subject of the decision, the
    /// project's symbols considered: it is in a language the decision does not
    /// cover (nor do its regions), it is generated code the decision skips,
    /// test code it leaves to other decisions, a production file without test
    /// code in a decision that is only about tests, or without example code in
    /// a decision that is only about examples.
    pub fn excludes(&self, project: &Project, file: &File) -> bool {
        !(self.admits_host(file) || self.admits_regions(file))
            || (!self.generated && file.generated)
            || (self.tests == TestScope::Exclude && file.test)
            || (self.tests == TestScope::Only
                && !file.test
                && !project
                    .symbols_in(&file.path)
                    .any(|symbol| project.in_test(&symbol.id)))
            || (self.examples == ExampleScope::Only && !file.regions().any(|(_, example)| example))
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
    /// file the model does not know counts as generated. A comment of a host
    /// file is a subject too when a region of a covered language may be.
    pub fn admits_comment(&self, file: Option<&File>) -> bool {
        match file {
            Some(file) => {
                (self.admits_host(file) || self.admits_regions(file))
                    && (self.generated || !file.generated)
            }
            None => self.generated,
        }
    }

    /// Whether a module is a subject, given the files that declare its
    /// symbols: a module of generated files is generated, and a module that
    /// exists to test another (`test_of`) or has only test files is test code.
    /// It is in the language its host stated for it, else when one of its files
    /// is, and it is example code as the host stated.
    pub fn admits_module(&self, module: &Module, files: &[&File]) -> bool {
        let language = match &module.language {
            Some(language) => self.admits_language(language),
            None => {
                files.is_empty()
                    || files
                        .iter()
                        .any(|f| self.admits_language(f.analysis_language()))
            }
        };
        if !language || !self.examples.admits(module.example) {
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
    /// and not in generated code the decision skips, and it is test code and
    /// example code as `tests` and `examples` demand. A symbol is test code
    /// when its file is a test file or its module exists to test another, and
    /// example code when its module is.
    pub fn admits_symbol(&self, project: &Project, symbol: &Symbol) -> bool {
        let file = project.file(&symbol.file);
        let language = project.language_of(symbol);
        if language.is_some_and(|language| !self.admits_language(language))
            || (!self.generated && file.is_some_and(|f| f.generated))
            || !self.examples.admits(project.in_example(&symbol.id))
        {
            return false;
        }
        match self.tests {
            TestScope::Include => true,
            TestScope::Exclude => !project.in_test(&symbol.id),
            TestScope::Only => project.in_test(&symbol.id),
        }
    }

    /// Whether the file is in a language the decision covers: the language of
    /// the file itself, or the one analyzed in it, as a decision file holds CEL.
    fn admits_host(&self, file: &File) -> bool {
        self.admits_language(&file.lang) || self.admits_language(file.analysis_language())
    }

    /// Whether some region the host routed to another provider is a subject.
    fn admits_regions(&self, file: &File) -> bool {
        file.regions()
            .any(|(language, example)| self.admits_region(language, example))
    }

    /// Whether a region of the file, in a language and example code or not,
    /// is a subject.
    fn admits_region(&self, language: &str, example: bool) -> bool {
        self.admits_language(language) && self.examples.admits(example)
    }
}
