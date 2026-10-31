//! The commands an agent also uses, as the command line spells them. The
//! first line of a doc comment is the command's one-line summary (its MCP
//! `title`), the whole comment its long help (its MCP `description`).

use std::sync::OnceLock;

use clap::{Args, Subcommand};

use crate::surface::{
    Result,
    args::{
        CheckArgs, ConfigShowArgs, DecisionEvalArgs, DecisionListArgs, DecisionShowArgs,
        DecisionSignalsArgs, DecisionTestArgs, DocumentArgs, FingerprintArgs, FixOptions,
        ModelAttachArgs, ModelDetachArgs, ModelListArgs, ModelRebuildArgs, ModelTrainArgs,
        ReviewListArgs, ReviewResolveArgs, SignalDismissArgs,
    },
    check, config, decision, model,
    reply::{CheckFormat, Context, FormatArg, Reply},
    review,
};

const CHECK_EXAMPLES: &str = "\
Examples:
  lighthouse check
  lighthouse check src/ --decision 'design/*' --format json --limit 10
  lighthouse check --changed --strict
  lighthouse check --fix --dry-run --decision design/exported-doc";

const RESOLVE_EXAMPLES: &str = "\
Examples:
  lighthouse review resolve 3fa9c1e --judgment pass --reason 'the helper is generated'
  lighthouse review resolve 3fa9c1e --judgment fail --suppress 'tracked in #214'
  lighthouse review resolve 3fa9c1e --judgment notApplicable --attributed-to agent:claude";

const EVAL_EXAMPLES: &str = "\
Examples:
  lighthouse decision eval design/prefer-method
  lighthouse decision eval design/prefer-method --check candidate.yaml --format json
  lighthouse decision eval design/prefer-method --repo cel=../cel --audit 50";

const TRAIN_EXAMPLES: &str = "\
Examples:
  lighthouse model train design/prefer-method
  lighthouse model train design/prefer-method --model local/learned --repo cel=../cel
  lighthouse model train design/prefer-method --from-model openai/gpt-5 --format json";

const REBUILD_EXAMPLES: &str = "\
Examples:
  lighthouse model rebuild
  lighthouse model rebuild 'design/*' --repo cel=../cel
  lighthouse model rebuild design/prefer-method --format json";

const ATTACH_EXAMPLES: &str = "\
Examples:
  lighthouse model attach design/prefer-method --model local/learned@sha256:3f2a9c1e
  lighthouse model attach design/prefer-method --model local/learned@sha256:3f2a9c1e --repo cel=../cel
  lighthouse model attach design/prefer-method --model local/learned@sha256:3f2a9c1e --attributed-to agent:claude --format json";

const SIGNALS_EXAMPLES: &str = "\
Examples:
  lighthouse decision signals
  lighthouse decision signals --all --limit 50 --format json
  lighthouse decision signal dismiss 3fa9c1e --reason 'a request, not a rule'";

const SHOW_EXAMPLES: &str = "\
Examples:
  lighthouse config show
  lighthouse config show --origin
  lighthouse config show --origin --format json";

/// `check`.
#[derive(Debug, Default, Args)]
pub struct CheckCommand {
    #[command(flatten)]
    pub args: CheckArgs,
    /// `text`, `json` (findings grouped by decision) or `sarif`.
    #[arg(long, value_enum, default_value_t)]
    pub format: CheckFormat,
    /// Print where the time went to stderr, phase by phase: reading, each
    /// language provider, merging, analyzers, the slowest rules, identity,
    /// the store and the report. Findings and exit code do not change.
    #[arg(long)]
    pub timings: bool,
    /// Fix what the catalog's fixers can fix, then report what is left. Only
    /// safe fixes are written, whatever makes them (a rule, a command, a
    /// plugin); unsafe ones, which every fix by a model is, are shown as
    /// previews with how to apply them. Each
    /// fix is verified (re-checked, with the safe fixes of what it introduced)
    /// and a file that gets worse is rolled back. PATHS and --decision select which findings are fixed.
    #[arg(long)]
    pub fix: bool,
    /// With --fix: fix only the findings with these fingerprints (repeatable;
    /// prefixes of at least 7 characters allowed). With --unsafe-fixes it
    /// applies the preview of each as it was shown, kept by the run that made
    /// it: a fix is never made again, a preview that is not kept is declined,
    /// and a file that changed since gives a conflict.
    #[arg(long, requires = "fix", value_name = "FINGERPRINT")]
    pub fingerprint: Vec<String>,
    #[command(flatten)]
    pub options: FixOptions,
}

