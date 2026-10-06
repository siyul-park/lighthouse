use std::{collections::BTreeMap, path::Path};

use lighthouse_model::{Diagnostic, Project, SymbolId};
use lighthouse_plugin::Facts;
use serde_json::{Map, Value, json};

use crate::engine::Input;

/// What the analysis knew about the subject of each finding, frozen into the
/// finding's record so a later review can snapshot it without re-analysis.
pub(crate) struct Subjects<'a> {
    project: &'a Project,
    /// Measured facts (`[{symbol, value}]` lists, or a single value for a
    /// file) by the file key they were computed for; the empty key holds
    /// project-scope facts.
    measured: BTreeMap<&'a str, Vec<(&'a str, &'a Value)>>,
    lines: BTreeMap<&'a Path, usize>,
}

impl<'a> Subjects<'a> {
    pub(crate) fn new(project: &'a Project, facts: &'a Facts, inputs: &'a [Input]) -> Self {
        let mut measured: BTreeMap<&str, Vec<(&str, &Value)>> = BTreeMap::new();
        for ((analyzer, key), value) in facts {
            measured
                .entry(key.as_str())
                .or_default()
                .push((analyzer.as_str(), value));
        }
        let lines = inputs
            .iter()
            .map(|i| (i.file.path.as_path(), i.text.lines().count()))
            .collect();
        Self {
            project,
            measured,
            lines,
        }
    }

    /// The language of the file. For a finding about a symbol, its shape
    /// (kind, visibility, owner, callers split by module, callees), function
    /// summary and the measures analyzers took of it; for any other finding,
    /// the file's size and the measures taken of the file.
    pub(crate) fn of(&self, diagnostic: &Diagnostic) -> Value {
        let mut facts = Map::new();
        if let Some(file) = self.project.file(&diagnostic.file) {
            facts.insert("language".to_owned(), json!(file.lang));
        }
        match diagnostic.symbol.as_deref().and_then(SymbolId::parse) {
            Some(id) => facts.extend(self.symbol_facts(&id, &diagnostic.file)),
            None => {
                facts.insert("file".to_owned(), self.file_facts(&diagnostic.file));
            }
        }
        Value::Object(facts)
    }

    fn file_facts(&self, file: &Path) -> Value {
        let symbols: Vec<_> = self.project.symbols_in(file).collect();
        let functions = symbols
            .iter()
            .filter(|s| self.project.function(&s.id).is_some())
            .count();
        let record = self.project.file(file);
        let key = file.to_string_lossy();
        let mut measures = Map::new();
        for (analyzer, value) in self.measured.get(key.as_ref()).into_iter().flatten() {
            if !is_symbol_list(value) {
                measures.insert((*analyzer).to_owned(), (*value).clone());
            }
        }
        json!({
            "path": key.replace('\\', "/"),
            "lines": self.lines.get(file),
            "symbols": symbols.len(),
            "functions": functions,
            "test": record.map(|f| f.test),
            "generated": record.map(|f| f.generated),
            "measures": measures,
        })
    }

    fn symbol_facts(&self, id: &SymbolId, file: &Path) -> Map<String, Value> {
        let mut facts = Map::new();
        if let Some(symbol) = self.project.symbol(id) {
            facts.insert("kind".to_owned(), json!(symbol.kind));
            facts.insert("visibility".to_owned(), json!(symbol.visibility));
            facts.insert("owner".to_owned(), json!(symbol.owner));
        }
        let callers = self.project.callers(id);
        let same_module = callers.iter().filter(|c| c.module() == id.module()).count();
        facts.insert("callers".to_owned(), json!(callers.len()));
        facts.insert("callers_same_module".to_owned(), json!(same_module));
        facts.insert(
            "callers_other_module".to_owned(),
            json!(callers.len() - same_module),
        );
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

/// Whether a fact is a per-symbol measurement list rather than a value about
/// the file itself.
fn is_symbol_list(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(|i| i.get("symbol").is_some()))
}
