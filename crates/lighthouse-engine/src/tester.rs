use std::{fmt::Write as _, fs, path::Path};

use tempfile::TempDir;

use lighthouse_plugin::Registry;
use lighthouse_spec::{
    Catalog, CheckKind, Config, Decision, Example, ExampleKind, Fix, SourceRuleSpec,
};
use serde_json::Value;

use crate::{Engine, FixPlan, FixRun, ModelOptions, Run, unified_diff};

mod cross;

use cross::{cross, introduced, judge, model_only};

/// Language of examples that need no language provider.
const NEUTRAL: &str = "text";

/// Runs the examples of catalog decisions through the whole engine, the way
/// `lighthouse check` would see them. The registry is built per example
/// because an engine owns its registry.
pub struct RuleTester<'a> {
    registry: Box<dyn Fn() -> Registry + 'a>,
    catalog: &'a Catalog,
    shipped: &'a Catalog,
    language: Option<String>,
    sources: Vec<SourceRuleSpec>,
    trusted: bool,
}

impl<'a> RuleTester<'a> {
    /// A tester over `catalog` that builds a fresh registry from `registry` for
    /// every example; `shipped` is the bundled catalog.
    pub fn new(
        registry: impl Fn() -> Registry + 'a,
        catalog: &'a Catalog,
        shipped: &'a Catalog,
    ) -> Self {
        Self {
            registry: Box::new(registry),
            catalog,
            shipped,
            language: None,
            sources: Vec::new(),
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

    /// Configures embedded-source selections used by the real project.
    pub fn sources(mut self, sources: &[SourceRuleSpec]) -> Self {
        self.sources = sources.to_vec();
        self
    }

    /// Failures of every checked decision; empty when all examples hold.
    pub fn check_all(&self) -> Vec<String> {
        self.catalog
            .decisions()
            .filter(|d| d.automated())
            .flat_map(|p| self.check(p))
            .collect()
    }

    /// Failures of one decision's examples, each prefixed `<id> <example>:`.
    /// With a language filter, a decision that does not apply to that language
    /// has nothing to show; one that does and has no example for it (and no
    /// language-neutral `text` example) is itself a failure: a checked rule
    /// must show what it does in every language it applies to. A command check
    /// that names the `files` it is run for is the exception: it has examples
    /// for the languages of those files only.
    pub fn check(&self, decision: &Decision) -> Vec<String> {
        self.tested(decision).failures
    }

    /// Like [`check`](Self::check), with the conflicts the examples declare and
    /// still have, which are not failures.
    pub fn tested(&self, decision: &Decision) -> Tested {
        let mut tested = Tested::default();
        if !self.applies(decision) {
            return tested;
        }
        let selected: Vec<&Example> = decision
            .examples
            .iter()
            .filter(|example| self.selects(example))
            .collect();
        if selected.is_empty() && self.language.is_some() {
            let language = self.language.as_deref().unwrap_or_default();
            if !exempt(decision, language) {
                tested.failures.push(format!(
                    "{}: no example for language `{language}`",
                    decision.id()
                ));
            }
            return tested;
        }
        let model_only = model_only(decision);
        for example in selected {
            // A candidate-only decision is not expected to emit a finding
            // without a bound model; its invalid example is checked by the
            // catalog's candidate-shape tests instead.
            if model_only && example.kind == ExampleKind::Invalid {
                continue;
            }
            if absent(decision, example).is_some() {
                continue;
            }
            match self.run(decision, example) {
                Ok(known) => tested.conflicts.extend(
                    known
                        .into_iter()
                        .map(|line| format!("{} {}: {line}", decision.id(), example.name)),
                ),
                Err(failure) => {
                    tested
                        .failures
                        .push(format!("{} {}: {failure}", decision.id(), example.name))
                }
            }
        }
        tested
    }

    /// The examples of `decision` that cannot run here because the program of
    /// its command check is not installed, one line each. They are not failures:
    /// the tool is the user's to install, and the decision says which it needs.
    pub fn skipped(&self, decision: &Decision) -> Vec<String> {
        if !self.applies(decision) {
            return Vec::new();
        }
        decision
            .examples
            .iter()
            .filter(|example| self.selects(example))
            .filter_map(|example| {
                let program = absent(decision, example)?;
                Some(format!(
                    "{} {}: `{program}` is not installed",
                    decision.id(),
                    example.name
                ))
            })
            .collect()
    }

    /// Whether the decision applies to the language under test, always when
    /// there is none.
    pub fn applies(&self, decision: &Decision) -> bool {
        self.language
            .as_deref()
            .is_none_or(|language| decision.applies_to(language))
    }

    fn selects(&self, example: &Example) -> bool {
        self.language
            .as_ref()
            .is_none_or(|language| *language == example.language || example.language == NEUTRAL)
    }

    /// Runs `example` of `decision`; the conflicts it declares and still has.
    fn run(&self, decision: &Decision, example: &Example) -> Result<Vec<String>, String> {
        let fixing = decision.fix.as_ref().is_some_and(Fix::deterministic)
            && example.kind == ExampleKind::Invalid
            && !example.fixed.is_empty();
        // A fix is judged by every rule of its engine, so the other decisions
        // have an engine of their own there.
        let others = if example.kind == ExampleKind::Valid || fixing {
            self.others(decision, example)
        } else {
            Vec::new()
        };
        let registry = (self.registry)();
        let shared: &[&Decision] = if fixing { &[] } else { &others };
        let config = config(&registry, decision, example, shared, &self.sources)?;
        let (outcome, engine, dir) = ExampleRun {
            registry,
            config,
            catalog: self.catalog,
            trusted: self.trusted,
            models: ModelOptions::default(),
        }
        .run_in(decision.id(), example)?;
        if !outcome.incomplete.is_empty() {
            return Err(format!("analysis incomplete: {:?}", outcome.incomplete));
        }
        let found = found(&outcome);
        match example.kind {
            ExampleKind::Valid if found.is_empty() => {
                judge(example, &cross(&engine, &others, example)?)
            }
            ExampleKind::Valid => Err(format!("expected no diagnostics, got {}", describe(&found))),
            ExampleKind::Invalid => {
                compare(example, &found)?;
                if !fixing {
                    return judge(example, &[]);
                }
                let crossing = self.crossing(&others, dir.path())?;
                let before = cross(&crossing, &others, example)?;
                self.check_fix(&engine, decision, example, dir.path())?;
                let after = cross(&crossing, &others, example)?;
                judge(example, &introduced(example, dir.path(), &before, after))
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
        let plan = FixPlan::from_catalog(self.catalog, self.shipped);
        let run = FixRun {
            rules: vec![decision.id().to_owned()],
            unsafe_fixes: Some(true),
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
            return Err(format!(
                "the rule still fires after the fix: {}",
                describe(&found(&again))
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

/// What the examples of a decision came to.
#[derive(Debug, Default)]
pub struct Tested {
    /// One line for each example that does not hold.
    pub failures: Vec<String>,
    /// One line for each conflict with another decision that an example
    /// declares and still has.
    pub conflicts: Vec<String>,
}

/// What an example is run with: the project's own configuration and plugins,
/// so that a rule bound to a model asks it.
pub struct ExampleRun<'a> {
    pub registry: Registry,
    pub config: Config,
    pub catalog: &'a Catalog,
    pub trusted: bool,
    pub models: ModelOptions,
}

impl ExampleRun<'_> {
    /// Checks the files of `example` in a directory of their own, with the
    /// rule `rule` only.
    pub fn run(self, rule: &str, example: &Example) -> Result<Run, String> {
        self.run_in(rule, example).map(|(run, ..)| run)
    }

    /// Like [`run`](Self::run), keeping the engine and the directory, for a
    /// caller that goes on to fix the files.
    fn run_in(self, rule: &str, example: &Example) -> Result<(Run, Engine, TempDir), String> {
        let dir = materialize(example)?;
        let engine = Engine::new(self.registry, self.config, self.catalog, dir.path())
            .map_err(|e| e.to_string())?
            .with_trust(self.trusted)
            .with_models(self.models);
        let run = engine
            .check(&[], &[rule.to_owned()])
            .map_err(|e| e.to_string())?;
        Ok((run, engine, dir))
    }
}

/// A directory holding the files of `example`.
fn materialize(example: &Example) -> Result<TempDir, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    for file in &example.files {
        let path = dir.path().join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&path, file.text()).map_err(|e| e.to_string())?;
    }
    Ok(dir)
}

/// The line and message of each finding of the run.
fn found(run: &Run) -> Vec<(u32, &str)> {
    run.diagnostics
        .iter()
        .map(|d| (d.span.start.line, d.message.as_str()))
        .collect()
}

fn config(
    registry: &Registry,
    decision: &Decision,
    example: &Example,
    others: &[&Decision],
    sources: &[SourceRuleSpec],
) -> Result<Config, String> {
    let mut rules: Vec<(&Decision, Option<&Example>)> = vec![(decision, Some(example))];
    rules.extend(others.iter().map(|d| (*d, None)));
    rules_config(registry, &rules, sources)
}

/// A configuration that enables each of `rules` at its decision's severity,
/// with the options of the example it is given, else the decision's own.
fn rules_config(
    registry: &Registry,
    rules: &[(&Decision, Option<&Example>)],
    sources: &[SourceRuleSpec],
) -> Result<Config, String> {
    let mut table = toml::Table::new();
    for (decision, example) in rules {
        let level = decision
            .severity
            .ok_or_else(|| "a decision without severity has no rule".to_owned())?;
        let mut rule = toml::Table::new();
        rule.insert("level".to_owned(), level.to_string().into());
        if let Some(example) = example.filter(|e| !e.options.is_empty()) {
            let mut options = toml::Table::new();
            for (key, value) in &example.options {
                options.insert(key.clone(), toml_value(value)?);
            }
            rule.insert("options".to_owned(), options.into());
        }
        table.insert(decision.id().to_owned(), rule.into());
    }
    let plugins: Vec<toml::Value> = registry
        .manifests()
        .iter()
        .map(|plugin| toml::Value::String(plugin.id.clone()))
        .collect();
    let mut root = toml::Table::new();
    root.insert("plugins".to_owned(), plugins.into());
    root.insert("rules".to_owned(), table.into());
    let sources: Vec<toml::Value> = sources
        .iter()
        .map(|source| toml::Value::try_from(source).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    if !sources.is_empty() {
        root.insert("sources".to_owned(), sources.into());
    }
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

/// Whether `decision` needs no example in `language`: its check is a command
/// that names the files it is run for, or has no program for the language.
fn exempt(decision: &Decision, language: &str) -> bool {
    match decision.check.as_ref().map(|check| &check.kind) {
        Some(CheckKind::Command(command)) => {
            !command.files.is_empty() || command.programs(decision).of(language).is_none()
        }
        _ => false,
    }
}

/// The program of `decision`'s command check for the language of `example`
/// when it is not on the `PATH`.
fn absent(decision: &Decision, example: &Example) -> Option<String> {
    let Some(CheckKind::Command(command)) = decision.check.as_ref().map(|check| &check.kind) else {
        return None;
    };
    let program = command
        .programs(decision)
        .of(&example.language)?
        .command
        .first()?
        .clone();
    if program.contains('/') || on_path(&program) {
        return None;
    }
    Some(program)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| fs::metadata(dir.join(program)).is_ok_and(|meta| meta.is_file()))
    })
}
