//! The plugin SPI: the traits and manifests every plugin kind implements, and
//! the `Registry` that holds them. It knows no specs and no configuration.

mod analysis;
mod annotations;
mod cel;
mod error;
mod findings;
mod fix;
mod models;
mod notices;
mod queries;
mod registry;
mod report;

use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use lighthouse_model::{
    Diagnostic, Document, Fingerprint, GeneratedSpec, Incomplete, Options, RunScope, Severity,
    ucm::{Applicability, Capability, File, Fragment, Methods, Model as CodeModel},
};
use lighthouse_process::Policy;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;

pub use analysis::{ANALYSIS, AnalyzerContext, OverlaidOf, Runtime, TextOf};
pub use annotations::Annotations;
pub use cel::{
    CatalogFunction, CelCatalog, CelLibrary, CelLibraryManifest, Function, Overload, Precedence,
};
pub use error::Error;
pub use findings::{
    BATCH, BATCHED, Batched, COMMAND, FINDINGS, Findings, JUDGED, Judged, Judging, Key, PATHS,
    TOOL, judge, queries, run_batches, set_judged,
};
pub use fix::{FixDecision, FixRequest, Fixer, FixerManifest, ValuesRequest};
pub use lighthouse_protocol::{
    Conventions, EmbeddedRegion, Mapping, TestNaming, TextMapSegment, Units, UnitsBy,
};
pub use models::{
    Abstained, Answer, Answered, Ask, Capabilities, CreateFineTuningJobRequest, DATASETS_DIR,
    Decided, DecisionRef, EditRequest, Edited, Example, Explored, FAIL_AT, FineTuneJobError,
    FineTuneMethod, FineTuneSupervisedHyperparameters, FineTuneSupervisedMethod, FineTuningJob,
    FineTuningJobStatus, LineKeys, ListModelResponse, MODELS_DIR, Model, ModelClient, Models,
    Neighbor, NoModels, Question, Settle, State, Subject, Supports, SystemOneRequest,
    SystemOneResponse, Tool, ToolEvent, TrainingLine, artifact_path, dataset_path, fails,
    request_of,
};
pub use notices::Notices;
pub use queries::{NoQueries, Pulling, Queries, Reads};
pub use registry::{Language, Registry, plugin_of};
pub use report::{REPORT, RUN, Read, Reported, quiet_report, set_run, state_report};

/// Identity of a plugin: `id` prefixes every analyzer, rule and fixer
/// it contributes. Every plugin kind describes itself through a manifest, a
/// static value separate from the behavior of the kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
}

impl Workspace {
    /// A workspace at `root` with no language options.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            languages: BTreeMap::new(),
            source_globs: Vec::new(),
            constructors: BTreeMap::new(),
            test_naming: BTreeMap::new(),
            overlays: BTreeMap::new(),
            cel_libraries: Vec::new(),
            sandbox: Policy::default(),
            ignores: Vec::new(),
        }
    }

    /// The text of a file as the run sees it: the overlay, else the disk.
    pub fn read(&self, path: &Path) -> Result<Cow<'_, str>, Error> {
        if let Some(text) = self.overlays.get(path) {
            return Ok(Cow::Borrowed(text));
        }
        fs::read_to_string(self.root.join(path))
            .map(Cow::Owned)
            .map_err(|e| Error::Incomplete(format!("{}: {e}", path.display())))
    }

    /// The test naming of `language`; the default convention for a language
    /// that declares none.
    pub fn test_naming(&self, language: &str) -> TestNaming {
        self.test_naming.get(language).cloned().unwrap_or_default()
    }

    /// The constructor prefixes of `language`; none for an unknown language.
    pub fn constructor_prefixes(&self, language: &str) -> &[String] {
        self.constructors.get(language).map_or(&[], Vec::as_slice)
    }
}

