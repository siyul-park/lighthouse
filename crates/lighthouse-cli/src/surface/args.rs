//! The arguments of the commands an agent also uses. One struct per command
//! serves the command line (clap), the MCP tool (serde, JSON Schema) and the
//! docs, so that a flag and its tool argument cannot differ: a tool argument
//! is the long flag in camelCase, with the same values and default.

use std::path::{Path, PathBuf};

use clap::{Args, ValueEnum};
use lighthouse_session::{
    Judgment, head,
    model::{Labelling, Repo},
    review::NewJudgment,
};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::surface::{Context, Result};

impl From<JudgmentArg> for Judgment {
    fn from(judgment: JudgmentArg) -> Self {
        match judgment {
            JudgmentArg::Pass => Self::Pass,
            JudgmentArg::Fail => Self::Fail,
            JudgmentArg::NotApplicable => Self::NotApplicable,
        }
    }
}

/// `check`: analyze the project and report. Analysis is always whole-project;
/// the arguments narrow what is reported and how.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckArgs {
    /// Where to report findings; the whole project is analyzed anyway.
    /// Default: the whole project.
    pub paths: Vec<PathBuf>,
    /// Report only files changed in the working tree against HEAD (modified,
    /// added, renamed, untracked). A report filter like PATHS.
    #[arg(long, conflicts_with = "diff")]
    pub changed: bool,
    /// Report only files changed since the merge base of BASE and HEAD,
    /// including the working tree and untracked files. A report filter.
    #[arg(long, value_name = "BASE")]
    pub diff: Option<String>,
    /// Run only these decisions: ids, comma-separated or repeated, and globs
    /// such as `design/*`.
    #[arg(long, value_delimiter = ',', value_name = "ID")]
    pub decision: Vec<String>,
    /// Report at most this many findings, errors first, and say how many were
    /// left out. Default: 25 with `--format json`, all otherwise.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// With `--format json`: compact groups findings by decision and states a
    /// decision's text once; full gives one self-contained record per finding.
    #[arg(long, value_enum, default_value_t)]
    pub detail: Detail,
    /// Do not record this run in `.lighthouse/lighthouse.db` and do not apply
    /// judgments. By default a run is recorded: findings it no longer reports
    /// are marked resolved (never when the analysis was incomplete), and
    /// findings whose latest judgment hides them stay out of the report.
    #[arg(long)]
    pub no_store: bool,
    /// Run every rule instead of reusing the findings of earlier runs kept in
    /// `.lighthouse/cache`. The findings and the exit code are the same.
    #[arg(long)]
    pub no_cache: bool,
    /// Ask no model: what they would judge is left to review.
    #[arg(long)]
    pub no_models: bool,
    /// Ask models that cost about the whole project, not only the files the
    /// run reports.
    #[arg(long)]
    pub all_models: bool,
    /// Record what models fail as judgments attributed to them, so that
    /// training can reproduce them from `decisions.jsonl`.
    #[arg(long)]
    pub record_model_labels: bool,
    /// Fail on warnings too.
    #[arg(long)]
    pub strict: bool,
    /// Fail when there are more than N warnings (errors always fail).
    #[arg(long, value_name = "N")]
    pub max_warnings: Option<usize>,
    /// Exit by findings even when the analysis was incomplete; the
    /// incompleteness is still reported.
    #[arg(long)]
    pub allow_incomplete: bool,
}

/// How much each item of a report carries.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Detail {
    /// Grouped by decision, a decision's text said once.
    #[default]
    Compact,
    /// One self-contained record per item.
    Full,
}

impl From<Detail> for lighthouse_report::Detail {
    fn from(detail: Detail) -> Self {
        match detail {
            Detail::Compact => Self::Compact,
            Detail::Full => Self::Full,
        }
    }
}

/// `fix`, the tool of `check --fix`: which findings, and how to fix them.
/// Name at least one of `paths`, `decision` and `fingerprints`: a fix through
/// MCP never defaults to the whole project.
#[derive(Debug, Default, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct FixArgs {
    /// Fix findings under these paths.
    pub paths: Vec<PathBuf>,
    /// Fix the findings of these decisions: ids or globs.
    pub decision: Vec<String>,
    /// Fix these findings: fingerprints or unique prefixes of at least 7
    /// characters.
    pub fingerprints: Vec<String>,
    /// Ask no model: what they would fix is left to review.
    pub no_models: bool,
    #[serde(flatten)]
    pub options: FixOptions,
}

