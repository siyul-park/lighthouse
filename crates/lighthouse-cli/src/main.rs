use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use clap::{Parser, Subcommand, ValueEnum};
use lighthouse_config::FILE_NAME;
use lighthouse_engine::FailOn;
use lighthouse_report::{Briefing, Detail, Format, render_with};
use lighthouse_session::{CheckRequest, DEFAULT_CONFIG, FixRequest, Session};
use review::ReviewCommand;

mod decisions;
mod docs;
mod hook;
mod review;
mod setup;
mod spec;
mod timings;

/// Where `docs generate` writes the agent skill.
const SKILL_PATH: &str = "skills/lighthouse/SKILL.md";

/// What `init` keeps out of version control: the store is a local cache of
/// one machine's checks and reviews.
const STORE_IGNORE: &str = ".lighthouse/*.db*";

/// The decision log is committed; branches that both appended merge by
/// keeping the lines of both.
const LOG_ATTRIBUTES: &str = ".lighthouse/decisions.jsonl merge=union";

/// Most findings of a SARIF report whose fix is previewed.
const SARIF_FIXES: usize = 200;

#[derive(Parser)]
#[command(name = "lighthouse", version, about = "Design-quality linter")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check the project and print diagnostics.
    ///
    /// PATHS select which diagnostics are reported, not what is analyzed: the
    /// whole project is always analyzed, because rules and language providers
    /// need more than the reported files (callers, packages, build context).
    ///
    /// Exit codes: 0 clean, 1 an error was found (or warnings fail the run:
    /// --strict, --max-warnings; info never does), 2 usage or runtime error,
    /// 3 the analysis was incomplete (a file could not be read or loaded, a
    /// plugin crashed or timed out). "Not checked" is never "passed";
    /// --allow-incomplete reports it but does not fail on it.
    ///
    /// Plugins named by the config are started and run with your privileges,
    /// like build scripts: only check projects whose config you trust. A plugin
    /// that cannot start, or that is not the protocol version or identity its
    /// entry names, is an error (exit 2); one that starts but crashes, times out
    /// or answers badly is reported as incomplete (exit 3).
    Check {
        /// Where to report diagnostics; the whole project is analyzed anyway.
        /// Default: the whole project.
        paths: Vec<PathBuf>,
        /// Report only files changed in the working tree against HEAD
        /// (modified, added, renamed, untracked). A report filter like PATHS:
        /// the whole project is still analyzed.
        #[arg(long, conflicts_with = "diff")]
        changed: bool,
        /// Report only files changed since the merge base of BASE and HEAD,
        /// including the working tree and untracked files. A report filter.
        #[arg(long, value_name = "BASE")]
        diff: Option<String>,
        /// text, json, sarif, agent (findings grouped for a coding agent) or
        /// agent-json (the same groups as one JSON object).
        #[arg(long, default_value = "text")]
        format: Format,
        /// Print at most this many findings, errors first, and say how many
        /// were left out. Agent formats only.
        #[arg(long)]
        limit: Option<usize>,
        /// compact (default): findings grouped by decision, then by file, a
        /// decision's text said once. full: one self-contained record per
        /// finding. Agent formats only.
        #[arg(long, default_value = "compact")]
        detail: Detail,
        /// Do not record this run in `.lighthouse/lighthouse.db` and do not
        /// apply review verdicts. By default a run is recorded: findings it
        /// no longer reports in the reported scope are marked resolved (never
        /// when the analysis was incomplete), and findings whose latest
        /// verdict is a rejection stay out of the report.
        #[arg(long)]
        no_store: bool,
        /// Fail on warnings too.
        #[arg(long)]
        strict: bool,
        /// Fail when there are more than N warnings (errors always fail).
        #[arg(long, value_name = "N")]
        max_warnings: Option<usize>,
        /// Exit by findings even when the analysis was incomplete; the
        /// incompleteness is still printed.
        #[arg(long)]
        allow_incomplete: bool,
        /// Run only these fully qualified rule ids.
        #[arg(long, value_delimiter = ',')]
        rules: Vec<String>,
        /// Print where the time went to stderr, phase by phase: reading, each
        /// language provider, merging, analyzers, the slowest rules, identity,
        /// the store and the report. Findings and exit code do not change.
        #[arg(long)]
        timings: bool,
        /// Fix what the catalog's fixers can fix, then report what is left.
        /// Only safe fixes of mechanical rules are applied; each is verified
        /// (formatted, re-checked) and a file that gets worse is rolled back.
        /// PATHS and --rules select which findings are fixed.
        #[arg(long)]
        fix: bool,
        /// With --fix: print the unified diff and leave every file as it was.
        #[arg(long, requires = "fix")]
        dry_run: bool,
        /// With --fix: also apply suggested fixes.
        #[arg(long, requires = "fix")]
        unsafe_fixes: bool,
        /// With --fix: use this registered fixer for the selected findings
        /// instead of the one their decision names. Its fixes count as
        /// suggested.
        #[arg(long, requires = "fix", value_name = "ID")]
        fixer: Option<String>,
        /// Use this config file; the project root is then the current directory.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Review the findings `check` remembers and record verdicts on them.
    ///
    /// Exit codes of every subcommand: 0 on success, 2 on a usage or runtime
    /// error (unknown or ambiguous fingerprint, a reason that does not fit the
    /// verdict, a finding seen again since --seen, an unusable store).
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// Inspect and test decisions.
    Decision {
        #[command(subcommand)]
        command: DecisionCommand,
    },
    /// Renamed to `decision`; kept so the old command says what to run.
    #[command(hide = true)]
    Rule {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show a decision or rule: intent, requirement, examples and status.
    Explain { id: String },
    /// Validate and migrate spec documents.
    Spec {
        #[command(subcommand)]
        command: SpecCommand,
    },
    /// Print the JSON Schema of a kind of spec document; without a kind, list
    /// the kinds.
    Schema {
        /// A kind such as `Decision` or `Project`, in any letter case.
        kind: Option<String>,
        /// Write the schema of every kind into this directory instead.
        #[arg(long, value_name = "DIR", conflicts_with = "kind")]
        write: Option<PathBuf>,
    },
    /// Generate or verify the Markdown docs derived from the decision catalog.
    Docs {
        #[command(subcommand)]
        command: DocsCommand,
    },
    /// Write a default lighthouse.toml in the current directory.
    ///
    /// With --agent the project is also wired for a coding agent: the MCP
    /// server, the hooks and the skill are merged into the agent's own
    /// files without overwriting anything you wrote, and running it again
    /// changes nothing. An existing lighthouse.toml is kept in that case.
    Init {
        /// Set up this coding agent as well.
        #[arg(long, value_enum)]
        agent: Option<Agent>,
    },
    /// Trust this project to run the commands it names: command fixers and the
    /// formatters of `lighthouse.toml`. Trust is yours, kept in
    /// `~/.lighthouse/trust.toml` for this root and the exact `lighthouse.toml`
    /// and `.lighthouse/rules` you have read; a change to either withdraws it.
    /// `LIGHTHOUSE_TRUST=1` trusts every project, for CI. Without trust a
    /// command fix is declined and formatting is skipped; checks and the
    /// verification of fixes still run.
    ///
    /// It lists the formatters and fixer commands it would trust and asks for
    /// confirmation on the terminal; without a terminal it refuses unless
    /// `--yes` says you have read the list.
    Trust {
        /// Withdraw the trust instead.
        #[arg(long)]
        revoke: bool,
        /// Trust without asking; for scripts, after reading what is listed.
        #[arg(long, conflicts_with = "revoke")]
        yes: bool,
    },
    /// Serve the Model Context Protocol on stdio, for coding agents: tools to
    /// check, explain, review and author decisions, and resources for the
    /// catalog. Verdicts recorded through it are reviews by an `agent`
    /// (the id is $LIGHTHOUSE_REVIEWER, else the client's name).
    Mcp,
    /// Entry points for agent hooks. Reads the hook payload on stdin.
    Hook {
        #[command(subcommand)]
        agent: HookAgent,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Agent {
    ClaudeCode,
}

#[derive(Subcommand)]
enum HookAgent {
    /// Claude Code hooks (see docs/agents.md).
    ClaudeCode {
        #[arg(value_enum)]
        event: hook::Event,
        /// Say that the analysis was incomplete but do not block on it.
        #[arg(long)]
        allow_incomplete: bool,
    },
}

#[derive(Subcommand)]
enum DecisionCommand {
    /// List the decisions that have a rule.
    List {
        /// Include every catalog decision with its implementation status.
        #[arg(long)]
        all: bool,
    },
    /// Run the examples of checked decisions: the bundled catalog and the
    /// project's `.lighthouse/decisions`. Exits 1 when any example fails.
    ///
    /// Examples run in each language whose plugin the config lists (or only
    /// in --language); a decision needs an example for every language run.
    Test {
        /// Decision ids; default: every checked decision.
        ids: Vec<String>,
        #[arg(long)]
        language: Option<String>,
        /// Use this config file; the project root is then the current directory.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum SpecCommand {
    /// Check every spec document under PATHS (default: the project's
    /// configuration and `.lighthouse/decisions`): each against the schema of
    /// its kind, and against the others: presets name decisions that exist,
    /// fix operations name registered order keys, `extends` names a decision,
    /// CEL compiles, examples are well formed. Exits 1 when anything is wrong.
    Validate {
        paths: Vec<PathBuf>,
        /// Also run the examples of the decisions found, through the whole
        /// engine.
        #[arg(long)]
        examples: bool,
    },
    /// Rewrite documents in the formats from before the resource model
    /// (`lighthouse.toml`, `lighthouse-plugin.toml`, a catalog of decision files of the earlier format,
    /// `.lighthouse/rules`) as `lighthouse/v1alpha1` resources. A document that
    /// already is one is left alone, so running it again changes nothing.
    Migrate {
        /// Files or directories (default: the project's configuration and
        /// `.lighthouse/rules`).
        paths: Vec<PathBuf>,
        /// Say what would change and change nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum DocsCommand {
    /// Write the decision docs and the agent skill.
    Generate {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
        /// Where the generated agent skill goes.
        #[arg(long, default_value = SKILL_PATH)]
        skill: PathBuf,
    },
    /// Exit 1 when the decision docs or the agent skill are stale.
    Check {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
        #[arg(long, default_value = SKILL_PATH)]
        skill: PathBuf,
    },
}

type Result<T> = lighthouse_session::Result<T>;

struct Options {
    format: Format,
    limit: Option<usize>,
    detail: Detail,
    store: bool,
    fail_on: FailOn,
    allow_incomplete: bool,
    timings: bool,
}

/// What `check --fix` is asked to do.
struct FixOptions {
    dry_run: bool,
    unsafe_fixes: bool,
    fixer: Option<String>,
}

/// A report filter taken from git instead of paths.
struct Reported<'a> {
    changed: bool,
    diff: Option<&'a str>,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("lighthouse: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Command::Check {
            paths,
            changed,
            diff,
            format,
            limit,
            detail,
            no_store,
            strict,
            max_warnings,
            allow_incomplete,
            rules,
            timings,
            fix,
            dry_run,
            unsafe_fixes,
            fixer,
            config,
        } => check(
            &paths,
            Reported {
                changed,
                diff: diff.as_deref(),
            },
            Options {
                format,
                limit,
                detail,
                store: !no_store,
                fail_on: FailOn {
                    strict,
                    max_warnings,
                },
                allow_incomplete,
                timings,
            },
            &rules,
            config.as_deref(),
            fix.then_some(FixOptions {
                dry_run,
                unsafe_fixes,
                fixer,
            }),
        ),
        Command::Review { command } => review::run(command),
        Command::Decision {
            command: DecisionCommand::List { all },
        } => decisions::list(all),
        Command::Decision {
            command:
                DecisionCommand::Test {
                    ids,
                    language,
                    config,
                },
        } => decisions::test(&ids, language.as_deref(), config.as_deref()),
        Command::Rule { args } => Err(format!(
            "`lighthouse rule` is now `lighthouse decision`: run `lighthouse decision {}`",
            args.join(" ")
        )
        .trim_end()
        .to_owned()
        .into()),
        Command::Explain { id } => decisions::explain(&id),
        Command::Spec { command } => match command {
            SpecCommand::Validate { paths, examples } => spec::validate(&paths, examples),
            SpecCommand::Migrate { paths, dry_run } => spec::migrate(&paths, dry_run),
        },
        Command::Schema { kind, write } => spec::schema(kind.as_deref(), write.as_deref()),
        Command::Docs { command } => match command {
            DocsCommand::Generate { out, skill } => docs::generate(&out, &skill),
            DocsCommand::Check { out, skill } => docs::check(&out, &skill),
        },
        Command::Init { agent } => init(agent),
        Command::Trust { revoke, yes } => trust(revoke, yes),
        Command::Mcp => {
            lighthouse_mcp::serve()?;
            Ok(0)
        }
        Command::Hook {
            agent:
                HookAgent::ClaudeCode {
                    event,
                    allow_incomplete,
                },
        } => Ok(hook::run(event, allow_incomplete)),
    }
}

fn check(
    paths: &[PathBuf],
    reported: Reported,
    options: Options,
    only: &[String],
    config: Option<&Path>,
    fix: Option<FixOptions>,
) -> Result<u8> {
    let started = Instant::now();
    let agent = matches!(options.format, Format::Agent | Format::AgentJson);
    if options.limit.is_some() && !agent {
        return Err("--limit applies to the agent formats only".into());
    }
    if options.detail != Detail::default() && !agent {
        return Err("--detail applies to the agent formats only".into());
    }
    if let Some(fix) = fix {
        let dry_run = fix.dry_run;
        fixing(paths, only, config, options.store, fix)?;
        if dry_run {
            return Ok(0);
        }
    }
    let request = CheckRequest {
        paths: paths.to_vec(),
        changed: reported.changed,
        diff: reported.diff.map(str::to_owned),
        rules: only.to_vec(),
        store: options.store,
    };
    let checked = lighthouse_session::check(Session::load(config)?, &request)?;
    for message in &checked.messages {
        eprintln!("lighthouse: {message}");
    }
    let outcome = &checked.outcome;
    let fixes = match options.format {
        Format::Agent | Format::AgentJson => checked.shown_fixes(options.limit, options.detail),
        Format::Sarif => {
            let all: Vec<_> = outcome.diagnostics.iter().take(SARIF_FIXES).collect();
            checked.fixes(&all)
        }
        Format::Text | Format::Json => BTreeMap::new(),
    };
    let reporting = Instant::now();
    print!(
        "{}",
        render_with(
            options.format,
            &outcome.diagnostics,
            &outcome.incomplete,
            &Briefing {
                detail: options.detail,
                fixes: Some(&fixes),
                ..checked.briefing(options.limit)
            }
        )
    );
    if options.timings {
        for line in timings::lines(&outcome.timings, reporting.elapsed(), started.elapsed()) {
            writeln!(io::stderr(), "{line}").ok();
        }
    }
    Ok(outcome.exit_code(options.fail_on, options.allow_incomplete))
}

fn trust(revoke: bool, yes: bool) -> Result<u8> {
    if revoke {
        let root = lighthouse_session::project_root()?;
        let had = lighthouse_session::revoke(&root)?;
        println!(
            "{} {}",
            if had {
                "no longer trusting"
            } else {
                "was not trusted:"
            },
            root.display()
        );
        return Ok(0);
    }
    let session = Session::load(None)?;
    println!("{} runs:", session.root.display());
    for command in &session.basis.commands {
        println!("  {command}");
    }
    if session.basis.commands.is_empty() {
        println!("  no formatter or fixer command");
    }
    if !yes && !confirmed()? {
        return Err(
            "not trusted: no confirmation (use a terminal, or `--yes` after reading the list)"
                .into(),
        );
    }
    let file = lighthouse_session::trust(&session.root, &session.basis)?;
    println!(
        "trusting {} as listed (recorded in {}); programs outside the project are trusted by command line only",
        session.root.display(),
        file.display()
    );
    Ok(0)
}

/// Asks on the terminal whether to trust; false without one.
fn confirmed() -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    print!("Trust this project to run them? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn init(agent: Option<Agent>) -> Result<u8> {
    let dir = env::current_dir()?;
    let path = dir.join(FILE_NAME);
    if path.exists() {
        if agent.is_none() {
            return Err(format!("{} already exists", path.display()).into());
        }
        println!("kept {}", path.display());
    } else {
        fs::write(&path, DEFAULT_CONFIG)?;
        println!("wrote {}", path.display());
        if ensure_line(&path.with_file_name(".gitignore"), STORE_IGNORE)? {
            println!("added {STORE_IGNORE} to .gitignore");
        }
        if ensure_line(&path.with_file_name(".gitattributes"), LOG_ATTRIBUTES)? {
            println!("added the decision log to .gitattributes (merge=union)");
        }
    }
    if let Some(Agent::ClaudeCode) = agent {
        setup::claude_code(&dir)?;
    }
    Ok(0)
}
/// Adds `line` to the file, creating it if needed; false when the file
/// already has it.
fn ensure_line(path: &Path, line: &str) -> Result<bool> {
    let mut text = fs::read_to_string(path).unwrap_or_default();
    if text.lines().any(|existing| existing.trim() == line) {
        return Ok(false);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    fs::write(path, text)?;
    Ok(true)
}

/// Fixes the selected findings and says what happened: the diff of a dry run
/// on stdout, everything else on stderr.
fn fixing(
    paths: &[PathBuf],
    only: &[String],
    config: Option<&Path>,
    store: bool,
    fix: FixOptions,
) -> Result<()> {
    let request = FixRequest {
        paths: paths.to_vec(),
        rules: only.to_vec(),
        dry_run: fix.dry_run,
        unsafe_fixes: fix.unsafe_fixes,
        fixer: fix.fixer,
        store,
        ..FixRequest::default()
    };
    let fixed = lighthouse_session::fix(Session::load(config)?, &request)?;
    for message in &fixed.messages {
        eprintln!("lighthouse: {message}");
    }
    if fixed.dry_run {
        print!("{}", fixed.diff);
    }
    let verb = if fixed.dry_run { "would fix" } else { "fixed" };
    for fix in fixed.applied() {
        eprintln!(
            "{verb} {} [{}] {} ({}): {}",
            fix.rule,
            fix.safety,
            fix.files.join(", "),
            fix.fixer,
            fix.description
        );
    }
    for fix in fixed.declined() {
        eprintln!(
            "not fixed {}:{} {}: {}",
            fix.path, fix.line, fix.rule, fix.reason
        );
    }
    eprintln!(
        "lighthouse: {} fix(es) {}, {} left alone",
        fixed.applied().len(),
        if fixed.dry_run { "proposed" } else { "applied" },
        fixed.declined().len()
    );
    Ok(())
}