/// Everything an analyzer or rule may read in one run.
pub struct Context<'a> {
    pub workspace: &'a Workspace,
    /// The code model as the graph holds it: what a rule reads of it is
    /// recorded as a dependency of its findings.
    pub model: &'a dyn CodeModel,
    /// The focused file and its text; `None` for project scope.
    pub file: Option<(&'a File, &'a str)>,
    pub facts: &'a Facts,
    /// The user trusts the project to run the commands it names. The user
    /// decides that, never the repository.
    pub trusted: bool,
    /// What the suppression directives of the files say, once the engine has
    /// judged them; read by the rules that run after suppression.
    pub annotations: &'a Annotations,
    /// Where the rule says what it left out or could not read; drained after
    /// each rule into the notices of the run.
    pub notices: &'a Notices,
    /// What code the rule being run may report on: its decision's scope as
    /// the project's configuration left it. The engine drops the files that
    /// are out before the rule runs; a rule drops the symbols that are.
    pub applies: Applicability,
    /// The batch the focused file is in, for a rule that
    /// [`batches`](Rule::batches); empty for another.
    pub batch: &'a Batch,
    /// The project's own documents, found by the engine for the rules that are
    /// about them.
    pub documents: &'a [Document],
    /// The models rules may put subjects to.
    pub models: &'a dyn Models,
    /// The graph the run's facts and indexes are pulled from.
    pub queries: &'a dyn Queries,
}

/// The files a program is run over together, and the configuration of the rule
/// that runs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    pub variant: String,
    pub files: Vec<String>,
}

impl Context<'_> {
    /// The analyzer's fact for the focused file, else its project fact.
    pub fn fact<T: DeserializeOwned>(&self, analyzer: &str) -> Result<T, Error> {
        let file = self.file.map(|(f, _)| f.path.to_string_lossy());
        let value = file
            .and_then(|key| self.facts.get(analyzer, &key))
            .or_else(|| self.facts.get(analyzer, ""))
            .ok_or_else(|| Error::MissingFact(analyzer.to_owned()))?;
        T::deserialize(value).map_err(|e| Error::BadFact {
            analyzer: analyzer.to_owned(),
            message: e.to_string(),
        })
    }
}

/// What the analyzers computed in one run, by analyzer id and scope key (the
/// file path, or empty for project scope).
#[derive(Debug, Clone, Default)]
pub struct Facts(BTreeMap<String, BTreeMap<String, Value>>);

impl Facts {
    /// No facts yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keeps `value` as the fact of `analyzer` at `key`.
    pub fn insert(&mut self, analyzer: &str, key: String, value: Value) {
        self.0
            .entry(analyzer.to_owned())
            .or_default()
            .insert(key, value);
    }

    /// The fact of `analyzer` at `key`.
    pub fn get(&self, analyzer: &str, key: &str) -> Option<&Value> {
        self.0.get(analyzer)?.get(key)
    }

    /// Every fact as `(analyzer, key, value)`, sorted.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.0.iter().flat_map(|(analyzer, keys)| {
            keys.iter()
                .map(move |(key, value)| (analyzer.as_str(), key.as_str(), value))
        })
    }
}

/// Where the checked project lives and how each language is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub root: PathBuf,
    /// Per-language options from the configuration, keyed by language id.
    pub languages: BTreeMap<String, Options>,
    /// File globs that require serialized ASTs from their owning provider.
    pub source_globs: Vec<String>,
    /// The name prefixes that make a function a constructor, by language id:
    /// what the provider declares, or the project's `constructorPrefixes`.
    pub constructors: BTreeMap<String, Vec<String>>,
    /// How tests are named after the symbols they own, by language id: what
    /// the provider declares, with the project's `testNaming` over it.
    pub test_naming: BTreeMap<String, TestNaming>,
    /// Text that stands in for the file of the same project-relative path:
    /// what a provider must read instead of the disk. Empty for a normal run;
    /// a fix run checks candidate edits this way before anything is written.
    pub overlays: BTreeMap<PathBuf, String>,
    /// CEL libraries registered by the host, as manifests for language providers.
    pub cel_libraries: Vec<CelLibraryManifest>,
    /// How the programs the project names run: the project's `[sandbox]`.
    pub sandbox: Policy,
    /// The `ignores` of the project, with those of its plugins first: what
    /// keeps files out of the project, for whoever lists its files again.
    pub ignores: Vec<String>,
}