/// How `check --fix` fixes (the `fix` tool takes the same).
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct FixOptions {
    /// Print the unified diff and leave every file as it was.
    #[arg(long, requires = "fix")]
    pub dry_run: bool,
    /// Write `unsafe` fixes, which are judgment calls, whatever makes them
    /// (`true`), or only preview them (`false`); unset, `[fix] unsafe`
    /// decides. A preview is applied by naming its finding.
    #[arg(skip)]
    pub unsafe_fixes: Option<bool>,
    /// Write `unsafe` fixes, as `unsafeFixes: true` does.
    #[arg(
        long = "unsafe-fixes",
        requires = "fix",
        overrides_with = "no_unsafe_fixes"
    )]
    #[serde(skip)]
    pub allow_unsafe: bool,
    /// Only preview `unsafe` fixes, whatever `[fix] unsafe` says.
    #[arg(long, requires = "fix", overrides_with = "allow_unsafe")]
    #[serde(skip)]
    pub no_unsafe_fixes: bool,
    /// Use this registered fixer for the selected findings instead of the one
    /// their decision names. Its fixes are `unsafe`.
    #[arg(long, requires = "fix", value_name = "ID")]
    pub fixer: Option<String>,
}

impl FixOptions {
    /// Whether the run writes `unsafe` fixes: as the flags or `unsafeFixes`
    /// say, `None` leaving it to the project.
    pub fn unsafe_permission(&self) -> Option<bool> {
        if self.no_unsafe_fixes {
            Some(false)
        } else if self.allow_unsafe {
            Some(true)
        } else {
            self.unsafe_fixes
        }
    }
}

/// `decision list`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct DecisionListArgs {
    /// Include every catalog decision with its implementation status.
    #[arg(long)]
    pub all: bool,
    /// List at most this many decisions and say how many were left out.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

/// `decision show`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionShowArgs {
    /// A decision or rule id such as `design/exported-doc`.
    #[arg(value_name = "ID")]
    pub id: String,
}

/// `decision signals`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct DecisionSignalsArgs {
    /// Include every signal: the settled ones, the prompts no rule decided
    /// and the findings of edits. By default, the open signals that state a
    /// decision.
    #[arg(long)]
    pub all: bool,
    /// List at most this many signals and say how many were left out.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

/// `decision signal dismiss`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignalDismissArgs {
    /// The signal, by id or by a unique prefix of at least 7 characters.
    #[arg(value_name = "ID")]
    pub id: String,
    /// Why the signal is not a decision. It labels the classifier's answer.
    #[arg(long, value_name = "TEXT")]
    pub reason: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub attributed: Attributed,
}

/// `decision create` and `decision update`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentArgs {
    /// A `Decision` document, as YAML or JSON. On the command line this is a
    /// file, or `-` for stdin; the tool takes the text or the object.
    /// `metadata.name` is the id (`local/<name>`); `spec.examples` needs at
    /// least one valid and one invalid example per language the project runs.
    #[arg(value_name = "FILE|-")]
    #[serde(deserialize_with = "text_or_object")]
    #[schemars(schema_with = "document_schema")]
    pub document: String,
}

/// `decision test`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct DecisionTestArgs {
    /// Decisions to test: ids and globs. Default: every checked decision.
    #[arg(value_name = "IDS", value_delimiter = ',')]
    pub decision: Vec<String>,
    /// Run the examples in this language only.
    #[arg(long)]
    pub language: Option<String>,
}

/// Who a record is attributed to, for the commands that write one.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct Attributed {
    /// Who this is attributed to: `person` or `agent`, with an id after a
    /// colon (`agent:claude`). Default: the person from `git config
    /// user.name` on the command line, the client's name through MCP.
    #[arg(long, value_name = "WHO", env = "LIGHTHOUSE_ATTRIBUTED_TO")]
    pub attributed_to: Option<String>,
}

