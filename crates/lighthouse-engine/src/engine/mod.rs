use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    Error, FixPlan, FixReport, FixRun, annotations,
    cache::{Cache, Error as CacheError},
    documents::Documents,
    facts::FindingFacts,
    generated::Generated,
    timings::Timings,
};
use ignore::gitignore::Gitignore;
use lighthouse_graph::{Error as GraphError, Graph, Revision};
use lighthouse_model::RunScope;
use lighthouse_model::{
    Diagnostic, Fingerprint, Incomplete, Options, Severity, Suppressed,
    ucm::{Applicability, File, Project},
};
use lighthouse_plugin::{
    Annotations, EmbeddedFile, Facts, Findings, LanguageProvider, Registry, Rule, Source, Workspace,
};
use lighthouse_protocol::hash::Hasher;
use lighthouse_spec::{
    Catalog, Config, GlobSet, Held, Models as BoundModels, ProjectFiles, Projects, Routing,
    RuleConfig, Subject as DecisionSubject,
};
use rayon::prelude::*;
use serde_json::Value;

mod availability;
mod client;
mod config;
mod conflicts;
mod index;
mod injections;
mod judging;
mod models;
mod phases;
mod project;
mod regions;
mod report;
mod route;
mod run;
mod scopes;
mod stated;
mod text;

pub use availability::{Availability, Needs};
pub use client::CommandModels;
pub use config::active;
use config::{
    Inherited, constructors, inheritance, io_error, load_languages, options_at, test_naming,
    validate_config,
};
pub use conflicts::{Conflicts, Orders};
use models::RunModels;
pub(crate) use phases::in_pool;
pub(crate) use route::{Member, Refusal, Route, Usage};
pub(crate) use scopes::Scopes;
use stated::Stated;
pub(crate) use text::Text;

/// Process exit code of a run whose analysis was incomplete.
pub const EXIT_INCOMPLETE: u8 = 3;

/// A model that fails this many requests is skipped for the rest of the run.
const MAX_FAILURES: usize = 3;

/// A run and the project it read: what a fix works from. The project is
/// merged from the graph once the run is done, which a check that only
/// reports never does.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub run: Run,
    pub project: Project,
}

impl std::ops::Deref for Outcome {
    type Target = Run;

    fn deref(&self) -> &Run {
        &self.run
    }
}

impl std::ops::DerefMut for Outcome {
    fn deref_mut(&mut self) -> &mut Run {
        &mut self.run
    }
}

/// The result of a run: findings, notices and the parts that were not analyzed.
#[derive(Debug, Clone, Default)]
pub struct Run {
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
    /// The gaps of `incomplete` that the key of the run decides: those of the
    /// startup and those a clean answer of a unit gave. Any other gap (a crash,
    /// a timeout, a file the model does not state) is not a function of the key.
    pub keyed: Vec<Incomplete>,
    /// The report scope: project-relative paths (a file, or a directory
    /// prefix) whose findings `diagnostics` holds. The empty path is the
    /// whole project; no entries means nothing was in scope.
    pub reported: Vec<PathBuf>,
    /// The rules that ran; a finding of any other rule says nothing about
    /// the code in this run.
    pub ran: Vec<String>,
    /// What the analysis knew about each finding's subject (language, symbol
    /// shape, function summary, measures), by fingerprint.
    pub facts: BTreeMap<Fingerprint, Value>,
    /// The options the configuration set for each finding's rule, by
    /// fingerprint; options left to the decision's defaults are absent.
    pub options: BTreeMap<Fingerprint, Options>,
    /// Every rule the configuration enables for some file, whether or not it
    /// ran: a remembered finding of any other rule is no longer configured.
    pub active: Vec<String>,
    /// The subjects no check decided, within the report scope: a model
    /// abstained, was unsure, failed, was over budget, or none is bound.
    /// They are not findings and never change the exit code.
    pub reviews: Vec<Diagnostic>,
    /// With [`ModelOptions::collect`], the questions rules put to models, by
    /// fingerprint: the same payload a model is asked about or trained on.
    pub questions: BTreeMap<String, lighthouse_plugin::Subject>,
    /// The probability of failing that models gave each subject this run,
    /// by fingerprint, whether or not it reached the band.
    pub probabilities: BTreeMap<String, (f64, String)>,
    /// Requests put to models this run (answers from the cache are not
    /// counted), and what they cost.
    pub usage: lighthouse_model::Usage,
    /// Findings that source directives suppress, within the report scope.
    /// They are not in `diagnostics` and never fail the run.
    pub suppressed: Vec<Suppressed>,
    /// The SHA-256 of the text the run read of each file of the project, by
    /// path: what a fix compares the file with before it writes.
    pub read: BTreeMap<PathBuf, String>,
    /// Where the time of the run went.
    pub timings: Timings,
    /// The revision the graph was at when the project of the run was read: a
    /// node that changed after an earlier run's and by this one's moved
    /// between the two.
    pub revision: Revision,
}

