use std::collections::BTreeMap;

use tree_sitter::{Language, Parser, Tree};

use crate::Error;

/// Grammars keyed by language id.
#[derive(Default)]
pub struct Syntax {
    grammars: BTreeMap<String, Language>,
}

impl Syntax {
    pub fn register(&mut self, id: &str, language: impl Into<Language>) {
        self.grammars.insert(id.to_owned(), language.into());
    }

    pub fn language(&self, id: &str) -> Result<&Language, Error> {
        self.grammars
            .get(id)
            .ok_or_else(|| Error::UnknownLanguage(id.to_owned()))
    }

    pub fn parse(&self, id: &str, text: &str) -> Result<Tree, Error> {
        let mut parser = Parser::new();
        parser
            .set_language(self.language(id)?)
            .map_err(|_| Error::Incompatible(id.to_owned()))?;
        parser.parse(text, None).ok_or(Error::Interrupted)
    }
}
