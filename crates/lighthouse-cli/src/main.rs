use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand, ValueEnum};
use lighthouse_config::FILE_NAME;
use lighthouse_report::{Format, render_with};
use lighthouse_session::{CheckRequest, DEFAULT_CONFIG, Session};
use review::ReviewCommand;

mod docs;
mod hook;
mod review;
mod rules;
mod setup;

/// Where `docs generate` writes the agent skill.
const SKILL_PATH: &str = "skills/lighthouse/SKILL.md";

/// What `init` keeps out of version control: the store is a local cache of
/// one machine's checks and reviews.
const STORE_IGNORE: &str = ".lighthouse/*.db*";

/// The decision log is committed; branches that both appended merge by
/// keeping the lines of both.
const LOG_ATTRIBUTES: &str = ".lighthouse/decisions.jsonl merge=union";

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
    /// Exit codes: 0 clean, 1 findings at or above the failing level, 2 usage
    /// or runtime error, 3 the analysis was incomplete (a file could not be
    /// read or loaded, a plugin crashed or timed out). "Not checked" is never
    /// "passed"; --allow-incomplete reports it but does not fail on it.
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
        /// text, json, sarif, agent (self-contained blocks for a coding agent)
        /// or agent-json (the same records as JSON lines).
        #[arg(long, default_value = "text")]
        format: Format,
        /// Print at most this many findings, errors first, and say how many
        /// were left out. Agent formats only.
        #[arg(long)]
        limit: Option<usize>,
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
        /// Exit by findings even when the analysis was incomplete; the
        /// incompleteness is still printed.
        #[arg(long)]
        allow_incomplete: bool,
        /// Run only these fully qualified rule ids.
        #[arg(long, value_delimiter = ',')]
        rules: Vec<String>,
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
    /// Inspect bundled rules.
    Rule {
        #[command(subcommand)]
        command: RuleCommand,
    },
    /// Show a pattern or rule: intent, requirement, examples and status.
    Explain { id: String },
    /// Generate or verify the Markdown docs derived from the pattern catalog.
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
    /// Serve the Model Context Protocol on stdio, for coding agents: tools to
    /// check, explain, review and author rules, and resources for the
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
enum RuleCommand {
    /// List bundled rules.
    List {
        /// Include every catalog pattern with its implementation status.
        #[arg(long)]
        all: bool,
    },
    /// Run the examples of implemented patterns: the bundled catalog and the
    /// project's `.lighthouse/rules`. Exits 1 when any example fails.
    ///
    /// Examples run in each language whose plugin the config lists (or only
    /// in --language); a pattern needs an example for every language run.
    Test {
        /// Pattern ids; default: every implemented pattern.
        ids: Vec<String>,
        #[arg(long)]
        language: Option<String>,
        /// Use this config file; the project root is then the current directory.
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DocsCommand {
    /// Write the pattern docs and the agent skill.
    Generate {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
        /// Where the generated agent skill goes.
        #[arg(long, default_value = SKILL_PATH)]
        skill: PathBuf,
    },
    /// Exit 1 when the pattern docs or the agent skill are stale.
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
    store: bool,
    strict: bool,
    allow_incomplete: bool,
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
            no_store,
            strict,
            allow_incomplete,
            rules,
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
                store: !no_store,
                strict,
                allow_incomplete,
            },
            &rules,
            config.as_deref(),
        ),
        Command::Review { command } => review::run(command),
        Command::Rule {
            command: RuleCommand::List { all },
        } => rules::list(all),
        Command::Rule {
            command:
                RuleCommand::Test {
                    ids,
                    language,
                    config,
                },
        } => rules::test(&ids, language.as_deref(), config.as_deref()),
        Command::Explain { id } => rules::explain(&id),
        Command::Docs { command } => match command {
            DocsCommand::Generate { out, skill } => docs::generate(&out, &skill),
            DocsCommand::Check { out, skill } => docs::check(&out, &skill),
        },
        Command::Init { agent } => init(agent),
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
) -> Result<u8> {
    if options.limit.is_some() && !briefed(options.format) {
        return Err("--limit applies to the agent formats only".into());
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
    print!(
        "{}",
        render_with(
            options.format,
            &outcome.diagnostics,
            &outcome.incomplete,
            &checked.briefing(briefed(options.format), options.limit)
        )
    );
    Ok(outcome.exit_code(options.strict, options.allow_incomplete))
}

/// Whether the format draws on the catalog.
fn briefed(format: Format) -> bool {
    matches!(format, Format::Agent | Format::AgentJson)
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
