use std::{collections::BTreeMap, path::Path};

use lighthouse_model::{Diagnostic, Project, SymbolId};
use lighthouse_plugin::Facts;
use serde_json::{Map, Value, json};

/// What the analysis knew about the subject of each finding, frozen into the
/// finding's record so a later review can snapshot it without re-analysis.
pub(crate) struct Subjects<'a> {
    project: &'a Project,
    /// Measured facts (`[{symbol, value}]` lists) by the file key they were
    /// computed for; the empty key holds project-scope facts.
    measured: BTreeMap<&'a str, Vec<(&'a str, &'a Value)>>,
}

impl<'a> Subjects<'a> {
    pub(crate) fn new(project: &'a Project, facts: &'a Facts) -> Self {
        let mut measured: BTreeMap<&str, Vec<(&str, &Value)>> = BTreeMap::new();
        for ((analyzer, key), value) in facts {
            measured
                .entry(key.as_str())
                .or_default()
                .push((analyzer.as_str(), value));
        }
        Self { project, measured }
    }

    /// The language of the file, and for a finding about a symbol its shape
    /// (kind, visibility, owner, callers and callees), function summary and
    /// the measures analyzers took of it.
    pub(crate) fn of(&self, diagnostic: &Diagnostic) -> Value {
        let mut facts = Map::new();
        if let Some(file) = self.project.file(&diagnostic.file) {
            facts.insert("language".to_owned(), json!(file.lang));
        }
        let symbol = diagnostic.symbol.as_deref().and_then(SymbolId::parse);
        if let Some(id) = symbol {
            facts.extend(self.symbol_facts(&id, &diagnostic.file));
        }
        Value::Object(facts)
    }

    fn symbol_facts(&self, id: &SymbolId, file: &Path) -> Map<String, Value> {
        let mut facts = Map::new();
        if let Some(symbol) = self.project.symbol(id) {
            facts.insert("kind".to_owned(), json!(symbol.kind));
            facts.insert("visibility".to_owned(), json!(symbol.visibility));
            facts.insert("owner".to_owned(), json!(symbol.owner));
        }
        facts.insert("callers".to_owned(), json!(self.project.callers(id).len()));
        facts.insert("callees".to_owned(), json!(self.project.callees(id).len()));
        if let Some(summary) = self.project.function(id) {
            facts.insert(
                "function".to_owned(),
                json!({
                    "statements": summary.statements,
                    "top_level": summary.top_level,
                    "params": summary.params,
                    "returns": summary.returns,
                    "max_nesting": summary.max_nesting,
                    "tokens": summary.tokens,
                }),
            );
        }
        facts.insert("measures".to_owned(), self.measures(id, file));
        facts
    }

    fn measures(&self, id: &SymbolId, file: &Path) -> Value {
        let keys = [file.to_string_lossy().into_owned(), String::new()];
        let mut measures = Map::new();
        for key in &keys {
            for (analyzer, value) in self.measured.get(key.as_str()).into_iter().flatten() {
                let hit =
                    value.as_array().into_iter().flatten().find(|item| {
                        item.get("symbol").and_then(Value::as_str) == Some(id.as_str())
                    });
                if let Some(value) = hit.and_then(|item| item.get("value")) {
                    measures.insert((*analyzer).to_owned(), value.clone());
                }
            }
        }
        Value::Object(measures)
    }
}
