//! The command line: global flags, the agent-loop commands of
//! `lighthouse_cli::surface`, and the commands only a terminal needs (setup,
//! housekeeping, build).

use std::{path::PathBuf, time::Duration};

use lighthouse_cli::surface::Action;

use crate::terminal::hook;

use clap::{ArgAction, ColorChoice, Parser, Subcommand, ValueEnum, builder::BoolishValueParser};

use clap_complete::Shell;

/// What `--version` prints: the release workflow sets `LIGHTHOUSE_VERSION` at
/// build time; any other build is the Cargo version marked `-dev`.
const VERSION: &str = match option_env!("LIGHTHOUSE_VERSION") {
    Some(version) => version,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Where `docs generate` writes the agent skill.
pub const SKILL_PATH: &str = "skills/lighthouse/SKILL.md";

#[derive(Debug, Parser)]
#[command(name = "lighthouse", version = VERSION, about = "Design-quality linter")]
pub struct Cli {
    /// The project file; the project root is then the current directory.
    #[arg(long, global = true, env = "LIGHTHOUSE_CONFIG", value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// Color of help and messages. `auto` honours NO_COLOR and CLICOLOR_FORCE.
    #[arg(
        long,
        global = true,
        env = "LIGHTHOUSE_COLOR",
        value_enum,
        default_value_t = ColorChoice::Auto
    )]
    pub color: ColorChoice,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(flatten)]
    Surface(Action),
    /// Manage the result cache in `.lighthouse/cache`.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Validate spec documents.
    Spec {
        #[command(subcommand)]
        command: SpecCommand,
    },
    /// Print the JSON Schema of a kind of document or output.
    ///
    /// Without a kind, lists the kinds: the spec documents and log records, and
    /// what the commands print with `--format json`.
    Schema {
        /// A kind such as `Decision` or `CheckReport`, in any letter case.
        kind: Option<String>,
        /// Write the schema of every kind into this directory instead.
        #[arg(long, value_name = "DIR", conflicts_with = "kind")]
        write: Option<PathBuf>,
    },
    /// Generate or verify the Markdown docs derived from the code.
    ///
    /// The decision docs, the command reference (`cli.md`), the tool reference
    /// (`mcp.md`) and the agent skill.
    Docs {
        #[command(subcommand)]
        command: DocsCommand,
    },
    /// Print the completions of a shell.
    Completions {
        #[arg(value_enum)]
        shell: CompletionShell,
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
    /// Trust this project to run the commands it names.
    ///
    /// The programs the project names: the command checks and fixes of its own
    /// decisions, its command models, `[fix] verify` and its plugins. Trust is yours,
    /// kept in `$XDG_CONFIG_HOME/lighthouse/trust.toml` (default
    /// `~/.config/lighthouse/trust.toml`) for this root and the exact
    /// `lighthouse.toml`, `.lighthouse/decisions` and personal config files
    /// (`config.toml`, `.lighthouse/local.toml`) you have read; a change to
    /// any of them withdraws it. Trust is kept per git repository: a worktree
    /// whose inputs are byte-identical to a trusted root of the same repository
    /// is trusted too, and `--revoke` withdraws this root only.
    /// `LIGHTHOUSE_TRUST=1` trusts every project, for CI.
    /// Without trust none of them runs: a command fix is declined, a command
    /// check is unavailable and `[fix] verify` fails; the bundled checks still run.
    ///
    /// It lists the programs it would trust and asks for
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
    /// Serve the Model Context Protocol on stdio, for coding agents.
    ///
    /// Tools to check, fix, review and author decisions, and resources for the
    /// catalog. Judgments recorded through it are attributed to a
    /// `SoftwareAgent` (the id is $LIGHTHOUSE_ATTRIBUTED_TO, else the client's
    /// name).
    Mcp,
    /// Keep the project loaded in a process that `check` and the hooks run in.
    ///
    /// The server listens on a socket of the project that only you can use.
    /// `check` and the agent hooks run their command there when it answers and
    /// in their own process when it does not, with the same output and exit
    /// code. It ends after `--idle` without a command; the first
    /// SessionStart hook of a session starts one. `--no-serve` runs one in its
    /// own process.
    Serve {
        /// How long without a command ends the server: a number with `ms`,
        /// `s`, `m` or `h`.
        #[arg(long, value_parser = duration, default_value = "30m")]
        idle: Duration,
        #[command(subcommand)]
        command: Option<ServeCommand>,
    },
    /// What the sandbox starts inside its namespace; not for people.
    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        command: InternalCommand,
    },
    /// Entry points for agent hooks. Reads the hook payload on stdin.
    ///
    /// For Claude Code, see docs/agents.md.
    Hook {
        #[arg(value_enum)]
        agent: Agent,
        #[arg(value_enum)]
        event: hook::Event,
        /// Say that the analysis was incomplete but do not block on it.
        #[arg(long)]
        allow_incomplete: bool,
        /// Run the hook in this process even when a `lighthouse serve` is
        /// running for the project.
        #[arg(
            long,
            env = "LIGHTHOUSE_NO_SERVE",
            value_parser = BoolishValueParser::new(),
            action = ArgAction::SetTrue
        )]
        no_serve: bool,
    },
}

/// The coding agents Lighthouse sets up and answers the hooks of.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Agent {
    ClaudeCode,
}

/// The shells `completions` writes for.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

impl From<CompletionShell> for Shell {
    fn from(shell: CompletionShell) -> Self {
        match shell {
            CompletionShell::Bash => Self::Bash,
            CompletionShell::Zsh => Self::Zsh,
            CompletionShell::Fish => Self::Fish,
            CompletionShell::Powershell => Self::PowerShell,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum InternalCommand {
    /// Forward a loopback port to the proxy socket of the sandbox, then run
    /// the program with everything after `--`.
    Bridge {
        socket: PathBuf,
        port: u16,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServeCommand {
    /// End the server of the project.
    Stop,
    /// Say whether a server answers for the project.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Delete the result cache and what the language plugins keep in it.
    ///
    /// Nothing but time is lost: the next check fills it again.
    Clean,
}

#[derive(Debug, Subcommand)]
pub enum SpecCommand {
    /// Check every spec document under PATHS against its schema and the rest.
    ///
    /// Default PATHS: the project's configuration and `.lighthouse/decisions`.
    /// Each document is checked against the schema of its kind, and against the
    /// others: presets name decisions that exist, `extends` names a decision,
    /// CEL compiles, examples are well formed. Exits 1 when anything is wrong.
    Validate {
        paths: Vec<PathBuf>,
        /// Also run the examples of the decisions found, through the whole
        /// engine.
        #[arg(long)]
        examples: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DocsCommand {
    /// Write the decision docs, the command and tool references, and the skill.
    Generate {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
        /// Where the generated agent skill goes.
        #[arg(long, default_value = SKILL_PATH)]
        skill: PathBuf,
    },
    /// Exit 1 when a generated doc or the agent skill is stale.
    Check {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
        #[arg(long, default_value = SKILL_PATH)]
        skill: PathBuf,
    },
}

/// A span of time as `30m` writes it.
fn duration(text: &str) -> Result<Duration, String> {
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| format!("`{text}` is not a number with a unit (ms, s, m, h)"))?;
    let millis = match unit {
        "ms" => 1,
        "s" => 1000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => {
            return Err(format!(
                "`{text}` is not a number with a unit (ms, s, m, h)"
            ));
        }
    };
    Ok(Duration::from_millis(number.saturating_mul(millis)))
}