/// `decision eval`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DecisionEvalArgs {
    /// The decision, such as `design/prefer-method`.
    #[arg(value_name = "ID")]
    pub id: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub labels: LabelSource,
    /// A decision file whose check is evaluated beside the current one.
    #[arg(long, value_name = "FILE", conflicts_with = "model")]
    pub check: Option<PathBuf>,
    /// A model to grade as the first layer of the decision, beside the
    /// current check: `<provider>/<model>@sha256:<hex>`. One fitted on labels
    /// is graded on those by refits that leave each fold out.
    #[arg(long, value_name = "REF")]
    pub model: Option<String>,
    /// Write this many selected subjects the check did not flag to a table
    /// for a person to label (`TP` or `FP`), to measure recall.
    #[arg(long, value_name = "N")]
    pub audit: Option<usize>,
}

/// Where the labels of a decision come from: the table of reviewed findings
/// and the repositories it is about. Flags and tool arguments of every command
/// that learns from labels.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct LabelSource {
    /// A table of reviewed findings (`fingerprint repo location verdict`).
    /// Default: `plugins/<pack>/labels/<name>.tsv` for the decision `<pack>/<name>`, when it exists.
    #[arg(long, value_name = "FILE")]
    pub labels: Option<PathBuf>,
    /// A repository the labels are about, `name=dir`, where `name` is the
    /// `repo` column of the table. Repeatable; default: this project.
    #[arg(long, value_name = "NAME=DIR")]
    pub repo: Vec<String>,
}

impl LabelSource {
    /// What the labels of `decision` are drawn from: the table, else
    /// `plugins/<pack>/labels/<name>.tsv` when `decision` is one id and the file exists, and
    /// the repositories, inside the project when the caller is confined.
    pub fn resolve(&self, ctx: &Context, decision: Option<&str>) -> Result<Labelling> {
        let root = ctx.root()?;
        let repos = self
            .repo
            .iter()
            .map(|spec| {
                let mut repo = Repo::parse(spec)?;
                repo.dir = ctx.paths(&root, &[repo.dir])?.remove(0);
                Ok(repo)
            })
            .collect::<Result<_>>()?;
        let table = self.labels.clone().or_else(|| {
            let (pack, name) = decision?.split_once('/')?;
            let path = Path::new("plugins")
                .join(pack)
                .join("labels")
                .join(format!("{name}.tsv"));
            path.is_file().then_some(path)
        });
        Ok(Labelling {
            repos,
            table,
            config: ctx.config.clone(),
        })
    }
}

/// `review list`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewListArgs {
    /// Only the findings of these decisions: ids or globs.
    #[arg(long, value_delimiter = ',', value_name = "ID")]
    pub decision: Vec<String>,
    /// Which findings: `open` is still reported and not suppressed.
    #[arg(long, value_enum, default_value_t)]
    pub status: StatusFilter,
    /// Also list findings that do not ask for review (errors, and judged
    /// ones).
    #[arg(long)]
    pub all: bool,
    /// List at most this many findings, errors first, and say how many were
    /// left out. Default: 50 with `--format json`, all otherwise.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
    /// With `--format json`: compact groups findings by decision; full gives
    /// one record per finding with its full fingerprint and `lastSeen`.
    #[arg(long, value_enum, default_value_t)]
    pub detail: Detail,
}

/// Which remembered findings a listing includes.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum StatusFilter {
    /// Still reported and not suppressed.
    #[default]
    Open,
    /// Kept out of reports by a judgment.
    Suppressed,
    /// Judged `notApplicable`, so the decision is too broad.
    Narrowing,
    /// Of a rule the configuration no longer enables.
    Inactive,
    /// No longer reported by a complete run.
    Resolved,
    /// Every remembered finding.
    All,
}

impl From<StatusFilter> for lighthouse_session::review::StatusFilter {
    fn from(status: StatusFilter) -> Self {
        match status {
            StatusFilter::Open => Self::Open,
            StatusFilter::Suppressed => Self::Suppressed,
            StatusFilter::Narrowing => Self::Narrowing,
            StatusFilter::Inactive => Self::Inactive,
            StatusFilter::Resolved => Self::Resolved,
            StatusFilter::All => Self::All,
        }
    }
}

