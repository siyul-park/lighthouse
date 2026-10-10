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
    Suppressed, hash,
};
use lighthouse_plugin::{
    Ctx, Facts, Indexed, LanguageProvider, Memo, Notices, Registry, Rule, RuleManifest, Source,
    Workspace,
};
use lighthouse_spec::{
    Catalog, Config, Domain, GeneratedCheck, GlobSet, ProjectError, Projects, RuleConfig, Rules,
};
use rayon::prelude::*;
use serde_json::Value;
use thiserror::Error;

use crate::{
    annotations, documents,
    generated::Attributes,
    identity,
    subject::Subjects,
    timings::{Timings, in_pool},
};

mod config;
mod phases;
mod run;

pub use config::active_rules;
use config::{constructors, io_error, load_languages, validate_config};

/// Name of the file, in `.gitignore` syntax, that keeps files out of the
/// analysis altogether, such as fixtures that are broken on purpose.
pub const IGNORE_FILE: &str = ".lighthouseignore";

/// Where providers keep derived results, relative to the root.
const CACHE_DIR: &str = ".lighthouse/cache";

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
    /// Findings that source directives suppress, within the report scope.
    /// They are not in `diagnostics` and never fail the run.
    pub suppressed: Vec<Suppressed>,
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
        let projects = catalog.projects()?;
        let mut notices = Vec::new();
        validate_config(&registry, &config, &projects)?;
        let (providers, languages) = load_languages(&registry, &config)?;
        let root = root.canonicalize().map_err(io_error(root))?;
        let (attributes, found) = Attributes::of(&root);
        notices.extend(found);
        let ws = Workspace {
            cache_dir: Some(root.join(CACHE_DIR)),
            root,
            languages: config.languages().clone(),
            constructors: constructors(&registry, &config),
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

    /// Runs without a cache: the providers get no directory to keep results in,
    /// and nothing is written under the root.
    pub fn without_cache(mut self) -> Self {
        self.ws.cache_dir = None;
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
}