/// What a language provider declares about itself: which files are its own and
/// what it guarantees about them. The `initialize` result of a plugin process
/// carries one per language, field for field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageManifest {
    /// The language id, such as `go`.
    pub id: String,
    /// The names the language goes by in a document, as GitHub Linguist's
    /// `aliases` are: what a fenced code block may call it (`golang`, `rs`).
    /// Matched without regard to case; no two providers of a registry have one
    /// alike.
    pub aliases: Vec<String>,
    pub globs: Vec<String>,
    /// Gitignore patterns (the syntax of a plugin's `ignores`) of the files the
    /// language does not claim although `globs` match them; other languages
    /// still see them.
    pub ignores: Vec<String>,
    pub conventions: Conventions,
    pub capabilities: Vec<Capability>,
    /// A fallback provider claims a file only when no other provider does.
    pub fallback: bool,
    /// Among regular providers the higher priority claims a file first.
    pub priority: i32,
    /// The language evaluates declarations in the order they are written, so
    /// that the host keeps load-time uses in order when it moves them.
    pub ordered_declarations: bool,
    /// How the language's files group into the units that are indexed and
    /// kept one by one.
    pub units: Units,
}

impl LanguageManifest {
    /// A regular provider of `id` for `globs`, with no conventions or
    /// capabilities; set those directly.
    pub fn new(id: impl Into<String>, globs: Vec<String>) -> Self {
        Self {
            id: id.into(),
            aliases: Vec::new(),
            globs,
            ignores: Vec::new(),
            conventions: Conventions::default(),
            capabilities: Vec::new(),
            fallback: false,
            priority: 0,
            ordered_declarations: false,
            units: Units::default(),
        }
    }
}

/// One physical host file and the regions selected for another provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedFile {
    pub file: File,
    pub text: String,
    pub regions: Vec<EmbeddedRegion>,
}

/// What a provider produced for a batch of units.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indexed {
    /// One per unit asked for.
    pub units: Vec<Fragments>,
    /// Things the user should see that do not make the analysis incomplete.
    pub notices: Vec<String>,
    /// Files or scopes that could not be analyzed.
    pub incomplete: Vec<Incomplete>,
    /// Where the provider's own time went, phase by phase.
    pub timings: Vec<(String, std::time::Duration)>,
}

/// What a provider produced for one unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fragments {
    pub id: String,
    pub fragments: Vec<Fragment>,
    /// What the unit's types provide and its interfaces require, for the host
    /// to settle across the units of the language.
    pub methods: Methods,
    /// Serialized ASTs and source mappings returned for common source selection.
    pub sources: Vec<lighthouse_protocol::Source>,
    /// A hash of everything the unit's files hold.
    pub body: String,
    /// A hash of what an importer can see of the unit, and of what it imports.
    pub api: String,
    /// The units of the project it imports, sorted.
    pub imports: Vec<String>,
    /// Files outside the project the answer depended on, by absolute path.
    pub reads: Vec<String>,
    /// Messages about the unit that leave its analysis complete.
    pub notices: Vec<String>,
    /// Files of the unit that could not be analyzed.
    pub incomplete: Vec<Incomplete>,
}

impl Fragments {
    /// The result of a unit that nothing imports: its `body` and `api` are a
    /// hash of its fragments.
    pub fn new(id: impl Into<String>, fragments: Vec<Fragment>) -> Self {
        let text = serde_json::to_string(&fragments).unwrap_or_default();
        let hash = lighthouse_protocol::hash::sha256(text);
        Self {
            id: id.into(),
            fragments,
            methods: Methods::default(),
            sources: Vec::new(),
            body: hash.clone(),
            api: hash,
            imports: Vec::new(),
            reads: Vec::new(),
            notices: Vec::new(),
            incomplete: Vec::new(),
        }
    }
}