impl Run {
    /// Whether a gap is one that another run of the same key may not have.
    pub(crate) fn has_unkeyed_gap(&self) -> bool {
        self.incomplete.iter().any(|gap| !self.keyed.contains(gap))
    }

    /// The findings of error or warn severity.
    pub(crate) fn loud(&self) -> impl Iterator<Item = &Diagnostic> {
        let loud = |d: &&Diagnostic| matches!(d.severity, Severity::Error | Severity::Warn);
        self.diagnostics.iter().filter(loud)
    }

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

/// What the rules of a run read: the selected rules and everything they may
/// look at.
struct Scene<'a> {
    /// The project's, with the texts the run stands in place of files.
    workspace: &'a Workspace,
    rules: &'a [&'a dyn Rule],
    inputs: &'a [Input],
    /// What the files state, and the documents.
    stated: &'a Stated,
    facts: &'a Facts,
    annotations: &'a Annotations,
    /// The models the rules may ask.
    models: &'a dyn lighthouse_plugin::Models,
    /// The graph the findings are pulled from.
    graph: &'a Graph,
}

pub(crate) struct Input {
    pub(crate) file: File,
    pub(crate) text: Text,
    /// The SHA-256 of the text, as the graph holds it.
    pub(crate) sha256: String,
    language: usize,
}

/// The findings of the rules, and the rules that did not finish.
type Collected = (Vec<Diagnostic>, Unsettled);

/// What some rule runs found and noted.
#[derive(Default)]
struct Gathered {
    found: Vec<Diagnostic>,
    notices: BTreeSet<String>,
    incomplete: Vec<Incomplete>,
    /// The time spent in each rule.
    times: BTreeMap<String, Duration>,
    /// The rules that did not finish where they were to judge, by file
    /// (`None` for the whole project): incomplete, or skipped for a missing
    /// capability. A directive for one of them is not known to be stale.
    unsettled: Unsettled,
}

impl Gathered {
    /// Adds what `later` gathered after this.
    fn merge(&mut self, later: Self) {
        self.found.extend(later.found);
        self.notices.extend(later.notices);
        self.incomplete.extend(later.incomplete);
        self.unsettled.extend(later.unsettled);
        for (rule, time) in later.times {
            *self.times.entry(rule).or_default() += time;
        }
    }

    /// Adds the findings a model answered.
    fn answer(&mut self, found: Vec<Diagnostic>) {
        self.found.extend(found);
    }