/// `review resolve`.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResolveArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub finding: FingerprintArgs,
    /// `fail`: the finding is right. `pass`: the code conforms. `notApplicable`:
    /// the decision does not apply here.
    #[arg(long, value_enum)]
    pub judgment: JudgmentArg,
    /// With `fail`: why the finding stays, recorded as an external
    /// suppression that keeps it out of later reports.
    #[arg(long, value_name = "JUSTIFICATION")]
    pub suppress: Option<String>,
    /// Why, written for the next reader.
    #[arg(long)]
    pub reason: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub attributed: Attributed,
    /// The `last seen` time of the finding as you read it (its `seen`
    /// evidence); refuses the judgment if the finding has been seen again
    /// since.
    #[arg(long, value_name = "LAST_SEEN")]
    pub seen: Option<String>,
}

impl ReviewResolveArgs {
    /// The judgment these arguments record, attributed by `ctx` and made at
    /// the project's current commit.
    pub fn judgment(&self, ctx: &Context) -> Result<NewJudgment> {
        Ok(NewJudgment {
            fingerprint: self.finding.fingerprint.clone(),
            judgment: self.judgment.into(),
            reason: self.reason.clone(),
            suppress: self.suppress.clone(),
            attribution: ctx.attribution(self.attributed.attributed_to.as_deref())?,
            commit: head(&ctx.root()?),
            expect_seen: self.seen.clone(),
            lighthouse_version: env!("CARGO_PKG_VERSION").to_owned(),
        })
    }
}

/// `review show` and `review history`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintArgs {
    /// A fingerprint, or the start of one (at least 7 characters).
    #[arg(value_name = "FP")]
    pub fingerprint: String,
}

/// A judgment, spelled as SARIF and the rule pipeline spell it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize, JsonSchema)]
pub enum JudgmentArg {
    /// The code conforms: the finding is a false positive.
    #[serde(rename = "pass")]
    #[value(name = "pass")]
    Pass,
    /// The finding is right.
    #[serde(rename = "fail")]
    #[value(name = "fail")]
    Fail,
    /// The decision does not apply here; a hint to narrow it.
    #[serde(rename = "notApplicable")]
    #[value(name = "notApplicable")]
    NotApplicable,
}

/// `model list`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelListArgs {
    /// Only the models bound to these decisions: ids or globs.
    #[arg(long, value_delimiter = ',', value_name = "ID")]
    pub decision: Vec<String>,
    /// List at most this many models and say how many were left out.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

/// `config show`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigShowArgs {
    /// Name, for each key, the layer its value comes from: `bundled`, or the
    /// file.
    #[arg(long)]
    pub origin: bool,
}

/// `model train`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelTrainArgs {
    /// The decision whose labels the model learns, such as
    /// `design/prefer-method`.
    #[arg(value_name = "ID")]
    pub id: String,
    /// The model to train, `<provider>/<model>`. Default: the one bound to
    /// the decision.
    #[arg(long, value_name = "REF")]
    pub model: Option<String>,
    /// Also label the subjects nobody labelled with this model's answers
    /// (weight 0.5): a fail where it is sure of one, a pass where it is sure
    /// of the other. `<provider>/<model>`.
    #[arg(long, value_name = "REF")]
    pub from_model: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub labels: LabelSource,
}

/// `model attach`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelAttachArgs {
    /// The decision the model judges, such as `design/prefer-method`.
    #[arg(value_name = "ID")]
    pub id: String,
    /// The model to attach, `<provider>/<model>@sha256:<hex>` for a learned
    /// one: graded as the first layer of the decision before anything is
    /// written.
    #[arg(long, value_name = "REF")]
    pub model: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub labels: LabelSource,
    #[command(flatten)]
    #[serde(flatten)]
    pub attributed: Attributed,
}

/// `model rebuild`.
#[derive(Debug, Default, Clone, Args)]
pub struct ModelRebuildArgs {
    /// Decisions or globs such as `design/*`; default: every decision with an
    /// attached model.
    #[arg(value_name = "ID")]
    pub decision: Vec<String>,
    #[command(flatten)]
    pub labels: LabelSource,
}

/// `model detach`.
#[derive(Debug, Default, Clone, Args, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelDetachArgs {
    /// The decision whose attached model is detached, such as
    /// `design/prefer-method`.
    #[arg(value_name = "ID")]
    pub id: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub attributed: Attributed,
}

/// The text of a document given as text, or an object given as JSON.
fn text_or_object<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    match Value::deserialize(deserializer)? {
        Value::String(text) => Ok(text),
        other => Ok(other.to_string()),
    }
}

/// A document is YAML or JSON text, or an object.
fn document_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": ["string", "object"] })
}