/// A file handed to a provider with its text as read by the engine.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    pub file: &'a File,
    pub text: &'a str,
    /// Empty for a physical file; populated when the same index interface
    /// analyzes source regions selected from another provider's syntax tree.
    pub regions: &'a [EmbeddedRegion],
}

/// A unit to index: the files (or the host file whose regions) it holds, and
/// the `api` of what it imported when it was last indexed.
#[derive(Debug, Clone, Copy)]
pub struct Unit<'a> {
    pub id: &'a str,
    /// The host's key of the unit.
    pub key: &'a str,
    pub files: &'a [Source<'a>],
    pub imports: &'a BTreeMap<String, String>,
}

/// Turns the units of one language into UCM fragments. Implementations must
/// be thread-safe, and `index` must not read outside `units` and `workspace`.
pub trait LanguageProvider: Send + Sync {
    fn manifest(&self) -> &LanguageManifest;
    /// What besides its manifest decides what the provider answers: the
    /// digest of the binary of a plugin process, so that a rebuilt plugin is
    /// asked again. Empty for a provider that is part of the host, whose
    /// build the host's own identity covers.
    fn identity(&self) -> String {
        String::new()
    }
    /// Indexes the units in one call. `files` are all the files of the
    /// language that the project holds, sorted, for an analysis that reads
    /// beyond its unit. An `Err` means the whole batch failed; every file is
    /// then incomplete.
    fn index(
        &self,
        workspace: &Workspace,
        units: &[Unit],
        files: &[String],
    ) -> Result<Indexed, Error>;
}

/// Computes one fact, declared by `id`, from the project and the facts of its `requires`.
/// `run` must be deterministic for equal inputs; the engine runs it once per `scope`.
pub trait Analyzer: Send + Sync {
    fn manifest(&self) -> &AnalyzerManifest;
    fn run(&self, ctx: &AnalyzerContext) -> Result<Value, Error>;
}

/// What an analyzer declares: the fact it computes, what that fact is
/// computed from, and what it looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzerManifest {
    /// Fully qualified `plugin/name`.
    pub id: String,
    /// Analyzers whose facts this one reads.
    pub requires: Vec<String>,
    pub scope: RunScope,
}

/// Static description of a rule: its identity, default severity, scope and
/// the analyzers and provider capabilities it depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleManifest {
    /// Fully qualified `plugin/name`.
    pub id: String,
    /// The uid of the decision the rule enforces: what its findings'
    /// fingerprints are seeded with, so that renaming the decision moves none
    /// of them. `None` for a rule no decision describes.
    pub uid: Option<String>,
    pub severity: Severity,
    pub scope: RunScope,
    pub description: String,
    pub docs: String,
    pub analyzers: Vec<String>,
    pub capabilities: Vec<Capability>,
    /// What code the rule's subjects may be in, from its decision's scope.
    pub applicability: Applicability,
    /// Changes whenever the way the rule judges changes (its check, scope,
    /// severity and option defaults, as the decision wrote them), so that the
    /// findings of the old way are not used. `None` for a rule whose findings
    /// are no node of the graph and are made again every run, such as one that
    /// reads more than the code model.
    pub revision: Option<String>,
    /// The programs the findings come from, by name: their identity is an
    /// input of the findings.
    pub tools: Vec<String>,
}

impl RuleManifest {
    /// The fingerprint of a finding of this rule about `symbol_path` with
    /// `snippet`, seeded by the decision's uid; a rule without a uid is
    /// seeded by its id.
    pub fn fingerprint(&self, symbol_path: &str, snippet: &str) -> Fingerprint {
        Fingerprint::new(
            self.uid.as_deref().unwrap_or(&self.id),
            symbol_path,
            snippet,
        )
    }
}

