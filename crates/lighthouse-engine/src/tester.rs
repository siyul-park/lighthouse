use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    ops::{Add, AddAssign},
    path::Path,
    time::{Duration, Instant},
};

use rayon::prelude::*;
use tempfile::TempDir;

use lighthouse_plugin::Registry;
use lighthouse_spec::{Catalog, CheckKind, Config, Decision, Example, ExampleKind, Fix};
use serde_json::{Map, Value};

use crate::{Engine, FixPlan, FixRun, ModelOptions, Run, unified_diff};

mod cross;
mod witness;

use cross::{cross, hits_in, introduced, judge, model_only};
pub use witness::{
    Alphabet, Arrangement, Basis, Bounded, Cell, Contradiction, Decl, Edge, Flagged, Forced,
    Incomplete, Kind, Language, MAX_BOUND, Oracle, Relation, Scratch, Searched, Shape, Single,
    Unit, Verdict, Visibility, Witness, enumerate,
};

/// Stack of a worker that checks decisions: the engine under an example
/// recurses deeply.
const WORKER_STACK: usize = 8 << 20;

/// Language of examples that need no language provider.
const NEUTRAL: &str = "text";

/// Runs the examples of catalog decisions through the whole engine, the way
/// `lighthouse check` would see them. The registry is built per example
/// because an engine owns its registry.
pub struct RuleTester<'a> {
    registry: Box<dyn Fn() -> Registry + Send + Sync + 'a>,
    catalog: &'a Catalog,
    shipped: &'a Catalog,
    /// The decisions whose programs are Lighthouse's own, which run without
    /// trust.
    bundled: &'a Catalog,
    language: Option<String>,
    trusted: bool,
}