    /// Adds what the rule `id` found, noticed and could not finish in `file`
    /// (`None` for the project), at `level`.
    fn settle(
        &mut self,
        (id, level): (&str, Severity),
        file: Option<&Path>,
        result: Result<Arc<Findings>, GraphError>,
    ) -> Result<(), Error> {
        match result {
            Ok(findings) => {
                self.notices.extend(findings.notices.iter().cloned());
                let found = findings
                    .diagnostics()
                    .map_err(|e| Error::Graph(GraphError::Encode(e.to_string())))?;
                self.found
                    .extend(found.into_iter().map(|d| phases::leveled(d, level)));
                Ok(())
            }
            Err(GraphError::Incomplete(reason)) => {
                self.unsettled
                    .insert((id.to_owned(), file.map(Path::to_owned)));
                self.incomplete.push(Incomplete {
                    path: file.map(Path::to_owned),
                    provider: None,
                    reason: format!("{id}: {reason}"),
                });
                Ok(())
            }
            Err(e) => Err(Error::Graph(e)),
        }
    }
}

/// The rules that did not finish, by file (`None` for the project).
pub(crate) type Unsettled = BTreeSet<(String, Option<PathBuf>)>;

/// A configured set of plugins that analyzes one project root. Running it does
/// not modify the project or the engine.
pub struct Engine {
    pub(crate) registry: Arc<Registry>,
    pub(crate) config: Config,
    /// A digest of the configuration as the engine was built over it. Its
    /// compiled patterns show what they matched when printed, so it is taken
    /// once, before any run.
    setup: String,
    /// The projects `extends` can name.
    projects: Projects,
    /// The models the configuration binds to its decisions.
    binding: BoundModels,
    /// Rules whose decision is not in force (proposed, rejected, deprecated
    /// or superseded): they run only when a run selects them by id, such as
    /// `decision test`; configuration and projects never enable them.
    unenforced: BTreeSet<String>,
    /// Rules whose decision is about the project's own documents (the `spec`
    /// domain): a run that selects one finds those documents first.
    documented: BTreeSet<String>,
    /// The files of the project: what every walk of it lists.
    pub(crate) files: ProjectFiles,
    /// How generated code is told apart.
    generated: Generated,
    pub(crate) ws: Workspace,
    /// What the decisions that read the options of another read.
    inherited: BTreeMap<String, Inherited>,
    /// Providers from listed plugins; indexes into `languages`.
    providers: Vec<usize>,
    languages: Vec<Language>,
    /// Rules enabled by the configuration for at least one file.
    active: BTreeSet<String>,
    /// Gaps known before the run, such as a plugin that failed to start.
    startup: Vec<Incomplete>,
    /// The user trusts the project to run the commands its checks and fixers
    /// name.
    pub(crate) trusted: bool,
    /// How a `command` model provider is run, when the host knows.
    commands: Option<CommandModels>,
    /// The result cache; `None` runs every rule every time.
    pub(crate) cache: Option<CacheSettings>,
    /// The graph of what is known of the files, opened by the first run.
    pub(crate) graph: std::sync::Mutex<Option<Graph>>,
    /// What the last walk set in it, locked after it.
    pub(crate) held: std::sync::Mutex<Held>,
    /// What the run may do with models.
    models: ModelOptions,
    /// What the search for contradictions between decisions needs; without it
    /// a rule that reads `decision.conflicts` finds none.
    conflicts: Option<Conflicts>,
}

/// Where the engine keeps the findings of its runs, and how much.
pub(crate) struct CacheSettings {
    pub(crate) dir: PathBuf,
    pub(crate) limit: u64,
}

struct Language {
    files: GlobSet,
    /// What the language leaves to the others although `files` match it.
    ignored: Gitignore,
    tests: GlobSet,
}

impl Language {
    /// Whether the language claims the project-relative file at `rel`: its
    /// `files` match it and its own `ignores` do not.
    fn claims(&self, rel: &Path) -> bool {
        self.files.is_match(rel)
            && !self
                .ignored
                .matched_path_or_any_parents(rel, false)
                .is_ignore()
    }
}

