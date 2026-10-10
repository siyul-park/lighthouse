use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use ignore::WalkBuilder;
use lighthouse_model::RunScope;
use lighthouse_model::{
    Applicability, Diagnostic, File, Fingerprint, Fragment, Incomplete, Options, Project, Severity,
    hash,
};
use lighthouse_plugin::{
    Ctx, Facts, Indexed, LanguageProvider, Memo, Registry, Rule, RuleManifest, Source, Workspace,
};
use lighthouse_spec::{
    Catalog, Config, Domain, GeneratedCheck, GlobSet, ProjectError, Projects, RuleConfig, Rules,
    glob_set,
};
use rayon::prelude::*;
use serde_json::Value;
use thiserror::Error;

use crate::{
    aliases,
    annotations::{self, Allowed},
    documents,
    generated::Attributes,
    identity,
    subject::Subjects,
    timings::{Timings, in_pool},
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
    Project(#[from] ProjectError),
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
    /// Where the time of the run went.
    pub timings: Timings,
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

/// What the rules of a run read: the selected rules and everything they may
/// look at.
struct Scene<'a> {
    rules: &'a [&'a dyn Rule],
    inputs: &'a [Input],
    project: &'a Project,
    facts: &'a Facts,
    memo: &'a Memo,
}

/// What some rule runs found and noted.
#[derive(Default)]
struct Gathered {
    found: Vec<Diagnostic>,
    notices: BTreeSet<String>,
    incomplete: Vec<Incomplete>,
    /// The time spent in each rule.
    spent: BTreeMap<String, Duration>,
}

impl Gathered {
    /// Adds what `later` gathered after this.
    fn merge(&mut self, later: Self) {
        self.found.extend(later.found);
        self.notices.extend(later.notices);
        self.incomplete.extend(later.incomplete);
        for (rule, time) in later.spent {
            *self.spent.entry(rule).or_default() += time;
        }
    }
}