/// A check that turns facts into diagnostics. `validate` rejects bad options
/// before any run; `check` must report only for the scope in `manifest()`.
pub trait Rule: Send + Sync {
    fn manifest(&self) -> &RuleManifest;
    fn validate(&self, options: &Options) -> Result<(), Error>;
    fn check(&self, ctx: &Context, options: &Options) -> Result<Vec<Diagnostic>, Error>;
    /// Whether the rule puts subjects to models (an `abstain` route). Its findings
    /// are a node of the graph only for a run that does not ask the models.
    fn reviews(&self) -> bool {
        false
    }
    /// Whether the rule runs one program over a batch of files, the files of
    /// the unit (or the directory) of the file it judges, and takes what the
    /// program says of the file from the answer for the batch ([`Rule::batch`]).
    fn batches(&self) -> bool {
        false
    }
    /// The findings in each of `files` that one run of the rule's program over
    /// them tells; a file left out is not answered, and the rule judges it by
    /// itself.
    fn batch(
        &self,
        _ctx: &Context,
        _options: &Options,
        _files: &[PathBuf],
    ) -> Result<BTreeMap<PathBuf, Vec<Diagnostic>>, Error> {
        Ok(BTreeMap::new())
    }
    /// Whether the rule reads the suppression directives of its files with
    /// what each suppressed (see [`Annotations`]). The engine runs such a rule
    /// once the other rules' findings are known, and its findings are not
    /// themselves suppressed.
    fn after_suppression(&self) -> bool {
        false
    }
    /// The sort of subject whose facts the rule's expressions read (`symbol`,
    /// `function` or `test`) and the fields they read of it; none for a rule
    /// that reads no fact of a subject.
    fn reads(&self) -> Option<(&'static str, Reads)> {
        None
    }
}

/// A bundle of language providers, analyzers, rules and fixers, all of whose
/// ids are qualified with `manifest().id`. Everything defaults to empty.
pub trait Plugin {
    fn manifest(&self) -> &PluginManifest;
    fn languages(&self) -> Vec<Box<dyn LanguageProvider>> {
        Vec::new()
    }
    fn analyzers(&self) -> Vec<Box<dyn Analyzer>> {
        Vec::new()
    }
    fn rules(&self) -> Vec<Box<dyn Rule>> {
        Vec::new()
    }
    /// The fixers of the plugin's decisions; see [`Fixer`].
    fn fixers(&self) -> Vec<Box<dyn Fixer>> {
        Vec::new()
    }
    /// The models of the plugin; each has the id of the plugin.
    fn models(&self) -> Vec<Box<dyn ModelClient>> {
        Vec::new()
    }
    /// The CEL libraries of the plugin; each id is qualified with the plugin's.
    fn cel_libraries(&self) -> Vec<Box<dyn CelLibrary>> {
        Vec::new()
    }
    /// The files its toolchain leaves out of a project (`testdata/`,
    /// `node_modules/`), as the project's `ignores` are written.
    fn ignores(&self) -> Vec<String> {
        Vec::new()
    }
    /// What the facts of rules are pulled with, if the plugin owns them.
    fn pulling(&self) -> Option<Arc<dyn Pulling>> {
        None
    }
    /// The files and the head-of-file markers that tell the generated code of
    /// its ecosystem apart, as the project's `generated` is written.
    fn generated(&self) -> GeneratedSpec {
        GeneratedSpec::default()
    }
}

/// What the rules read of the facts of each sort of subject, merged.
pub fn reads_of<'a>(rules: impl IntoIterator<Item = &'a dyn Rule>) -> BTreeMap<String, Reads> {
    let mut found: BTreeMap<String, Reads> = BTreeMap::new();
    for (subject, reads) in rules.into_iter().filter_map(Rule::reads) {
        found.entry(subject.to_owned()).or_default().merge(&reads);
    }
    found
}

/// Deserializes rule options; unknown keys are rejected when `T` denies them.
pub fn options<T: DeserializeOwned>(rule: &str, options: &Options) -> Result<T, Error> {
    serde_json::from_value(Value::Object(options.clone())).map_err(|e| Error::Options {
        rule: rule.to_owned(),
        message: e.to_string(),
    })
}