/// Whether and how a run asks models.
#[derive(Debug, Clone, Default)]
pub struct ModelOptions {
    /// Models are asked at all. Off, the subjects they would judge are
    /// reported for review.
    pub off: bool,
    /// Ask models that cost about the whole project, not only the files the
    /// run reports.
    pub all: bool,
    /// Ask models only about the subjects with these fingerprints, wherever
    /// they are; the rest are left to review. Overrides `all` and the report
    /// scope for what models are asked.
    pub fingerprints: Option<BTreeSet<String>>,
    /// Keep the questions rules put to models in [`Run::questions`].
    pub collect: bool,
    /// The model a revision attached to a decision, by decision id. A run
    /// without them is the check without the layer a revision adds, which is
    /// what grading that layer starts from.
    pub attached: BTreeMap<String, String>,
}

impl Engine {
    /// `root` is the directory of `lighthouse.toml`; globs match paths relative to it.
    /// `catalog` supplies the projects `extends` names and which decisions are
    /// in force; a rule no decision of it describes is in force.
    pub fn new(
        registry: impl Into<Arc<Registry>>,
        config: Config,
        catalog: &Catalog,
        root: &Path,
    ) -> Result<Self, Error> {
        let registry: Arc<Registry> = registry.into();
        let projects = catalog.projects().clone();
        let config = config.inheriting(&projects)?;
        let binding = config.models(&projects)?;
        validate_config(&registry, &config, &binding, &projects)?;
        let root = root.canonicalize().map_err(io_error(root))?;
        let (providers, languages) = load_languages(&registry, &config, &root)?;
        let ignores = ignores(&registry, &config);
        let files = ProjectFiles::new(&root, &ignores)?;
        let generated = Generated::new(&root, &registry, &config)?;
        let ws = Workspace {
            root,
            source_globs: config
                .injections()
                .iter()
                .flat_map(|source| source.files.iter().cloned())
                .collect(),
            languages: config
                .languages()
                .map(|(id, options)| (id.to_owned(), options.clone()))
                .collect(),
            constructors: constructors(&registry, &config),
            test_naming: test_naming(&registry, &config),
            overlays: BTreeMap::new(),
            cel_libraries: registry
                .cel()
                .libraries()
                .map(|library| library.manifest().clone())
                .collect(),
            sandbox: config.sandbox().clone(),
            ignores,
        };
        let enabled = active(&config, &projects)?;
        let mut setup = Hasher::new();
        setup.update(format!("{config:?}"));
        Ok(Self {
            registry,
            setup: setup.hex(),
            config,
            projects,
            binding,
            unenforced: catalog
                .decisions()
                .filter(|d| !d.enforced())
                .map(|d| d.id().to_owned())
                .collect(),
            documented: catalog
                .decisions()
                .filter(|d| d.scope.subject == DecisionSubject::Decision)
                .map(|d| d.id().to_owned())
                .collect(),
            files,
            generated,
            ws,
            inherited: inheritance(catalog),
            providers,
            languages,
            active: enabled,
            startup: Vec::new(),
            trusted: false,
            commands: None,
            cache: None,
            graph: std::sync::Mutex::new(None),
            held: std::sync::Mutex::new(Held::default()),
            models: ModelOptions::default(),
            conflicts: None,
        })
    }

    /// Whether the configuration enables the rule `id`.
    pub fn active(&self, id: &str) -> bool {
        self.active.contains(id)
    }

    /// Says whether the user trusts the project to run the commands its checks name.
    pub fn with_trust(mut self, trusted: bool) -> Self {
        self.trusted = trusted;
        self
    }

    /// Tells the engine how to run the `command` model providers of a trusted
    /// project: `commands` builds the client of one.
    pub fn with_commands(mut self, commands: CommandModels) -> Self {
        self.commands = Some(commands);
        self
    }

    /// Lets a run search for contradictions between the decisions of the
    /// project, for the rules that read them (see [`Conflicts`]).
    pub fn with_conflicts(mut self, conflicts: Conflicts) -> Self {
        self.conflicts = Some(conflicts);
        self
    }

