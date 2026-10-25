use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use ignore::WalkBuilder;
use lighthouse_model::RunScope;
use lighthouse_model::{
    Diagnostic, Fingerprint, Incomplete, Options, Severity, Suppressed,
    ucm::{Applicability, File, Fragment, Project},
};
use lighthouse_plugin::{
    Context, EmbeddedFile, Facts, Indexed, LanguageProvider, Memo, Notices, Registry, Rule, Source,
    Workspace,
};
use lighthouse_spec::{
    Attributes, Catalog, Config, Error as SpecError, GeneratedCheck, GlobSet,
    Models as BoundModels, Projects, Routing, RuleConfig, Subject as DecisionSubject,
};
use rayon::prelude::*;
use serde_json::Value;
use thiserror::Error;

use crate::{
    annotations,
    cache::{Cache, Error as CacheError},
    documents::Documents,
    facts::FindingFacts,
    timings::Timings,
};

mod availability;
mod batch;
mod cached;
mod client;
mod config;
mod models;
mod phases;
mod route;
mod run;
mod scopes;
mod sources;

pub use availability::{Availability, Needs};
use cached::RunCache;
pub use client::CommandModels;
pub use config::active;
use config::{
    Inherited, constructors, inheritance, io_error, load_languages, options_at, test_naming,
    validate_config,
};
use models::RunModels;
pub(crate) use route::{Member, Refusal, Route, Usage};
pub(crate) use scopes::Scopes;

/// Name of the file, in `.gitignore` syntax, that keeps files out of the
/// analysis altogether, such as fixtures that are broken on purpose.
pub const IGNORE_FILE: &str = ".lighthouseignore";

/// Process exit code of a run whose analysis was incomplete.
pub const EXIT_INCOMPLETE: u8 = 3;

/// A model that fails this many requests is skipped for the rest of the run.
const MAX_FAILURES: usize = 3;

/// Why a run could not start or finish: invalid configuration, unknown or
/// unlisted plugin or rule, or an unreadable path. Analysis gaps are not
/// errors; they are [`Run::incomplete`].
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Spec(#[from] SpecError),
    #[error(transparent)]
    Plugin(#[from] lighthouse_plugin::Error),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    #[error("unknown language `{0}` in `languages` configuration")]
    UnknownLanguage(String),
    #[error("unknown source provider `{0}` in sources[].language.provider")]
    UnknownSourceProvider(String),
    #[error("`{0}` belongs to a plugin not listed in `plugins`")]
    PluginNotListed(String),
    #[error("unknown model `{0}`: declare it under `[models.<name>]`")]
    UnknownModel(String),
    #[error("model `{name}`: {reason}")]
    Model { name: String, reason: String },
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
    pub model_calls: usize,
    pub model_cost: f64,
    /// Findings that source directives suppress, within the report scope.
    /// They are not in `diagnostics` and never fail the run.
    pub suppressed: Vec<Suppressed>,
    /// The analysis the findings came from; fixes read it.
    pub project: Project,
    /// Where the time of the run went.
    pub timings: Timings,
}

impl Run {
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
    /// The project's, with the texts the run stands in place of files.
    workspace: &'a Workspace,
    rules: &'a [&'a dyn Rule],
    inputs: &'a [Input],
    project: &'a Project,
    facts: &'a Facts,
    memo: &'a Memo,
    /// Where findings of an earlier run are looked up and kept, if the run has
    /// a cache.
    cache: Option<&'a RunCache<'a>>,
    /// The models the rules may ask.
    models: &'a dyn lighthouse_plugin::Models,
}

/// The rules that did not finish, by file (`None` for the project).
pub(crate) type Unsettled = BTreeSet<(String, Option<PathBuf>)>;

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
}

/// Where the engine keeps the findings of its runs, and how much.
struct CacheSettings {
    dir: PathBuf,
    limit: u64,
}