impl<'a> RuleTester<'a> {
    /// A tester over `catalog` that builds a fresh registry from `registry` for
    /// every example; `shipped` is the bundled catalog.
    pub fn new(
        registry: impl Fn() -> Registry + Send + Sync + 'a,
        catalog: &'a Catalog,
        shipped: &'a Catalog,
    ) -> Self {
        Self {
            registry: Box::new(registry),
            catalog,
            shipped,
            bundled: shipped,
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

    /// The catalog compiled into the tool, when `shipped` holds more: a
    /// command check of its decisions needs no trust, as in a run.
    pub fn bundled(mut self, bundled: &'a Catalog) -> Self {
        self.bundled = bundled;
        self
    }

    /// Failures of every checked decision; empty when all examples hold. The
    /// decisions are checked side by side and their failures put in catalog
    /// order.
    pub fn check_all(&self) -> Vec<String> {
        let decisions: Vec<&Decision> =
            self.catalog.decisions().filter(|d| d.automated()).collect();
        let pool = rayon::ThreadPoolBuilder::new()
            .stack_size(WORKER_STACK)
            .thread_name(|n| format!("lighthouse-check-{n}"))
            .build();
        let failures: Vec<Vec<String>> = match pool {
            Ok(pool) => pool.install(|| decisions.par_iter().map(|d| self.check(d)).collect()),
            Err(_) => decisions.iter().map(|d| self.check(d)).collect(),
        };
        failures.into_iter().flatten().collect()
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
        let registry = (self.registry)();
        for example in selected {
            // A candidate-only decision is not expected to emit a finding
            // without a bound model; its invalid example is checked by the
            // catalog's candidate-shape tests instead.
            if model_only && example.kind == ExampleKind::Invalid {
                continue;
            }
            if self.unrunnable(decision, example).is_some()
                || !self.lacking(&registry, example).is_empty()
            {
                continue;
            }
            match self.run(decision, example, &mut tested.spent) {
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

    /// The examples of `decision` that cannot run here, one line each: the
    /// program of its command check is not installed, or is not run because
    /// the project is not trusted. They are not failures: the tool is the
    /// user's to install and trust is the user's to give. An example that
    /// needs a plugin that is not available is counted by
    /// [`lacking`](Self::lacking) instead.
    pub fn skipped(&self, decision: &Decision) -> Vec<String> {
        if !self.applies(decision) {
            return Vec::new();
        }
        decision
            .examples
            .iter()
            .filter(|example| self.selects(example))
            .filter_map(|example| {
                let reason = self.unrunnable(decision, example)?;
                Some(format!("{} {}: {reason}", decision.id(), example.name))
            })
            .collect()
    }

    /// The plugins that examples of `decision` need and the registry lacks,
    /// each with how many of its examples are not run for that. A plugin is
    /// needed by an example when a shareable project whose injections produce
    /// the language of the example lists it, as `core/authoring` lists the
    /// plugins of the files its CEL is written in.
    pub fn lacking_plugins(&self, decision: &Decision) -> BTreeMap<String, usize> {
        let mut lacking = BTreeMap::new();
        if !self.applies(decision) {
            return lacking;
        }
        let registry = (self.registry)();
        for example in decision.examples.iter().filter(|e| self.selects(e)) {
            if self.unrunnable(decision, example).is_some() {
                continue;
            }
            for plugin in self.lacking(&registry, example) {
                *lacking.entry(plugin).or_default() += 1;
            }
        }
        lacking
    }

    /// Why the example cannot run here, when it cannot: its command is not
    /// installed, or is not run in a project that is not trusted.
    fn unrunnable(&self, decision: &Decision, example: &Example) -> Option<String> {
        let program = program(decision, example)?;
        if !program.contains('/') && !on_path(&program) {
            return Some(format!("`{program}` is not installed"));
        }
        (!self.trusted && !decision.bundled(self.bundled))
            .then(|| format!("`{program}` is not run: this project is not trusted to run commands"))
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

    /// The shareable projects an example of `language` runs extending: those
    /// of the catalog whose injections produce that language, in name order.
    /// What a language is written in is the data of those projects; the
    /// example never sees the injections of the project that runs it.
    fn extends(&self, language: &str) -> Vec<String> {
        self.catalog
            .projects()
            .injecting(language)
            .map(str::to_owned)
            .collect()
    }

    /// The plugins an example needs that `registry` lacks: those the
    /// projects it extends list, directly or through their own `extends`.
    fn lacking(&self, registry: &Registry, example: &Example) -> Vec<String> {
        let extends = self.extends(&example.language);
        if extends.is_empty() {
            return Vec::new();
        }
        let inherited = extending(&extends).and_then(|config| {
            config
                .inheriting(self.catalog.projects())
                .map_err(|e| e.to_string())
        });
        inherited.map_or_else(
            |_| Vec::new(),
            |config| {
                config
                    .plugins()
                    .iter()
                    .filter(|plugin| !registry.has_plugin(&plugin.id))
                    .map(|plugin| plugin.id.clone())
                    .collect()
            },
        )
    }

    /// Runs `example` of `decision`; the conflicts it declares and still has.
    fn run(
        &self,
        decision: &Decision,
        example: &Example,
        spent: &mut Spent,
    ) -> Result<Vec<String>, String> {
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
        let started = Instant::now();
        let registry = (self.registry)();
        let shared: &[&Decision] = if fixing { &[] } else { &others };
        let mut configured: Vec<(&Decision, Option<&Map<String, Value>>)> =
            vec![(decision, Some(&example.options))];
        configured.extend(shared.iter().map(|other| (*other, None)));
        let config = rules_config(
            self.catalog,
            &registry,
            &configured,
            &self.extends(&example.language),
        )?;
        // A valid example is checked once for its own decision and the others,
        // and the findings are told apart by their rule.
        let mut rules = vec![decision.id().to_owned()];
        if example.kind == ExampleKind::Valid {
            rules.extend(others.iter().map(|d| d.id().to_owned()));
        }
        let (mut together, engine, dir) = ExampleRun {
            registry,
            config,
            catalog: self.catalog,
            trusted: self.trusted,
            models: ModelOptions::default(),
        }
        .run_rules(&rules, example)?;
        // What stops the others from running does not stop the decision: an
        // incomplete run is asked again for the decision alone.
        let (outcome, hits) = if together.incomplete.is_empty() {
            let hits = hits_in(&together, example, |rule| rule != decision.id());
            together.diagnostics.retain(|d| d.rule_id == decision.id());
            (together, Some(hits))
        } else if rules.len() > 1 {
            let own = engine.check(&[], &rules[..1]).map_err(|e| e.to_string())?;
            (own, None)
        } else {
            (together, None)
        };
        spent.check += started.elapsed();
        if !outcome.incomplete.is_empty() {
            return Err(format!("analysis incomplete: {:?}", outcome.incomplete));
        }
        let found = found(&outcome);
        match example.kind {
            ExampleKind::Valid if found.is_empty() => {
                let hits = timed(&mut spent.crossing, || {
                    hits.map_or_else(|| cross(&engine, &others, example), Ok)
                });
                judge(example, &hits?)
            }
            ExampleKind::Valid => Err(format!("expected no diagnostics, got {}", describe(&found))),
            ExampleKind::Invalid => {
                compare(example, &found)?;
                if !fixing {
                    return judge(example, &[]);
                }
                let (crossing, before) = timed(&mut spent.crossing, || {
                    let crossing = self.crossing(&others, example, dir.path())?;
                    let before = cross(&crossing, &others, example)?;
                    Ok::<_, String>((crossing, before))
                })?;
                timed(&mut spent.fix, || {
                    self.check_fix(&engine, decision, example, dir.path())
                })?;
                let after = timed(&mut spent.crossing, || cross(&crossing, &others, example))?;
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
    /// Where the time of its examples went.
    pub spent: Spent,
}

/// Where the time of running examples went, summed over them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spent {
    /// Building the registry and engine of the example and checking it.
    pub check: Duration,
    /// Applying the decision's fix and judging the result.
    pub fix: Duration,
    /// Checking the example against the other decisions.
    pub crossing: Duration,
}

impl Spent {
    /// Everything together.
    pub fn total(&self) -> Duration {
        self.check + self.fix + self.crossing
    }
}

impl Add for Spent {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            check: self.check + other.check,
            fix: self.fix + other.fix,
            crossing: self.crossing + other.crossing,
        }
    }
}

impl AddAssign for Spent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
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
        self.run_rules(&[rule.to_owned()], example)
    }

    /// Like [`run_in`](Self::run_in), with several rules.
    fn run_rules(
        self,
        rules: &[String],
        example: &Example,
    ) -> Result<(Run, Engine, TempDir), String> {
        let dir = materialize(example)?;
        let engine = Engine::new(self.registry, self.config, self.catalog, dir.path())
            .map_err(|e| e.to_string())?
            .with_trust(self.trusted)
            .with_models(self.models);
        let run = engine.check(&[], rules).map_err(|e| e.to_string())?;
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

/// Runs `work`, adding the time it takes to `slot`.
fn timed<R>(slot: &mut Duration, work: impl FnOnce() -> R) -> R {
    let started = Instant::now();
    let done = work();
    *slot += started.elapsed();
    done
}

/// The line and message of each finding of the run.
fn found(run: &Run) -> Vec<(u32, &str)> {
    run.diagnostics
        .iter()
        .map(|d| (d.span.start.line, d.message.as_str()))
        .collect()
}

/// A configuration that enables each of `rules` at its decision's severity,
/// with the options of the example it is given, else the decision's own. The
/// options an example sets that its decision reads from another
/// (`options.from`) go to that decision, which is enabled with them.
fn rules_config(
    catalog: &Catalog,
    registry: &Registry,
    rules: &[(&Decision, Option<&Map<String, Value>>)],
    extends: &[String],
) -> Result<Config, String> {
    let mut table = toml::Table::new();
    let mut set = |id: &str, level: String, options: toml::Table| {
        let rule = table
            .entry(id.to_owned())
            .or_insert_with(|| toml::Table::new().into())
            .as_table_mut()
            .expect("a rule is a table");
        rule.insert("level".to_owned(), level.into());
        if !options.is_empty() {
            let own = rule
                .entry("options".to_owned())
                .or_insert_with(|| toml::Table::new().into())
                .as_table_mut()
                .expect("options are a table");
            own.extend(options);
        }
    };
    for (decision, example) in rules {
        let level = decision
            .severity
            .ok_or_else(|| "a decision without severity has no rule".to_owned())?;
        let source = decision
            .options_from()
            .and_then(|from| catalog.decision(from));
        let inherited = source
            .map(|s| decision.inheriting(s))
            .and_then(|d| d.options.as_ref().map(|options| options.inherited.clone()));
        let (mut own, mut read) = (toml::Table::new(), toml::Table::new());
        for (key, value) in example.iter().flat_map(|options| options.iter()) {
            let into = if inherited.as_ref().is_some_and(|keys| keys.contains(key)) {
                &mut read
            } else {
                &mut own
            };
            into.insert(key.clone(), toml_value(value)?);
        }
        set(decision.id(), level.to_string(), own);
        if let Some(source) = source.filter(|_| !read.is_empty()) {
            let level = source
                .severity
                .ok_or_else(|| "a decision without severity has no rule".to_owned())?;
            set(source.id(), level.to_string(), read);
        }
    }
    let plugins: Vec<toml::Value> = registry
        .manifests()
        .iter()
        .map(|plugin| toml::Value::String(plugin.id.clone()))
        .collect();
    let mut root = toml::Table::new();
    root.insert("plugins".to_owned(), plugins.into());
    root.insert("rules".to_owned(), table.into());
    if !extends.is_empty() {
        root.insert(
            "extends".to_owned(),
            toml::Value::try_from(extends).map_err(|e| e.to_string())?,
        );
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

/// The program of `decision`'s command check for the language of `example`.
fn program(decision: &Decision, example: &Example) -> Option<String> {
    let Some(CheckKind::Command(command)) = decision.check.as_ref().map(|check| &check.kind) else {
        return None;
    };
    command
        .programs(decision)
        .of(&example.language)?
        .command
        .first()
        .cloned()
}

/// A configuration that extends `projects` and sets nothing else.
fn extending(projects: &[String]) -> Result<Config, String> {
    let list = toml::Value::try_from(projects).map_err(|e| e.to_string())?;
    Config::parse_inline(&format!("extends = {list}")).map_err(|e| e.to_string())
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| fs::metadata(dir.join(program)).is_ok_and(|meta| meta.is_file()))
    })
}
