use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use ignore::WalkBuilder;
use lighthouse_config::{Config, GlobSet, Rules, glob_set};
use lighthouse_model::{
    Diagnostic, File, Fingerprint, Fragment, Incomplete, Options, Project, Severity,
};
use lighthouse_plugin::{Ctx, Facts, LanguageProvider, Registry, Rule, Scope, Source, Workspace};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    annotations::{self, Allowed},
    identity,
    subject::Subjects,
};

/// Name of the file, in `.gitignore` syntax, that keeps files out of the
/// analysis altogether, such as fixtures that are broken on purpose.
pub const IGNORE_FILE: &str = ".lighthouseignore";

/// Process exit code of a run whose analysis was incomplete.
pub const EXIT_INCOMPLETE: u8 = 3;

/// Why a run could not start or finish: invalid configuration, unknown or
/// unlisted plugin or rule, or an unreadable path. Analysis gaps are not
/// errors; they are [`Outcome::incomplete`].
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] lighthouse_config::Error),
    #[error(transparent)]
    Plugin(#[from] lighthouse_plugin::Error),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    #[error("`{0}` belongs to a plugin not listed in `plugins`")]
    PluginNotListed(String),
    #[error("unknown rule `{0}`")]
    UnknownRule(String),
    #[error("rule `{0}` is not enabled by the configuration")]
    RuleNotEnabled(String),
    /// A fix run that cannot start or finish safely: incomplete analysis, a
    /// file changed under it, a formatter that cannot run, an unknown fixer.
    #[error("{0}")]
    Fix(String),
}

/// The result of a run: findings, notices and the parts that were not analyzed.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Sorted by file, line, column, rule; limited to the requested paths.
    pub diagnostics: Vec<Diagnostic>,
    /// Facts the user should know that leave the analysis complete: rules
    /// skipped for a missing provider capability, build-variant duplicates,
    /// ambiguous edge targets, provider messages.
    pub notices: BTreeSet<String>,
    /// Parts of the analysis scope that were not analyzed, sorted. Covers the
    /// whole project whatever paths were requested: paths only filter what is
    /// reported, not what is analyzed. Unreadable files, files a language
    /// provider could not parse or load, provider crashes and timeouts, and
    /// requested paths outside the root are incomplete. A non-UTF-8 file
    /// claimed only by a fallback provider (binary data) is a notice.
    pub incomplete: Vec<Incomplete>,
    /// The report scope: project-relative paths (a file, or a directory
    /// prefix) whose findings `diagnostics` holds. The empty path is the
    /// whole project; no entries means nothing was in scope.
    pub reported: Vec<PathBuf>,
    /// The rules that ran; a finding of any other rule says nothing about
    /// the code in this run.
    pub rules: Vec<String>,
    /// What the analysis knew about each finding's subject (language, symbol
    /// shape, function summary, measures), by fingerprint.
    pub facts: BTreeMap<Fingerprint, Value>,
    /// The options the configuration set for each finding's rule, by
    /// fingerprint; options left to the decision's defaults are absent.
    pub options: BTreeMap<Fingerprint, Options>,
    /// Every rule the configuration enables for some file, whether or not it
    /// ran: a remembered finding of any other rule is no longer configured.
    pub configured: Vec<String>,
    /// Findings that source annotations allow, within the report scope. They
    /// are not in `diagnostics` and never fail the run.
    pub allowed: Vec<Allowed>,
    /// The analysis the findings came from; fixes read it.
    pub project: Project,
}

impl Outcome {
    /// Process exit code: 3 if the analysis is incomplete, unless
    /// `allow_incomplete`; else 1 if any error diagnostic, or when the
    /// warnings fail the run as `fail_on` says; otherwise 0. `info` never
    /// fails a run. The CLI exits 2 on usage and runtime errors. A `bool`
    /// stands for `FailOn { strict, .. }`.
    pub fn exit_code(&self, fail_on: impl Into<FailOn>, allow_incomplete: bool) -> u8 {
        if !self.incomplete.is_empty() && !allow_incomplete {
            return EXIT_INCOMPLETE;
        }
        let fail_on = fail_on.into();
        let count = |severity: Severity| {
            self.diagnostics
                .iter()
                .filter(|d| d.severity == severity)
                .count()
        };
        let warnings = count(Severity::Warn);
        let fails = count(Severity::Error) > 0
            || (fail_on.strict && warnings > 0)
            || fail_on.max_warnings.is_some_and(|max| warnings > max);
        u8::from(fails)
    }
}