    /// Keeps what rules find in `dir`, and the rest of the cache, at most
    /// `limit` bytes of it all, so that a later run over code that did not
    /// change what a rule reads takes the findings instead of running the
    /// rule. The findings are the same either way.
    pub fn with_cache(mut self, dir: PathBuf, limit: u64) -> Self {
        self.cache = Some(CacheSettings { dir, limit });
        self
    }

    /// Opens the result cache, if the engine keeps one.
    pub(crate) fn open_cache(&self) -> Option<Result<Cache, CacheError>> {
        let settings = self.cache.as_ref()?;
        Some(Cache::open(&settings.dir))
    }

    /// Sets what the run may do with models.
    pub fn with_models(mut self, models: ModelOptions) -> Self {
        self.set_models(models);
        self
    }

    /// Sets what the next run may do with models: an engine that serves one
    /// request after another takes the options of each.
    pub fn set_models(&mut self, models: ModelOptions) {
        self.models = models;
    }

    /// Adds gaps found while assembling the plugins; every run reports them.
    pub fn with_incomplete(mut self, gaps: Vec<Incomplete>) -> Self {
        self.startup.extend(gaps);
        self
    }

    /// Fixes the selected findings of the project: see the module docs. The
    /// analysis must be complete, because a fix that rests on a partial
    /// analysis is a guess.
    pub fn fix(&self, plan: &FixPlan, run: &FixRun) -> Result<FixReport, Error> {
        crate::fix::fix(self, plan, run)
    }

    /// Checks the whole project, then reports diagnostics under `paths`.
    /// `only` restricts which rules run; empty means all enabled rules.
    pub fn check(&self, paths: &[PathBuf], only: &[String]) -> Result<Run, Error> {
        self.validate_only(only)?;
        let (scopes, gaps) = Scopes::new(paths, &self.ws.root)?;
        self.finished(self.run(scopes, gaps, only, (&Overlays::new(), true)))
    }

    /// Like `check`, but the run is never answered from the last report, and
    /// the project that was read comes with it, as a fix needs it.
    pub fn check_project(&self, paths: &[PathBuf], only: &[String]) -> Result<Outcome, Error> {
        self.validate_only(only)?;
        let (scopes, gaps) = Scopes::new(paths, &self.ws.root)?;
        let run = self.finished(self.run(scopes, gaps, only, (&Overlays::new(), false)))?;
        self.outcome(run)
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
        self.validate_only(only)?;
        let (scopes, gaps) = Scopes::new(paths, &self.ws.root)?;
        let run = self.finished(self.run(scopes, gaps, only, (overlays, false)))?;
        self.outcome(run)
    }

    /// Checks the whole project, then reports only the diagnostics in `files`,
    /// project-relative paths of changed files. Unlike `check`, no files means
    /// nothing is reported, not everything: analysis scope is unchanged, only
    /// the report is narrowed.
    pub fn check_files(&self, files: &[PathBuf], only: &[String]) -> Result<Run, Error> {
        self.validate_only(only)?;
        self.finished(self.run(
            Scopes::files(files),
            Vec::new(),
            only,
            (&Overlays::new(), true),
        ))
    }
}

/// The `ignores` that apply to the project: those of the listed plugins in the
/// order they were registered, then the project's, in which the extended
/// projects' come first.
fn ignores(registry: &Registry, config: &Config) -> Vec<String> {
    registry
        .ignores()
        .filter(|(id, _)| config.lists(id))
        .flat_map(|(_, patterns)| patterns.iter().cloned())
        .chain(config.ignores().iter().cloned())
        .collect()
}

/// The starting position shared by ordered and round-robin model bindings.
fn rotation_start(routing: Routing, len: usize, turn: &AtomicUsize) -> usize {
    match (routing, len) {
        (Routing::RoundRobin, len) if len > 0 => turn.fetch_add(1, Ordering::SeqCst) % len,
        _ => 0,
    }
}