/// A configured set of plugins that analyzes one project root. Running it does
/// not modify the project or the engine.
pub struct Engine {
    pub(crate) registry: Registry,
    pub(crate) config: Config,
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
    /// The `linguist-generated` attributes of the project.
    attributes: Attributes,
    /// What assembling the engine found worth telling; every run reports it.
    notices: Vec<String>,
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
    cache: Option<CacheSettings>,
    /// What the run may do with models.
    models: ModelOptions,
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
        registry: Registry,
        config: Config,
        catalog: &Catalog,
        root: &Path,
    ) -> Result<Self, Error> {
        let projects = catalog.projects().clone();
        let binding = config.models(&projects)?;
        validate_config(&registry, &config, &binding, &projects)?;
        let (providers, languages) = load_languages(&registry, &config)?;
        let root = root.canonicalize().map_err(io_error(root))?;
        let (attributes, notices) = Attributes::new(&root);
        let ws = Workspace {
            root,
            source_globs: config
                .sources()
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
            cache_dir: None,
            sandbox: config.sandbox().clone(),
        };
        let enabled = active(&config, &projects)?;
        Ok(Self {
            registry,
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
            attributes,
            notices,
            ws,
            inherited: inheritance(catalog),
            providers,
            languages,
            active: enabled,
            startup: Vec::new(),
            trusted: false,
            commands: None,
            cache: None,
            models: ModelOptions::default(),
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

    /// Keeps what rules find in `dir`, at most `limit` bytes of it, so that a
    /// later run over code that did not change what a rule reads takes the
    /// findings instead of running the rule. The findings are the same either
    /// way. Providers may keep their own data under the directory too.
    pub fn with_cache(mut self, dir: PathBuf, limit: u64) -> Self {
        self.ws.cache_dir = Some(dir.clone());
        self.cache = Some(CacheSettings { dir, limit });
        self
    }

    /// Opens the result cache, if the engine keeps one.
    pub(crate) fn open_cache(&self) -> Option<Result<Cache, CacheError>> {
        let settings = self.cache.as_ref()?;
        Some(Cache::open(&settings.dir, settings.limit))
    }

    /// Sets what the run may do with models.
    pub fn with_models(mut self, models: ModelOptions) -> Self {
        self.models = models;
        self
    }

    /// Adds gaps found while assembling the plugins; every run reports them.
    pub fn with_incomplete(mut self, gaps: Vec<Incomplete>) -> Self {
        self.startup.extend(gaps);
        self
    }

    /// Checks the whole project, then reports diagnostics under `paths`.
    /// `only` restricts which rules run; empty means all enabled rules.
    pub fn check(&self, paths: &[PathBuf], only: &[String]) -> Result<Run, Error> {
        self.check_overlaid(paths, only, &Overlays::new())
    }

    /// Like `check`, but the project is read with `overlays` standing in for
    /// the files of the same path: nothing is read from or written to those
    /// files on disk. This is how a fix verifies an edit before writing it.
    pub fn check_overlaid(
        &self,
        paths: &[PathBuf],
        only: &[String],
        overlays: &Overlays,
    ) -> Result<Run, Error> {
        self.validate_only(only)?;
        let (scopes, gaps) = Scopes::new(paths, &self.ws.root)?;
        self.run(scopes, gaps, only, overlays)
    }

    /// Checks the whole project, then reports only the diagnostics in `files`,
    /// project-relative paths of changed files. Unlike `check`, no files means
    /// nothing is reported, not everything: analysis scope is unchanged, only
    /// the report is narrowed.
    pub fn check_files(&self, files: &[PathBuf], only: &[String]) -> Result<Run, Error> {
        self.validate_only(only)?;
        self.run(Scopes::files(files), Vec::new(), only, &Overlays::new())
    }
}

/// The starting position shared by ordered and round-robin model bindings.
fn rotation_start(routing: Routing, len: usize, turn: &AtomicUsize) -> usize {
    match (routing, len) {
        (Routing::RoundRobin, len) if len > 0 => turn.fetch_add(1, Ordering::SeqCst) % len,
        _ => 0,
    }
}
