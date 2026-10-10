//! The `homonyms` fact: the public types of other modules that share a name.

use std::{collections::BTreeMap, sync::OnceLock};

use lighthouse_model::{Project, Symbol, SymbolId, SymbolKind, Visibility};
use serde_json::{Value, json};

use super::Builder;

/// The public production types of the project by name, built by the first
/// check of a run that asks and shared by the others.
#[derive(Default)]
pub(crate) struct Names(OnceLock<BTreeMap<(String, String), Vec<SymbolId>>>);

impl Builder<'_> {
    /// The public types declared under the same name in other modules, each
    /// with its id and module; empty for anything but a public type.
    pub(super) fn homonyms(&self, symbol: &Symbol) -> Value {
        let project = self.project;
        if !is_public_type(project, symbol) {
            return json!([]);
        }
        let names = self.names.0.get_or_init(|| names(project));
        let same = names
            .get(&key(project, symbol))
            .map_or(&[][..], Vec::as_slice);
        Value::Array(
            same.iter()
                .filter(|id| id.module() != symbol.id.module())
                .map(|id| json!({ "id": id.as_str(), "module": id.module() }))
                .collect(),
        )
    }
}

fn names(project: &Project) -> BTreeMap<(String, String), Vec<SymbolId>> {
    let mut names: BTreeMap<(String, String), Vec<SymbolId>> = BTreeMap::new();
    for symbol in project
        .symbols
        .iter()
        .filter(|s| is_public_type(project, s))
    {
        names
            .entry(key(project, symbol))
            .or_default()
            .push(symbol.id.clone());
    }
    names
}

/// The name and language of a type: a type that a plugin mirrors in another
/// language is not a homonym of its mirror.
fn key(project: &Project, symbol: &Symbol) -> (String, String) {
    let lang = project.file(&symbol.file).map_or("", |f| f.lang.as_str());
    (lang.to_owned(), symbol.name.clone())
}

/// Whether the symbol is a public type of production code, not generated.
fn is_public_type(project: &Project, symbol: &Symbol) -> bool {
    symbol.kind == SymbolKind::Type
        && symbol.visibility == Visibility::Public
        && !project.in_test(&symbol.id)
        && project.file(&symbol.file).is_some_and(|f| !f.generated)
}