/// `decision <verb>`.
#[derive(Debug, Subcommand)]
pub enum DecisionCommand {
    /// List the decisions that have a rule.
    List {
        #[command(flatten)]
        args: DecisionListArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Show a decision or rule: context, requirement, examples and status.
    Show {
        #[command(flatten)]
        args: DecisionShowArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Add a project-local decision from a Decision document.
    ///
    /// The document is YAML or JSON, from a file or stdin (`-`). Nothing is
    /// written unless the candidate validates, compiles and every example
    /// passes the whole engine; a rejection leaves the project untouched. A
    /// document without `spec.status` is written as `proposed`: it produces no
    /// findings until a person accepts it. Decisions that may repeat it are named
    /// in the reply ("possible duplicate"), and the signals named in
    /// `provenance.wasDerivedFrom` as `signal:<id>` become recorded. The
    /// decision also needs the `local` plugin in lighthouse.toml to run in
    /// `check`.
    Create {
        #[command(flatten)]
        args: DocumentArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Replace a project-local decision with a Decision document.
    ///
    /// The same gate as `decision create` applies, and nothing is written when
    /// it fails. The signals the document names in `provenance.wasDerivedFrom`
    /// become linked. A bundled decision is not changed in place: its level and
    /// options are the `rules` of lighthouse.toml, and different wording or
    /// examples are a local decision.
    Update {
        #[command(flatten)]
        args: DocumentArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// List the session signals: what the hooks heard of design decisions.
    ///
    /// The hooks classify each prompt of a person (directive, correction,
    /// preference, none, or an abstention when no rule decides) and record the
    /// findings an agent's edits introduced or resolved, locally and never in
    /// git. By default the open signals that state a decision are listed; --all
    /// lists every one. The counts are by class, by rule that fired, by
    /// abstention and by status, and the measures of the loop are the share of
    /// signals recorded or linked, of proposals named a possible duplicate and of
    /// proposals a person accepted, each with a Wilson 95% interval. Retention
    /// and whether the agent is told are `[signals]` in lighthouse.toml.
    #[command(after_long_help = SIGNALS_EXAMPLES)]
    Signals {
        #[command(flatten)]
        args: DecisionSignalsArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Settle the session signals.
    Signal {
        #[command(subcommand)]
        command: SignalCommand,
    },
    /// Run the examples of checked decisions through the whole engine.
    ///
    /// The bundled catalog and the project's `.lighthouse/decisions`. Examples
    /// run in each language whose plugin the config lists (or only in
    /// --language); a decision needs an example for every language run. Exits
    /// 1 when any example fails.
    Test {
        #[command(flatten)]
        args: DecisionTestArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Measure a decision's check against its labels.
    ///
    /// Reports precision, FPR, recall, FNR, selector precision and coverage,
    /// each with a Wilson 95% interval; the Brier score, ECE and
    /// AUROC of the probabilities models gave, with how many subjects they answered;
    /// the model calls and their cost, and a verdict against the decision's evaluation policy
    /// (`rules."<id>".evaluation`). With --check, a candidate decision file is
    /// measured beside it and their agreement is reported. Labelled positives
    /// come mostly from flagged findings, so recall is an upper bound; --audit
    /// writes unflagged selected subjects for a person to label.
    #[command(after_long_help = EVAL_EXAMPLES)]
    Eval {
        #[command(flatten)]
        args: DecisionEvalArgs,
        #[command(flatten)]
        output: FormatArg,
    },
}

/// `decision signal <verb>`.
#[derive(Debug, Subcommand)]
pub enum SignalCommand {
    /// Dismiss a signal: it is not a design decision.
    ///
    /// A dismissed signal stays in `decision signals --all` with its reason
    /// and who gave it, and is not asked about again. The reason is the label
    /// that a later evaluation of the classifier learns from. The signal id is
    /// the one `decision signals` shows, or a unique prefix of at least 7
    /// characters.
    #[command(after_long_help = SIGNALS_EXAMPLES)]
    Dismiss {
        #[command(flatten)]
        args: SignalDismissArgs,
        #[command(flatten)]
        output: FormatArg,
    },
}

/// `review <verb>`.
#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    /// List the findings that ask for review.
    ///
    /// By default the open findings of decisions that authored `warn` or
    /// `info` and that no judgment stands for. A finding's `last seen` is the
    /// `--seen` evidence `review resolve` takes.
    List {
        #[command(flatten)]
        args: ReviewListArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Show one finding: where it is, its evidence and its latest review.
    Show {
        #[command(flatten)]
        args: FingerprintArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Record a judgment on a finding.
    ///
    /// `fail` means the finding is right; add --suppress with a justification
    /// to leave it in place on purpose (a SARIF `external` suppression).
    /// `pass` means the code conforms (a false positive); `notApplicable`
    /// means the decision does not apply here, a hint to narrow it. A
    /// judgment that is not `fail`, or a `fail` with a suppression, keeps the
    /// finding out of later reports while its decision and evidence stay as
    /// they are; an error is recorded but never hidden. Judgments are appended
    /// to .lighthouse/decisions.jsonl, which is meant to be committed, and
    /// never edited: a later one replaces the standing of the finding.
    ///
    /// --seen is the `last seen` time of the finding as you read it; the
    /// judgment is refused if the finding has been seen again since.
    #[command(after_long_help = RESOLVE_EXAMPLES)]
    Resolve {
        #[command(flatten)]
        args: ReviewResolveArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Show every judgment recorded on a finding, oldest first.
    History {
        #[command(flatten)]
        args: FingerprintArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Delete resolved and inactive findings that nobody judged.
    ///
    /// Findings with judgments stay, because the judgments are labels.
    Prune {
        /// Only those resolved or inactive for at least this many days.
        #[arg(long, value_name = "DAYS")]
        older_than: Option<u32>,
    },
}

/// `model <verb>`.
#[derive(Debug, Subcommand)]
pub enum ModelCommand {
    /// List the models bound to decisions.
    ///
    /// The classification model, the models rules name and the trained models
    /// attached to decisions, with whether asking each reaches a service
    /// outside the machine.
    List {
        #[command(flatten)]
        args: ModelListArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Fit a model on the labels of a decision.
    ///
    /// The labels are the rows of the labels table, the judgments recorded
    /// under its current meaning version, and optionally the answers of another model;
    /// the decision's examples ride in the question. Writes the dataset under
    /// `.lighthouse/datasets/` and the model under `.lighthouse/models/`, named
    /// by its digest; it binds nothing (bind `local/learned@sha256:<hex>`).
    #[command(after_long_help = TRAIN_EXAMPLES)]
    Train {
        #[command(flatten)]
        args: ModelTrainArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Make a model the first layer of a decision, by a revision.
    ///
    /// Grades the model as `decision eval --model` does and appends a
    /// `Revision` to the decision log only when it meets the evaluation
    /// policy, loses no recall, newly fails none of the decision's examples
    /// that should pass, and decides a labelled subject the check leaves for
    /// review. Otherwise it exits 1 with the reasons and writes nothing; there
    /// is no way to force it. The next `check` asks the model first.
    #[command(after_long_help = ATTACH_EXAMPLES)]
    Attach {
        #[command(flatten)]
        args: ModelAttachArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Remove the model a revision attached to a decision.
    ///
    /// Appends a `Revision` that binds no model; exits 2 when none is
    /// attached. The model and the labels it learned from stay where they are.
    Detach {
        #[command(flatten)]
        args: ModelDetachArgs,
        #[command(flatten)]
        output: FormatArg,
    },
    /// Make the missing artifacts of attached models again, from their labels.
    ///
    /// A learned model is a file named by its digest and kept out of git; a
    /// fresh clone has the revision that attached it and not the file. Per
    /// attached model the status is `present` (the file is what its name says),
    /// `rebuilt` (training made it again, bit for bit), `differs` (a file or a
    /// training run gives another digest; nothing is deleted), `labels-changed`
    /// (the labels are no longer those it was attached with; nothing is
    /// trained), `not-rebuildable` (its trainer is not deterministic or does
    /// not learn) or `not-local` (a service, not a file). Exits 1 unless every
    /// model is `present`, `rebuilt` or `not-local`.
    #[command(after_long_help = REBUILD_EXAMPLES)]
    Rebuild {
        #[command(flatten)]
        args: ModelRebuildArgs,
        #[command(flatten)]
        output: FormatArg,
    },
}

/// `config <verb>`.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show the effective environment: models, providers, bindings, fix, sandbox.
    ///
    /// Meaning is the project's alone (`lighthouse.toml`); how this machine
    /// computes and applies is read from, in order, the bundled defaults, the
    /// project, your file (`$XDG_CONFIG_HOME/lighthouse/config.toml`, default
    /// `~/.config/lighthouse/config.toml`) and the repository-local
    /// `.lighthouse/local.toml`. A later layer wins key by key; a provider is
    /// replaced whole. Shows each key that has a value, sorted; --origin names
    /// the layer each value comes from, as `git config --show-origin` does.
    /// The values a provider passes to its program (`env`) are hidden.
    #[command(after_long_help = SHOW_EXAMPLES)]
    Show {
        #[command(flatten)]
        args: ConfigShowArgs,
        #[command(flatten)]
        output: FormatArg,
    },
}

#[derive(Debug, clap::Parser)]
#[command(no_binary_name = true)]
struct Tree {
    #[command(subcommand)]
    action: Action,
}

/// A command of the agent loop, with its arguments.
#[derive(Debug, Subcommand)]
pub enum Action {
    /// Check the project and print findings.
    ///
    /// PATHS select which findings are reported, not what is analyzed: the
    /// whole project is always analyzed, because decisions and language
    /// providers need more than the reported files (callers, packages, build
    /// context).
    ///
    /// Exit codes: 0 clean, 1 an error was found (or warnings fail the run:
    /// --strict, --max-warnings; info never does), 2 usage or runtime error,
    /// 3 the analysis was incomplete (a file could not be read or loaded, a
    /// plugin crashed or timed out). "Not checked" is never "passed";
    /// --allow-incomplete reports it but does not fail on it.
    ///
    /// Plugins named by the config are started and run with your privileges,
    /// like build scripts: only check projects whose config you trust. A
    /// plugin that cannot start, or that is not the protocol version or
    /// identity its entry names, is an error (exit 2); one that starts but
    /// crashes, times out or answers badly is reported as incomplete (exit 3).
    #[command(after_long_help = CHECK_EXAMPLES)]
    Check(Box<CheckCommand>),
    /// Inspect, author, test and evaluate decisions.
    Decision {
        #[command(subcommand)]
        command: DecisionCommand,
    },
    /// Review the findings `check` remembers and record judgments on them.
    ///
    /// Exit codes of every subcommand: 0 on success, 2 on a usage or runtime
    /// error (unknown or ambiguous fingerprint, a suppression that does not
    /// fit the judgment, a finding seen again since --seen, an unusable store).
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// List, train, attach and detach the models that judge decisions.
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// Show the configuration of this machine.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

/// Runs the command.
pub fn run(ctx: &Context, action: &Action) -> Result<Reply> {
    match action {
        Action::Check(command) => check::check(ctx, command),
        Action::Decision { command } => match command {
            DecisionCommand::List { args, output } => decision::list(ctx, args, output.format),
            DecisionCommand::Show { args, output } => decision::show(ctx, args, output.format),
            DecisionCommand::Create { args, output } => decision::create(
                ctx,
                &decision::read_document(&args.document)?,
                output.format,
            ),
            DecisionCommand::Update { args, output } => decision::update(
                ctx,
                &decision::read_document(&args.document)?,
                output.format,
            ),
            DecisionCommand::Signals { args, output } => {
                decision::signals(ctx, args, output.format)
            }
            DecisionCommand::Signal { command } => match command {
                SignalCommand::Dismiss { args, output } => {
                    decision::dismiss(ctx, args, output.format)
                }
            },
            DecisionCommand::Test { args, output } => decision::test(ctx, args, output.format),
            DecisionCommand::Eval { args, output } => decision::eval(ctx, args, output.format),
        },
        Action::Review { command } => match command {
            ReviewCommand::List { args, output } => review::list(ctx, args, output.format),
            ReviewCommand::Show { args, output } => review::show(ctx, args, output.format),
            ReviewCommand::Resolve { args, output } => review::resolve(ctx, args, output.format),
            ReviewCommand::History { args, output } => review::history(ctx, args, output.format),
            ReviewCommand::Prune { older_than } => review::prune(ctx, *older_than),
        },
        Action::Model { command } => match command {
            ModelCommand::List { args, output } => model::list(ctx, args, output.format),
            ModelCommand::Train { args, output } => model::train(ctx, args, output.format),
            ModelCommand::Attach { args, output } => model::attach(ctx, args, output.format),
            ModelCommand::Detach { args, output } => model::detach(ctx, args, output.format),
            ModelCommand::Rebuild { args, output } => model::rebuild(ctx, args, output.format),
        },
        Action::Config { command } => match command {
            ConfigCommand::Show { args, output } => config::show(ctx, args, output.format),
        },
    }
}

/// The clap command of the agent-loop commands, built once, to walk by path.
pub fn command() -> &'static clap::Command {
    static TREE: OnceLock<clap::Command> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut tree = <Tree as clap::CommandFactory>::command();
        tree.build();
        tree
    })
}

/// The command at `path`, with the arguments it inherits built in.
pub fn command_at(path: &[&str]) -> &'static clap::Command {
    path.iter().fold(command(), |command, name| {
        command
            .find_subcommand(name)
            .unwrap_or_else(|| panic!("no command `{name}`"))
    })
}

/// The one-line summary and the long help of the command at `path`: what the
/// tool of that command has as `title` and `description`.
pub fn help(path: &[&str]) -> (String, String) {
    let command = command_at(path);
    let about = command
        .get_about()
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut long = command
        .get_long_about()
        .map(ToString::to_string)
        .unwrap_or_else(|| about.clone());
    if let Some(examples) = command.get_after_long_help() {
        long = format!("{long}\n\n{examples}");
    }
    (about, long)
}