/// When warnings fail a run. Errors always do and `info` never does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FailOn {
    /// Any warning fails the run.
    pub strict: bool,
    /// More than this many warnings fail the run.
    pub max_warnings: Option<usize>,
}

impl From<bool> for FailOn {
    fn from(strict: bool) -> Self {
        Self {
            strict,
            max_warnings: None,
        }
    }
}

/// Replacement texts by project-relative path; see [`Engine::check_overlaid`].
pub type Overlays = BTreeMap<PathBuf, String>;

/// What a run reports: everything under some paths, or exactly some files.
enum Reported<'a> {
    Paths(&'a [PathBuf]),
    Files(&'a [PathBuf]),
}

struct Language {
    files: GlobSet,
    tests: GlobSet,
}

pub(crate) struct Input {
    pub(crate) file: File,
    pub(crate) text: String,
    language: usize,
}

/// A configured set of plugins that analyzes one project root. Running it does
/// not modify the project or the engine.
pub struct Engine {
    pub(crate) registry: Registry,
    pub(crate) config: Config,
    pub(crate) ws: Workspace,
    /// Providers from listed plugins; indexes into `languages`.
    providers: Vec<usize>,
    languages: Vec<Language>,
    /// Rules enabled by the configuration for at least one file.
    active: BTreeSet<String>,
    /// Gaps known before the run, such as a plugin that failed to start.
    startup: Vec<Incomplete>,
    trusted: bool,
}

impl Engine {
    /// `root` is the directory of `lighthouse.toml`; globs match paths relative to it.
    pub fn new(registry: Registry, config: Config, root: &Path) -> Result<Self, Error> {
        validate_config(&registry, &config)?;
        let (providers, languages) = load_languages(&registry, &config)?;
        let ws = Workspace {
            root: root.canonicalize().map_err(io_error(root))?,
            languages: config.languages().clone(),
            overlays: BTreeMap::new(),
        };
        let mut engine = Self {
            registry,
            config,
            ws,
            providers,
            languages,
            active: BTreeSet::new(),
            startup: Vec::new(),
            trusted: false,
        };
        engine.active = active_rules(&engine.registry, &engine.config)?;
        Ok(engine)
    }

    /// Says whether the user trusts the project to run the commands its checks name.
    pub fn with_trust(mut self, trusted: bool) -> Self {
        self.trusted = trusted;
        self
    }

    /// Adds gaps found while assembling the plugins; every run reports them.
    pub fn with_incomplete(mut self, gaps: Vec<Incomplete>) -> Self {
        self.startup.extend(gaps);
        self
    }

    /// Checks the whole project, then reports diagnostics under `paths`.
    /// `only` restricts which rules run; empty means all enabled rules.
    pub fn check(&self, paths: &[PathBuf], only: &[String]) -> Result<Outcome, Error> {
        self.run(Reported::Paths(paths), only, &Overlays::new())
    }

    /// Like `check`, but the project is read with `overlays` standing in for
    /// the files of the same path: nothing is read from or written to those
    /// files on disk. This is how a fix verifies an edit before writing it.
    pub fn check_overlaid(
        &self,
        paths: &[PathBuf],
        only: &[String],
        overlays: &Overlays,
    ) -> Result<Outcome, Error> {
        self.run(Reported::Paths(paths), only, overlays)
    }

    /// Checks the whole project, then reports only the diagnostics in `files`,
    /// project-relative paths of changed files. Unlike `check`, no files means
    /// nothing is reported, not everything: analysis scope is unchanged, only
    /// the report is narrowed.
    pub fn check_files(&self, files: &[PathBuf], only: &[String]) -> Result<Outcome, Error> {
        self.run(Reported::Files(files), only, &Overlays::new())
    }

    fn run(
        &self,
        reported: Reported,
        only: &[String],
        overlays: &Overlays,
    ) -> Result<Outcome, Error> {
        for id in only {
            if self.registry.rule(id).is_none() {
                return Err(Error::UnknownRule(id.clone()));
            }
            if !self.active.contains(id) {
                return Err(Error::RuleNotEnabled(id.clone()));
            }
        }
        let selected: Vec<&dyn Rule> = self
            .active
            .iter()
            .filter(|id| only.is_empty() || only.contains(id))
            .filter_map(|id| self.registry.rule(id))
            .filter(|rule| rule.manifest().enforced || only.contains(&rule.manifest().id))
            .collect();

        let mut outcome = Outcome::default();
        let mut incomplete = self.startup.clone();
        let scopes = match reported {
            Reported::Paths(paths) => self.scopes(paths, &mut incomplete)?,
            Reported::Files(files) => files.to_vec(),
        };
        let inputs = self.read(overlays, &mut outcome.notices, &mut incomplete);
        let (inputs, project) =
            self.build_project(inputs, overlays, &mut outcome.notices, &mut incomplete);
        let facts = self.analyze(&selected, &inputs, &project)?;
        let found = self.apply(
            &selected,
            &inputs,
            &project,
            &facts,
            (&mut outcome.notices, &mut incomplete),
        )?;

        let ran: BTreeSet<String> = selected.iter().map(|r| r.manifest().id.clone()).collect();
        let level = |rule: &str, file: &Path, lang: &str| self.level_at(rule, file, lang);
        let gate = annotations::Gate {
            level: &level,
            active: &self.active,
            selected: &ran,
        };
        let (mut found, allowed) = annotations::apply(found, &project, &gate);
        found.retain(|d| scopes.iter().any(|s| d.file.starts_with(s)));
        outcome.allowed = allowed
            .into_iter()
            .filter(|a| scopes.iter().any(|s| a.diagnostic.file.starts_with(s)))
            .collect();
        found.sort_by(|a, b| {
            (&a.file, a.span.start, &a.rule_id).cmp(&(&b.file, b.span.start, &b.rule_id))
        });
        let ordinal = identity::assign(&mut found, &project);
        let subjects = Subjects::new(&project, &facts, &inputs);
        outcome.facts = found
            .iter()
            .map(|d| {
                let mut subject = subjects.of(d);
                if ordinal.contains(&d.fingerprint) {
                    subject["ordinal"] = Value::Bool(true);
                }
                (d.fingerprint.clone(), subject)
            })
            .collect();
        outcome.options = self.options_of(&found, &project)?;
        outcome.diagnostics = found;
        outcome.reported = scopes;
        outcome.project = project;
        outcome.rules = ran.into_iter().collect();
        outcome.configured = self.active.iter().cloned().collect();
        incomplete.sort();
        incomplete.dedup();
        outcome.incomplete = incomplete;
        Ok(outcome)
    }

    /// The level the configuration gives a rule at a file; `None` when off.
    fn level_at(&self, rule: &str, file: &Path, lang: &str) -> Option<Severity> {
        let rules = self
            .config
            .resolve(file, lang, &|id| self.preset_rules(id))
            .ok()?;
        rules.get(rule)?.level
    }

    /// The options the configuration resolves for each finding's rule at its
    /// file (project-scope rules at the project).
    fn options_of(
        &self,
        found: &[Diagnostic],
        project: &Project,
    ) -> Result<BTreeMap<Fingerprint, Options>, Error> {
        let mut resolved: BTreeMap<(PathBuf, String), Rules> = BTreeMap::new();
        let mut options = BTreeMap::new();
        for d in found {
            let project_scope = self
                .registry
                .rule(&d.rule_id)
                .is_some_and(|r| r.manifest().scope == Scope::Project);
            let (path, lang) = match project.file(&d.file) {
                Some(file) if !project_scope => (file.path.clone(), file.lang.clone()),
                _ => (PathBuf::new(), String::new()),
            };
            let rules = match resolved.entry((path, lang)) {
                std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::btree_map::Entry::Vacant(e) => {
                    let (path, lang) = e.key().clone();
                    let rules = self
                        .config
                        .resolve(&path, &lang, &|id| self.preset_rules(id))?;
                    e.insert(rules)
                }
            };
            let found = rules.get(&d.rule_id).map(|c| c.options.clone());
            options.insert(d.fingerprint.clone(), found.unwrap_or_default());
        }
        Ok(options)
    }

    /// Report paths relative to the root; those outside it are incomplete.
    pub(crate) fn scopes(
        &self,
        paths: &[PathBuf],
        incomplete: &mut Vec<Incomplete>,
    ) -> Result<Vec<PathBuf>, Error> {
        if paths.is_empty() {
            return Ok(vec![PathBuf::new()]);
        }
        let mut scopes = Vec::new();
        for path in paths {
            let abs = path.canonicalize().map_err(io_error(path))?;
            match abs.strip_prefix(&self.ws.root) {
                Ok(rel) => scopes.push(rel.to_owned()),
                Err(_) => incomplete.push(Incomplete {
                    path: Some(path.clone()),
                    reason: format!("outside {}, not checked", self.ws.root.display()),
                }),
            }
        }
        Ok(scopes)
    }

    /// Reads every file under the root that a listed language claims.
    fn read(
        &self,
        overlays: &Overlays,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> Vec<Input> {
        let mut inputs = Vec::new();
        let walk = WalkBuilder::new(&self.ws.root)
            .require_git(false)
            .add_custom_ignore_filename(IGNORE_FILE)
            .build();
        for entry in walk {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    incomplete.push(Incomplete {
                        path: None,
                        reason: format!("walk: {e}"),
                    });
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&self.ws.root) else {
                incomplete.push(Incomplete {
                    path: Some(entry.path().to_owned()),
                    reason: "outside the root, not checked".to_owned(),
                });
                continue;
            };
            if let Some(mut input) = self.read_input(entry.path(), rel, notices, incomplete) {
                if let Some(text) = overlays.get(rel) {
                    input.file.hash = hash_of(text);
                    input.text.clone_from(text);
                }
                inputs.push(input);
            }
        }
        inputs.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        inputs
    }

    /// Reads one file if a listed language claims it; a file that cannot be read
    /// becomes an incomplete entry (or a notice for binary data) and is dropped.
    fn read_input(
        &self,
        path: &Path,
        rel: &Path,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> Option<Input> {
        let &language = self
            .providers
            .iter()
            .find(|&&i| self.languages[i].files.is_match(rel))?;
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                if self.provider(language).manifest().fallback {
                    notices.insert(format!("{}: skipped, not valid UTF-8", rel.display()));
                } else {
                    incomplete.push(Incomplete {
                        path: Some(rel.to_owned()),
                        reason: "not valid UTF-8".to_owned(),
                    });
                }
                return None;
            }
            Err(e) => {
                incomplete.push(Incomplete {
                    path: Some(rel.to_owned()),
                    reason: format!("unreadable: {e}"),
                });
                return None;
            }
        };
        let file = File {
            path: rel.to_owned(),
            lang: self.provider(language).manifest().id.clone(),
            hash: hash_of(&text),
            generated: false,
            test: self.languages[language].tests.is_match(rel),
        };
        Some(Input {
            file,
            text,
            language,
        })
    }

    fn provider(&self, language: usize) -> &dyn LanguageProvider {
        self.registry
            .languages()
            .nth(language)
            .map(|(_, l)| l)
            .expect("language index comes from the registry")
    }

    /// Has each provider index its files in one batch and merges the fragments. Files a provider did not
    /// index drop out; the reason is recorded as incomplete.
    fn build_project(
        &self,
        inputs: Vec<Input>,
        overlays: &Overlays,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> (Vec<Input>, Project) {
        let ws = Workspace {
            overlays: overlays.clone(),
            ..self.ws.clone()
        };
        let mut batches: BTreeMap<usize, Vec<&Input>> = BTreeMap::new();
        for input in &inputs {
            batches.entry(input.language).or_default().push(input);
        }
        let mut parts: Vec<Fragment> = Vec::new();
        for (language, batch) in batches {
            let provider = self.provider(language);
            let sources: Vec<Source> = batch
                .iter()
                .map(|i| Source {
                    file: &i.file,
                    text: &i.text,
                })
                .collect();
            match provider.index(&ws, &sources) {
                Ok(indexed) => {
                    parts.extend(indexed.fragments);
                    notices.extend(indexed.notices);
                    incomplete.extend(indexed.incomplete);
                }
                Err(e) => incomplete.push(Incomplete {
                    path: None,
                    reason: format!(
                        "language `{}` had an execution error, {} file(s) not analyzed: {e}",
                        provider.manifest().id,
                        batch.len()
                    ),
                }),
            }
        }
        let project = Project::merge(parts);
        notices.extend(project.notices().iter().cloned());
        let kept = inputs
            .into_iter()
            .filter(|i| project.file(&i.file.path).is_some())
            .collect();
        (kept, project)
    }

    fn analyze(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
    ) -> Result<Facts, Error> {
        let wanted = rules
            .iter()
            .flat_map(|r| r.manifest().analyzers.iter().map(String::as_str));
        let mut facts = Facts::new();
        for analyzer in self.registry.order(wanted)? {
            let runs: Vec<(Option<&Input>, String)> = match analyzer.manifest().scope {
                Scope::Project => vec![(None, String::new())],
                Scope::File => inputs
                    .iter()
                    .map(|i| (Some(i), i.file.path.to_string_lossy().into_owned()))
                    .collect(),
            };
            for (input, key) in runs {
                let ctx = Ctx {
                    ws: &self.ws,
                    project,
                    file: input.map(|i| (&i.file, i.text.as_str())),
                    facts: &facts,
                    keys: &self.registry,
                    trusted: self.trusted,
                };
                let fact = analyzer.run(&ctx)?;
                facts.insert((analyzer.manifest().id.clone(), key), fact);
            }
        }
        Ok(facts)
    }

    fn apply(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
        facts: &Facts,
        (notices, incomplete): (&mut BTreeSet<String>, &mut Vec<Incomplete>),
    ) -> Result<Vec<Diagnostic>, Error> {
        let mut found =
            self.apply_file_rules(rules, inputs, project, facts, notices, incomplete)?;
        found.extend(self.apply_project_rules(rules, inputs, project, facts, notices, incomplete)?);
        Ok(found)
    }

    fn apply_file_rules(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
        facts: &Facts,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let mut found = Vec::new();
        for input in inputs {
            let resolved = self
                .config
                .resolve(&input.file.path, &input.file.lang, &|id| {
                    self.preset_rules(id)
                })?;
            let provider = self.provider(input.language);
            for rule in rules.iter().filter(|r| r.manifest().scope == Scope::File) {
                let meta = rule.manifest();
                let Some(config) = resolved.get(&meta.id) else {
                    continue;
                };
                let Some(level) = config.level else { continue };
                if let Some(missing) = meta
                    .capabilities
                    .iter()
                    .find(|c| !provider.manifest().capabilities.contains(c))
                {
                    notices.insert(format!(
                        "{}: skipped for language `{}`, missing capability {missing}",
                        meta.id, input.file.lang
                    ));
                    continue;
                }
                let ctx = Ctx {
                    ws: &self.ws,
                    project,
                    file: Some((&input.file, input.text.as_str())),
                    facts,
                    keys: &self.registry,
                    trusted: self.trusted,
                };
                let checked = rule.check(&ctx, &config.options);
                for mut d in unfinished(checked, &meta.id, Some(&input.file.path), incomplete)? {
                    d.severity = level;
                    found.push(d);
                }
            }
        }
        Ok(found)
    }

    fn apply_project_rules(
        &self,
        rules: &[&dyn Rule],
        inputs: &[Input],
        project: &Project,
        facts: &Facts,
        notices: &mut BTreeSet<String>,
        incomplete: &mut Vec<Incomplete>,
    ) -> Result<Vec<Diagnostic>, Error> {
        let languages: BTreeSet<usize> = inputs.iter().map(|i| i.language).collect();
        let resolved = self
            .config
            .resolve(Path::new(""), "", &|id| self.preset_rules(id))?;
        let mut found = Vec::new();
        for rule in rules
            .iter()
            .filter(|r| r.manifest().scope == Scope::Project)
        {
            let meta = rule.manifest();
            let Some(config) = resolved.get(&meta.id) else {
                continue;
            };
            let Some(level) = config.level else { continue };
            let missing = meta.capabilities.iter().find(|c| {
                languages
                    .iter()
                    .any(|&l| !self.provider(l).manifest().capabilities.contains(c))
            });
            if let Some(missing) = missing {
                notices.insert(format!(
                    "{}: skipped, missing capability {missing}",
                    meta.id
                ));
                continue;
            }
            let ctx = Ctx {
                ws: &self.ws,
                project,
                file: None,
                facts,
                keys: &self.registry,
                trusted: self.trusted,
            };
            let checked = rule.check(&ctx, &config.options);
            for mut d in unfinished(checked, &meta.id, None, incomplete)? {
                d.severity = level;
                found.push(d);
            }
        }
        Ok(found)
    }

    fn preset_rules(&self, id: &str) -> Option<Rules> {
        self.registry.preset_rules(id)
    }
}

/// The content hash the model records for a file's text.
pub fn hash_of(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The rules the configuration enables for at least one file: the presets it
/// extends and the entries it sets, resolved over the registry.
pub fn active_rules(registry: &Registry, config: &Config) -> Result<BTreeSet<String>, Error> {
    let mut active: BTreeSet<String> = config
        .resolve(Path::new(""), "", &|id| registry.preset_rules(id))?
        .into_iter()
        .filter_map(|(id, c)| c.level.map(|_| id))
        .collect();
    let overridden = config.configured().filter(|(_, c)| c.level.is_some());
    active.extend(overridden.map(|(id, _)| id.to_owned()));
    Ok(active)
}

/// What a rule found, or nothing and a gap when it could not finish: the gap
/// is recorded for the file (or the project), so the run is incomplete rather
/// than clean. Any other failure ends the run.
fn unfinished(
    checked: Result<Vec<Diagnostic>, lighthouse_plugin::Error>,
    rule: &str,
    file: Option<&Path>,
    incomplete: &mut Vec<Incomplete>,
) -> Result<Vec<Diagnostic>, Error> {
    match checked {
        Err(lighthouse_plugin::Error::Incomplete(reason)) => {
            incomplete.push(Incomplete {
                path: file.map(Path::to_owned),
                reason: format!("{rule}: {reason}"),
            });
            Ok(Vec::new())
        }
        other => Ok(other?),
    }
}

/// Rejects a configuration that names plugins, presets or rules the registry
/// lacks, lists them inconsistently, or gives a rule invalid options.
fn validate_config(registry: &Registry, config: &Config) -> Result<(), Error> {
    for plugin in config.plugins() {
        if !registry.has_plugin(&plugin.id) {
            return Err(Error::UnknownPlugin(plugin.id.clone()));
        }
    }
    registry.validate()?;
    let listed = |id: &str| config.lists(lighthouse_plugin::plugin_of(id));
    let mut entries: Vec<_> = config
        .configured()
        .map(|(id, c)| (id.to_owned(), c.options.clone()))
        .collect();
    for preset in config.extends() {
        let preset = registry
            .preset(preset)
            .ok_or_else(|| lighthouse_config::Error::UnknownPreset(preset.clone()))?;
        if !listed(&preset.id) {
            return Err(Error::PluginNotListed(preset.id.clone()));
        }
        let rules = registry
            .preset_rules(&preset.id)
            .ok_or_else(|| lighthouse_config::Error::PresetCycle(preset.id.clone()))?;
        entries.extend(rules.into_iter().map(|(id, c)| (id, c.options)));
    }
    for (id, options) in &entries {
        let rule = registry
            .rule(id)
            .ok_or_else(|| Error::UnknownRule(id.clone()))?;
        if !listed(id) {
            return Err(Error::PluginNotListed(id.clone()));
        }
        rule.validate(options)?;
    }
    Ok(())
}

/// The language providers of the registry, and which of them listed plugins enable.
fn load_languages(
    registry: &Registry,
    config: &Config,
) -> Result<(Vec<usize>, Vec<Language>), Error> {
    let mut providers = Vec::new();
    let mut languages = Vec::new();
    for (plugin, provider) in registry.languages() {
        if config.lists(plugin) {
            providers.push(languages.len());
        }
        languages.push(Language {
            files: glob_set(&provider.manifest().globs)?,
            tests: glob_set(&provider.manifest().conventions.test_globs)?,
        });
    }
    Ok((providers, languages))
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error {
    |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}
