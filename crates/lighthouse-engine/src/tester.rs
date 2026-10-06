use std::{fmt::Write as _, fs};

use lighthouse_config::Config;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Example, Kind, Pattern};
use serde_json::Value;

use crate::Engine;

/// Runs the examples of catalog patterns through the whole engine, the way
/// `lighthouse check` would see them. The registry is built per example
/// because an engine owns its registry.
pub struct RuleTester<'a> {
    registry: fn() -> Registry,
    catalog: &'a Catalog,
}

impl<'a> RuleTester<'a> {
    pub fn new(registry: fn() -> Registry, catalog: &'a Catalog) -> Self {
        Self { registry, catalog }
    }

    /// Failures of every implemented pattern; empty when all examples hold.
    pub fn check_all(&self) -> Vec<String> {
        self.catalog
            .patterns()
            .filter(|p| p.implementation.is_some())
            .flat_map(|p| self.check(p))
            .collect()
    }

    /// Failures of one pattern's examples, each prefixed `<id> <example>:`.
    pub fn check(&self, pattern: &Pattern) -> Vec<String> {
        pattern
            .examples
            .iter()
            .filter_map(|example| {
                let failure = self.run(pattern, example).err()?;
                Some(format!("{} {}: {failure}", pattern.id, example.name))
            })
            .collect()
    }

    fn run(&self, pattern: &Pattern, example: &Example) -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        for file in &example.files {
            let path = dir.path().join(&file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&path, file.text()).map_err(|e| e.to_string())?;
        }
        let registry = (self.registry)();
        let config = config(&registry, pattern, example)?;
        let outcome = Engine::new(registry, config, dir.path())
            .and_then(|engine| engine.check(&[], std::slice::from_ref(&pattern.id)))
            .map_err(|e| e.to_string())?;
        let found: Vec<(u32, &str)> = outcome
            .diagnostics
            .iter()
            .map(|d| (d.span.start.line, d.message.as_str()))
            .collect();
        match example.kind {
            Kind::Valid if found.is_empty() => Ok(()),
            Kind::Valid => Err(format!("expected no diagnostics, got {}", describe(&found))),
            Kind::Invalid => compare(example, &found),
        }
    }
}

fn config(registry: &Registry, pattern: &Pattern, example: &Example) -> Result<Config, String> {
    let level = pattern
        .severity()
        .ok_or_else(|| "a pattern without severity has no rule".to_owned())?;
    let mut rule = toml::Table::new();
    rule.insert("level".to_owned(), level.to_string().into());
    for (key, value) in &example.options {
        rule.insert(key.clone(), toml_value(value)?);
    }
    let plugins: Vec<toml::Value> = registry
        .plugins()
        .map(|id| toml::Value::String(id.to_owned()))
        .collect();
    let mut rules = toml::Table::new();
    rules.insert(pattern.id.clone(), rule.into());
    let mut root = toml::Table::new();
    root.insert("plugins".to_owned(), plugins.into());
    root.insert("rules".to_owned(), rules.into());
    Config::parse(&root.to_string()).map_err(|e| e.to_string())
}

fn toml_value(value: &Value) -> Result<toml::Value, String> {
    toml::Value::try_from(value).map_err(|e| e.to_string())
}

fn compare(example: &Example, found: &[(u32, &str)]) -> Result<(), String> {
    let want: Vec<u32> = example.expect.iter().map(|e| e.line).collect();
    let got: Vec<u32> = found.iter().map(|(line, _)| *line).collect();
    if want != got {
        return Err(format!(
            "expected diagnostics at lines {want:?}, got {}",
            describe(found)
        ));
    }
    for (expect, (line, message)) in example.expect.iter().zip(found) {
        if let Some(text) = &expect.message
            && !message.contains(text.as_str())
        {
            return Err(format!("line {line}: `{message}` lacks `{text}`"));
        }
    }
    Ok(())
}

fn describe(found: &[(u32, &str)]) -> String {
    if found.is_empty() {
        return "none".to_owned();
    }
    let mut out = String::new();
    for (line, message) in found {
        let _ = write!(out, "[{line}: {message}] ");
    }
    out.trim_end().to_owned()
}
