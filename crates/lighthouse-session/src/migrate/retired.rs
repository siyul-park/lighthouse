//! The kinds the project absorbed: a `Preset` is a `Project`, and a
//! `DecisionOverride` is an entry of the project's `rules`.

use lighthouse_spec::Catalog;
use serde_json::{Map, Value, json};

/// The kind of a preset document before it became a project.
pub const PRESET: &str = "Preset";
/// The kind of an override document before the project absorbed it.
pub const OVERRIDE: &str = "DecisionOverride";

/// What a `DecisionOverride` says that a project's `rules` can say: the
/// level and the options of one decision.
#[derive(Debug, Clone, PartialEq)]
pub struct Fold {
    pub id: String,
    pub level: Option<String>,
    pub options: Map<String, Value>,
}

impl Fold {
    /// The fold of an override document, or why it cannot be folded: wording,
    /// exceptions, per-language values and examples need a local decision.
    pub fn of(doc: &Value) -> Result<Self, String> {
        let spec = doc
            .get("spec")
            .and_then(Value::as_object)
            .ok_or("an override has a `spec`")?;
        if let Some(key) = spec
            .keys()
            .find(|k| !["extends", "severity", "options"].contains(&k.as_str()))
        {
            return Err(format!(
                "its `{key}` is not something a project's `rules` can say; write a local decision instead"
            ));
        }
        let id = spec
            .get("extends")
            .and_then(Value::as_str)
            .ok_or("an override `extends` a decision")?;
        Ok(Self {
            id: id.to_owned(),
            level: spec
                .get("severity")
                .and_then(Value::as_str)
                .map(str::to_owned),
            options: spec
                .get("options")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Sets the rule in the `Project` document: the level of the override,
    /// else the one the project has, else the decision's authored severity;
    /// the options merge over the project's, and what else the rule says
    /// (`generated`) stays. Options are named as `declared` spells them.
    pub fn apply(
        &self,
        project: &mut Value,
        declared: &super::modern::Declared,
        notes: &mut Vec<String>,
    ) -> Result<(), String> {
        let spec = project
            .get_mut("spec")
            .and_then(Value::as_object_mut)
            .ok_or("a project has a `spec`")?;
        let rules = spec
            .entry("rules")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or("`rules` is a table")?;
        let mut rule = match rules.remove(&self.id) {
            Some(Value::String(level)) => json!({ "level": level }),
            Some(table @ Value::Object(_)) => table,
            _ => json!({}),
        };
        let table = rule.as_object_mut().ok_or("a rule is a table")?;
        let level = match self.level.clone().or_else(|| {
            table
                .get("level")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }) {
            Some(level) => level,
            None => Catalog::bundled()
                .decision(&self.id)
                .and_then(|d| d.severity())
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    format!("`{}` is not a bundled decision with a severity", self.id)
                })?,
        };
        table.insert("level".to_owned(), Value::String(level));
        if !self.options.is_empty() {
            let options = table
                .entry("options")
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .ok_or("`options` is a table")?;
            options.extend(self.options.clone());
        }
        if table.len() == 1 {
            rule = table.remove("level").unwrap_or(Value::Null);
        }
        rules.insert(self.id.clone(), rule);
        super::modern::rename_rule_options(rules, declared, notes);
        Ok(())
    }

    /// Where this fold and `earlier`, of the same decision, say different
    /// things: a level or an option set to two values.
    pub fn conflicts_with(&self, earlier: &Self) -> Option<String> {
        if self.id != earlier.id {
            return None;
        }
        if let (Some(a), Some(b)) = (&self.level, &earlier.level)
            && a != b
        {
            return Some(format!("its level is `{a}` here and `{b}` there"));
        }
        self.options
            .iter()
            .find(|(key, value)| earlier.options.get(*key).is_some_and(|v| v != *value))
            .map(|(key, _)| format!("option `{key}` has another value there"))
    }
}

/// Whether the YAML document is one of the kinds that were retired.
pub fn is_retired(doc: &serde_norway::Value) -> bool {
    matches!(
        doc.get("kind").and_then(serde_norway::Value::as_str),
        Some(PRESET | OVERRIDE)
    )
}

/// A `Project` document from a `Preset`: the same `extends` and `rules`.
pub fn preset_to_project(mut doc: Value) -> Value {
    if let Some(kind) = doc.get_mut("kind") {
        *kind = json!("Project");
    }
    doc
}
