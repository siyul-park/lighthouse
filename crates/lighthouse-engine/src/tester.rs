use std::{fmt::Write as _, fs, path::Path};

use lighthouse_config::Config;
use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, Decision, Example, ExampleKind};
use serde_json::Value;

use crate::{Engine, FixPlan, FixRun, unified_diff};

/// Language of examples that need no language provider.
const NEUTRAL: &str = "text";

/// Runs the examples of catalog decisions through the whole engine, the way
/// `lighthouse check` would see them. The registry is built per example
/// because an engine owns its registry.
pub struct RuleTester<'a> {
    registry: Box<dyn Fn() -> Registry + 'a>,
    catalog: &'a Catalog,
    language: Option<String>,
    trusted: bool,
}

impl<'a> RuleTester<'a> {
    /// A tester over `catalog` that builds a fresh registry from `registry` for
    /// every example.
    pub fn new(registry: impl Fn() -> Registry + 'a, catalog: &'a Catalog) -> Self {
        Self {
            registry: Box::new(registry),
            catalog,
            language: None,
            trusted: false,
        }
    }

    /// Lets fix examples run commands, as a trusted project does.
    pub fn trusted(mut self, trusted: bool) -> Self {
        self.trusted = trusted;
        self
    }

    /// Runs only the examples written for `language`; a registry usually
    /// holds the provider of one language.
    pub fn language(mut self, language: &str) -> Self {
        self.language = Some(language.to_owned());
        self
    }

    /// Failures of every checked decision; empty when all examples hold.
    pub fn check_all(&self) -> Vec<String> {
        self.catalog
            .decisions()
            .filter(|d| d.check.is_some())
            .flat_map(|p| self.check(p))
            .collect()
    }

    /// Failures of one decision's examples, each prefixed `<id> <example>:`.
    /// With a language filter, a decision that has no example for that language
    /// (and no language-neutral `text` example) is itself a failure: a checked
    /// rule must show what it does in every language under test.
    pub fn check(&self, decision: &Decision) -> Vec<String> {
        let selected: Vec<&Example> = decision
            .examples
            .iter()
            .filter(|example| self.selects(example))
            .collect();
        if selected.is_empty() && self.language.is_some() {
            return vec![format!(
                "{}: no example for language `{}`",
                decision.id(),
                self.language.as_deref().unwrap_or_default()
            )];
        }
        selected
            .into_iter()
            .filter_map(|example| {
                let failure = self.run(decision, example).err()?;
                Some(format!("{} {}: {failure}", decision.id(), example.name))
            })
            .collect()
    }

    fn selects(&self, example: &Example) -> bool {
        self.language
            .as_ref()
            .is_none_or(|language| *language == example.language || example.language == NEUTRAL)
    }

    fn run(&self, decision: &Decision, example: &Example) -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        for file in &example.files {
            let path = dir.path().join(&file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&path, file.text()).map_err(|e| e.to_string())?;
        }
        let registry = (self.registry)();
        let config = config(&registry, decision, example)?;
        let engine = Engine::new(registry, config, dir.path()).map_err(|e| e.to_string())?;
        let outcome = engine
            .check(&[], &[decision.id().to_owned()])
            .map_err(|e| e.to_string())?;
        let found: Vec<(u32, &str)> = outcome
            .diagnostics
            .iter()
            .map(|d| (d.span.start.line, d.message.as_str()))
            .collect();
        match example.kind {
            ExampleKind::Valid if found.is_empty() => Ok(()),
            ExampleKind::Valid => Err(format!("expected no diagnostics, got {}", describe(&found))),
            ExampleKind::Invalid => {
                compare(example, &found)?;
                if decision.fix.is_some() && !example.fixed.is_empty() {
                    self.check_fix(&engine, decision, example, dir.path())?;
                }
                Ok(())
            }
        }
    }

    /// Applies the decision's fix to the written example and holds it to the
    /// example's `fixed` files: the text is exactly that, the rule no longer
    /// fires, and a second application changes nothing.
    fn check_fix(
        &self,
        engine: &Engine,
        decision: &Decision,
        example: &Example,
        dir: &Path,
    ) -> Result<(), String> {
        let plan = FixPlan::from_catalog(self.catalog);
        let run = FixRun {
            rules: vec![decision.id().to_owned()],
            unsafe_fixes: true,
            trusted: self.trusted,
            ..FixRun::default()
        };
        let report = engine.fix(&plan, &run).map_err(|e| format!("fix: {e}"))?;
        if report.applied.is_empty() {
            let why: Vec<String> = report.declined.iter().map(|d| d.reason.clone()).collect();
            return Err(format!("fix applied nothing ({})", why.join("; ")));
        }
        for file in &example.files {
            let want = example
                .fixed
                .iter()
                .find(|f| f.path == file.path)
                .map_or_else(|| file.text(), |f| f.text());
            let got = fs::read_to_string(dir.join(&file.path)).map_err(|e| e.to_string())?;
            // A final newline is the formatter's business, not the fix's.
            if got.trim_end_matches('\n') != want.trim_end_matches('\n') {
                return Err(format!(
                    "fix of `{}` is not what `fixed` says:\n{}",
                    file.path,
                    unified_diff(Path::new(&file.path), want, &got)
                ));
            }
        }
        let again = engine
            .check(&[], &[decision.id().to_owned()])
            .map_err(|e| e.to_string())?;
        if !again.diagnostics.is_empty() {
            let found: Vec<(u32, &str)> = again
                .diagnostics
                .iter()
                .map(|d| (d.span.start.line, d.message.as_str()))
                .collect();
            return Err(format!(
                "the rule still fires after the fix: {}",
                describe(&found)
            ));
        }
        let second = engine
            .fix(&plan, &run)
            .map_err(|e| format!("fix again: {e}"))?;
        if !second.applied.is_empty() || !second.changes.is_empty() {
            return Err(
                "the fix is not idempotent: applying it again changed the files".to_owned(),
            );
        }
        Ok(())
    }
}

fn config(registry: &Registry, decision: &Decision, example: &Example) -> Result<Config, String> {
    let level = decision
        .severity()
        .ok_or_else(|| "a decision without severity has no rule".to_owned())?;
    let mut rule = toml::Table::new();
    rule.insert("level".to_owned(), level.to_string().into());
    if !example.options.is_empty() {
        let mut options = toml::Table::new();
        for (key, value) in &example.options {
            options.insert(key.clone(), toml_value(value)?);
        }
        rule.insert("options".to_owned(), options.into());
    }
    let plugins: Vec<toml::Value> = registry
        .plugins()
        .map(|id| toml::Value::String(id.to_owned()))
        .collect();
    let mut rules = toml::Table::new();
    rules.insert(decision.id().to_owned(), rule.into());
    let mut root = toml::Table::new();
    root.insert("plugins".to_owned(), plugins.into());
    root.insert("rules".to_owned(), rules.into());
    Config::parse_inline(&root.to_string()).map_err(|e| e.to_string())
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
