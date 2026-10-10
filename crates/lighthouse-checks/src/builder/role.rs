//! The `role` fact of a function and the constructor facts beside it.

use lighthouse_model::{FunctionSummary, Symbol, SymbolKind};
use serde_json::{Map, Value, json};

use super::Builder;
use crate::layout;

/// What a function is, as the providers and the project's conventions say.
#[derive(Clone, Copy, Default)]
pub(super) struct Flags {
    pub test: bool,
    pub implementation: bool,
    pub constructor: bool,
    pub entrypoint: bool,
    pub method: bool,
}

impl Builder<'_> {
    /// `constructor_named`, `role` and `built`, for the expressions that
    /// mention them (`limit` and `counted` read `role` and `built`).
    pub(super) fn role_facts(
        &self,
        map: &mut Map<String, Value>,
        symbol: &Symbol,
        summary: Option<&FunctionSummary>,
    ) {
        let needs = self.needs;
        if needs.fact("constructor_named") {
            map.insert(
                "constructor_named".to_owned(),
                json!(self.constructor_named(symbol)),
            );
        }
        if needs.fact("effective") {
            map.insert("effective".to_owned(), self.effective(symbol, summary));
        }
        if needs.fact("role")
            || needs.fact("built")
            || needs.calls("limit")
            || needs.calls("counted")
        {
            map.insert("role".to_owned(), json!(self.role(symbol, summary)));
            map.insert("built".to_owned(), json!(self.built(symbol)));
        }
    }

    /// Whether the name is one of the language's constructor prefixes, or one
    /// followed by a new word.
    pub(super) fn constructor_named(&self, symbol: &Symbol) -> bool {
        let language = self.language_of(symbol);
        self.ws
            .constructor_prefixes(language)
            .iter()
            .any(|p| layout::has_word_prefix(&symbol.name, p))
    }

    /// What a constructor builds: the name without its prefix (`NewServer`
    /// builds `Server`), else its owner (`Server::new`), else its own name.
    pub(super) fn built(&self, symbol: &Symbol) -> String {
        let named = self
            .ws
            .constructor_prefixes(self.language_of(symbol))
            .iter()
            .filter(|p| layout::has_word_prefix(&symbol.name, p))
            .filter_map(|p| symbol.name.strip_prefix(p.as_str()))
            .find(|rest| !rest.is_empty());
        let owner = symbol
            .owner
            .as_ref()
            .and_then(|o| self.project.symbol(o))
            .map(|o| o.name.as_str());
        named.or(owner).unwrap_or(&symbol.name).to_owned()
    }

    pub(super) fn language_of(&self, symbol: &Symbol) -> &str {
        self.project
            .file(&symbol.file)
            .map_or(self.language.as_str(), |f| f.lang.as_str())
    }

    /// What a function is for: the first role its flags give (see
    /// [`precedence`]). Empty for anything that is not a function.
    pub(super) fn role(&self, symbol: &Symbol, summary: Option<&FunctionSummary>) -> &'static str {
        if !matches!(
            symbol.kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Test
        ) {
            return "";
        }
        precedence(Flags {
            test: symbol.kind == SymbolKind::Test || self.project.in_test(&symbol.id),
            implementation: summary.is_some_and(|s| s.implementation),
            constructor: summary.is_some_and(|s| s.constructs) || self.constructor_named(symbol),
            entrypoint: self.entrypoint(symbol),
            method: symbol.kind == SymbolKind::Method,
        })
    }

    /// Go `main` and `init` in `package main` (`init` in any package), Rust
    /// `main` at the root of a binary target.
    pub(super) fn entrypoint(&self, symbol: &Symbol) -> bool {
        if symbol.owner.is_some() {
            return false;
        }
        let module = symbol.id.module();
        match (self.language_of(symbol), symbol.name.as_str()) {
            ("go", "init") => true,
            ("go", "main") => self
                .project
                .module(module)
                .is_some_and(|m| m.name.as_deref() == Some("main")),
            ("rust", "main") => module.contains("[bin:") && !module.contains('/'),
            _ => false,
        }
    }
}

/// The role of a function: `test`, `implementation` (an interface or trait
/// fixes its signature), `constructor`, `entrypoint`, then `method` or
/// `function`; the first flag set wins.
pub(super) fn precedence(flags: Flags) -> &'static str {
    let ordered = [
        (flags.test, "test"),
        (flags.implementation, "implementation"),
        (flags.constructor, "constructor"),
        (flags.entrypoint, "entrypoint"),
    ];
    ordered
        .into_iter()
        .find_map(|(set, role)| set.then_some(role))
        .unwrap_or(if flags.method { "method" } else { "function" })
}