/// A configured set of plugins that analyzes one project root. Running it does
/// not modify the project or the engine.
pub struct Engine {
    pub(crate) registry: Registry,
    pub(crate) config: Config,
    /// The projects `extends` can name.
    projects: Projects,
    /// Rules whose decision is not in force (proposed, rejected, deprecated
    /// or superseded): they run only when a run selects them by id, such as
    /// `decision test`; configuration and projects never enable them.
    unenforced: BTreeSet<String>,
    /// Rules whose decision is about the project's own documents (the `spec`
    /// domain): a run that selects one finds those documents first.
    documented: BTreeSet<String>,
    /// Ids decisions were renamed from, with their current ids.
    aliases: BTreeMap<String, String>,
    /// The `linguist-generated` attributes of the project.
    attributes: Attributes,
    /// What assembling the engine found worth telling; every run reports it.
    notices: Vec<String>,
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
    /// `catalog` supplies the projects `extends` names and which decisions are
    /// in force; a rule no decision of it describes is in force.
    pub fn new(
        registry: Registry,
        config: Config,
        catalog: &Catalog,
        root: &Path,
    ) -> Result<Self, Error> {
        let aliases = catalog.aliases();
        let mut config = config;
        let mut projects = catalog.projects()?;
        let mut notices = aliases::apply(&mut config, &mut projects, &aliases);
        validate_config(&registry, &config, &projects)?;
        let (providers, languages) = load_languages(&registry, &config)?;
        let root = root.canonicalize().map_err(io_error(root))?;
        let (attributes, found) = Attributes::of(&root);
        notices.extend(found);
        let ws = Workspace {
            root,
            languages: config.languages().clone(),
            overlays: BTreeMap::new(),
        };
        let mut engine = Self {
            registry,
            config,
            projects,
            unenforced: catalog
                .decisions()
                .filter(|d| !d.enforced())
                .map(|d| d.id().to_owned())
                .collect(),
            documented: catalog
                .decisions()
                .filter(|d| d.scope.domain == Domain::Spec)
                .map(|d| d.id().to_owned())
                .collect(),
            aliases,
            attributes,
            notices,
            ws,
            providers,
            languages,
            active: BTreeSet::new(),
            startup: Vec::new(),
            trusted: false,
        };
        engine.active = active_rules(&engine.config, &engine.projects)?;
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
        let only_before = only;
        let only: Vec<String> = only
            .iter()
            .map(|id| self.aliases.get(id).unwrap_or(id).clone())
            .collect();
        let only = &only[..];
        let mut asked_by_old_id: Vec<String> = Vec::new();
        for (asked, now) in only_before.iter().zip(only) {
            if asked != now {
                asked_by_old_id.push(format!(
                    "decision `{asked}` is now `{now}`; the old id still works"
                ));
            }
        }
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
            .filter(|rule| {
                let id = &rule.manifest().id;
                !self.unenforced.contains(id) || only.contains(id)
            })
            .collect();

        let mut outcome = Outcome::default();
        outcome.notices.extend(self.notices.iter().cloned());
        outcome.notices.extend(asked_by_old_id);
        let mut timings = Timings::default();
        let mut incomplete = self.startup.clone();
        let scopes = match reported {
            Reported::Paths(paths) => self.scopes(paths, &mut incomplete)?,
            Reported::Files(files) => files.to_vec(),
        };
        let started = Instant::now();
        let inputs = self.read(overlays, &mut outcome.notices, &mut incomplete);
        timings.read = started.elapsed();
        let (inputs, project) = self.build_project(
            inputs,
            overlays,
            (&mut outcome.notices, &mut incomplete),
            &mut timings,
        );
        let project = if selected
            .iter()
            .any(|rule| self.documented.contains(&rule.manifest().id))
        {
            let found = documents::find(&self.ws.root, overlays);
            outcome.notices.extend(found.notices);
            project.with_documents(found.documents)
        } else {
            project
        };
        let memo = Memo::default();
        let facts = self.analyze(&selected, &inputs, &project, &memo, &mut timings)?;
        let started = Instant::now();
        let scene = Scene {
            rules: &selected,
            inputs: &inputs,
            project: &project,
            facts: &facts,
            memo: &memo,
        };
        let applied = self.apply(&scene)?;
        timings.rules_wall = started.elapsed();
        timings.add_rules(applied.spent);
        outcome.notices.extend(applied.notices);
        incomplete.extend(applied.incomplete);
        let found = applied.found;

        let started = Instant::now();

        let ran: BTreeSet<String> = selected.iter().map(|r| r.manifest().id.clone()).collect();
        let level = |rule: &str, file: &Path, lang: &str| self.level_at(rule, file, lang);
        let manifests: BTreeMap<String, &RuleManifest> = selected
            .iter()
            .map(|r| (r.manifest().id.clone(), r.manifest()))
            .collect();
        let texts: BTreeMap<&Path, &str> = inputs
            .iter()
            .map(|i| (i.file.path.as_path(), i.text.as_str()))
            .collect();
        let text = |path: &Path| texts.get(path).copied();
        let gate = annotations::Gate {
            level: &level,
            active: &self.active,
            selected: &ran,
            manifests: &manifests,
            aliases: &self.aliases,
            text: &text,
        };
        let applied = annotations::apply(found, &project, &gate)?;
        outcome.notices.extend(applied.notices);
        let (mut found, allowed) = (applied.kept, applied.allowed);
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
        timings.identity = started.elapsed();
        outcome.timings = timings;
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
    fn level_at(
        &self,
        rule: &str,
        file: &Path,
        lang: &str,
    ) -> Result<Option<Severity>, ProjectError> {
        let rules = self.config.resolve(file, lang, &self.projects)?;
        Ok(rules.get(rule).and_then(|config| config.level))
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
                .is_some_and(|r| r.manifest().scope == RunScope::Project);
            let (path, lang) = match project.file(&d.file) {
                Some(file) if !project_scope => (file.path.clone(), file.lang.clone()),
                _ => (PathBuf::new(), String::new()),
            };
            let rules = match resolved.entry((path, lang)) {
                std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
                std::collections::btree_map::Entry::Vacant(e) => {
                    let (path, lang) = e.key().clone();
                    let rules = self.config.resolve(&path, &lang, &self.projects)?;
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
                    input.file.hash = hash::sha256(text);
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
            hash: hash::sha256(&text),
            generated: self.config.is_generated(rel) || self.attributes.is_generated(rel),
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

    /// Has each provider index its files in one batch, the providers side by
    /// side, and merges the fragments. Files a provider did not index drop
    /// out; the reason is recorded as incomplete.
    fn build_project(
        &self,
        inputs: Vec<Input>,
        overlays: &Overlays,
        (notices, incomplete): (&mut BTreeSet<String>, &mut Vec<Incomplete>),
        timings: &mut Timings,
    ) -> (Vec<Input>, Project) {
        let ws = Workspace {
            overlays: overlays.clone(),
            ..self.ws.clone()
        };
        let mut batches: BTreeMap<usize, Vec<&Input>> = BTreeMap::new();
        for input in &inputs {
            batches.entry(input.language).or_default().push(input);
        }
        let batches: Vec<(usize, Vec<&Input>)> = batches.into_iter().collect();
        let indexed: Vec<(Result<Indexed, lighthouse_plugin::Error>, Duration)> = in_pool(|| {
            batches
                .par_iter()
                .map(|(language, batch)| {
                    let sources: Vec<Source> = batch
                        .iter()
                        .map(|i| Source {
                            file: &i.file,
                            text: &i.text,
                        })
                        .collect();
                    let started = Instant::now();
                    let indexed = self.provider(*language).index(&ws, &sources);
                    (indexed, started.elapsed())
                })
                .collect()
        });
        let mut parts: Vec<Fragment> = Vec::new();
        for ((language, batch), (indexed, spent)) in batches.iter().zip(indexed) {
            let provider = self.provider(*language);
            timings.index.push((provider.manifest().id.clone(), spent));
            match indexed {
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
        let started = Instant::now();
        let project = Project::merge(parts);
        timings.merge = started.elapsed();
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
        memo: &Memo,
        timings: &mut Timings,
    ) -> Result<Facts, Error> {
        let wanted = rules
            .iter()
            .flat_map(|r| r.manifest().analyzers.iter().map(String::as_str));
        let mut facts = Facts::new();
        for analyzer in self.registry.order(wanted)? {
            let started = Instant::now();
            let runs: Vec<(Option<&Input>, String)> = match analyzer.manifest().scope {
                RunScope::Project => vec![(None, String::new())],
                RunScope::File => inputs
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
                    memo,
                    applies: Applicability::default(),
                };
                let fact = analyzer.run(&ctx)?;
                facts.insert((analyzer.manifest().id.clone(), key), fact);
            }
            timings
                .analyzers
                .push((analyzer.manifest().id.clone(), started.elapsed()));
        }
        Ok(facts)
    }

    /// Runs the rules over the files and the project, in parallel; what they
    /// found comes back in a fixed order: the findings of each file in file
    /// order, rule by rule, then those of the project rules in rule order.
    fn apply(&self, scene: &Scene) -> Result<Gathered, Error> {
        let (files, project) = in_pool(|| {
            rayon::join(
                || self.apply_file_rules(scene),
                || self.apply_project_rules(scene),
            )
        });
        let mut all = files?;
        all.merge(project?);
        Ok(all)
    }

    fn apply_file_rules(&self, scene: &Scene) -> Result<Gathered, Error> {
        let per_file: Vec<Result<Gathered, Error>> = scene
            .inputs
            .par_iter()
            .map(|input| self.apply_to_file(scene, input))
            .collect();
        let mut all = Gathered::default();
        for gathered in per_file {
            all.merge(gathered?);
        }
        Ok(all)
    }

    fn apply_to_file(&self, scene: &Scene, input: &Input) -> Result<Gathered, Error> {
        let mut gathered = Gathered::default();
        let resolved = self
            .config
            .resolve(&input.file.path, &input.file.lang, &self.projects)?;
        let provider = self.provider(input.language);
        // What the merged model says of the file: a provider may know it is
        // generated where the host, reading it, did not.
        let file = scene.project.file(&input.file.path).unwrap_or(&input.file);
        for rule in scene
            .rules
            .iter()
            .filter(|r| r.manifest().scope == RunScope::File)
        {
            let meta = rule.manifest();
            let Some(config) = resolved.get(&meta.id) else {
                continue;
            };
            let Some(level) = config.level else { continue };
            let applies = self.applies(meta.applicability, config);
            if applies.excludes(scene.project, file) {
                continue;
            }
            if let Some(missing) = meta
                .capabilities
                .iter()
                .find(|c| !provider.manifest().capabilities.contains(c))
            {
                gathered.notices.insert(format!(
                    "{}: skipped for language `{}`, missing capability {missing}",
                    meta.id, input.file.lang
                ));
                continue;
            }
            let ctx = Ctx {
                ws: &self.ws,
                project: scene.project,
                file: Some((file, input.text.as_str())),
                facts: scene.facts,
                keys: &self.registry,
                trusted: self.trusted,
                memo: scene.memo,
                applies,
            };
            let started = Instant::now();
            let checked = rule.check(&ctx, &config.options);
            *gathered.spent.entry(meta.id.clone()).or_default() += started.elapsed();
            let found = unfinished(
                checked,
                &meta.id,
                Some(&input.file.path),
                &mut gathered.incomplete,
            )?;
            gathered.found.extend(found.into_iter().map(|mut d| {
                d.severity = level;
                d
            }));
        }
        Ok(gathered)
    }

    fn apply_project_rules(&self, scene: &Scene) -> Result<Gathered, Error> {
        let languages: BTreeSet<usize> = scene.inputs.iter().map(|i| i.language).collect();
        let resolved = self.config.resolve(Path::new(""), "", &self.projects)?;
        let rules: Vec<&&dyn Rule> = scene
            .rules
            .iter()
            .filter(|r| r.manifest().scope == RunScope::Project)
            .collect();
        let per_rule: Vec<Result<Gathered, Error>> = rules
            .par_iter()
            .map(|rule| self.apply_project_rule(scene, **rule, &languages, &resolved))
            .collect();
        let mut all = Gathered::default();
        for gathered in per_rule {
            all.merge(gathered?);
        }
        Ok(all)
    }

    fn apply_project_rule(
        &self,
        scene: &Scene,
        rule: &dyn Rule,
        languages: &BTreeSet<usize>,
        resolved: &Rules,
    ) -> Result<Gathered, Error> {
        let mut gathered = Gathered::default();
        let meta = rule.manifest();
        let Some(config) = resolved.get(&meta.id) else {
            return Ok(gathered);
        };
        let Some(level) = config.level else {
            return Ok(gathered);
        };
        let missing = meta.capabilities.iter().find(|c| {
            languages
                .iter()
                .any(|&l| !self.provider(l).manifest().capabilities.contains(c))
        });
        if let Some(missing) = missing {
            gathered.notices.insert(format!(
                "{}: skipped, missing capability {missing}",
                meta.id
            ));
            return Ok(gathered);
        }
        let ctx = Ctx {
            ws: &self.ws,
            project: scene.project,
            file: None,
            facts: scene.facts,
            keys: &self.registry,
            trusted: self.trusted,
            memo: scene.memo,
            applies: self.applies(meta.applicability, config),
        };
        let started = Instant::now();
        let checked = rule.check(&ctx, &config.options);
        *gathered.spent.entry(meta.id.clone()).or_default() += started.elapsed();
        let found = unfinished(checked, &meta.id, None, &mut gathered.incomplete)?;
        gathered.found.extend(found.into_iter().map(|mut d| {
            d.severity = level;
            d
        }));
        Ok(gathered)
    }
}

impl Engine {
    /// What code a rule may report on at a file: the decision's scope, unless
    /// the project says otherwise for all its decisions (`generated.check`)
    /// or for this rule (`generated`).
    fn applies(&self, declared: Applicability, rule: &RuleConfig) -> Applicability {
        let project = self
            .config
            .generated_check()
            .map(|c| c == GeneratedCheck::Include);
        Applicability {
            generated: rule.generated.or(project).unwrap_or(declared.generated),
            ..declared
        }
    }
}

/// The rules the configuration enables for at least one file: the projects it
/// extends and the entries it sets, resolved over `projects`.
pub fn active_rules(config: &Config, projects: &Projects) -> Result<BTreeSet<String>, Error> {
    let mut active: BTreeSet<String> = config
        .resolve(Path::new(""), "", projects)?
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

/// Rejects a configuration that names plugins, projects or rules the registry
/// lacks, lists them inconsistently, or gives a rule invalid options.
fn validate_config(registry: &Registry, config: &Config, projects: &Projects) -> Result<(), Error> {
    for plugin in config.plugins() {
        if !registry.has_plugin(&plugin.id) {
            return Err(Error::UnknownPlugin(plugin.id.clone()));
        }
    }
    registry.validate()?;
    let listed = |id: &str| config.lists(lighthouse_plugin::plugin_of(id));
    let entries = config.layer().entries(projects)?;
    for (id, config) in entries {
        let rule = registry
            .rule(id)
            .ok_or_else(|| Error::UnknownRule(id.to_owned()))?;
        if !listed(id) {
            return Err(Error::PluginNotListed(id.to_owned()));
        }
        rule.validate(&config.options)?;
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
