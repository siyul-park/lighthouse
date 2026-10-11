//! The command line: global flags, the agent-loop commands of
//! `lighthouse_cli::surface`, and the commands only a terminal needs (setup,
//! housekeeping, build).

use std::path::PathBuf;

use lighthouse_cli::surface::Action;

use crate::cli::hook;
use clap::{ColorChoice, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

/// Where `docs generate` writes the agent skill.
pub const SKILL_PATH: &str = "skills/lighthouse/SKILL.md";

#[derive(Debug, Parser)]
#[command(name = "lighthouse", version, about = "Design-quality linter")]
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
    /// Command fixers and the formatters of `lighthouse.toml`. Trust is yours,
    /// kept in `$XDG_CONFIG_HOME/lighthouse/trust.toml` (default
    /// `~/.config/lighthouse/trust.toml`) for this root and the exact
    /// `lighthouse.toml` and `.lighthouse/decisions` you have read; a change to
    /// either withdraws it. `LIGHTHOUSE_TRUST=1` trusts every project, for CI.
    /// Without trust a command fix is declined and formatting is skipped;
    /// checks and the verification of fixes still run.
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
    /// Serve the Model Context Protocol on stdio, for coding agents.
    ///
    /// Tools to check, fix, review and author decisions, and resources for the
    /// catalog. Judgments recorded through it are attributed to a
    /// `SoftwareAgent` (the id is $LIGHTHOUSE_ATTRIBUTED_TO, else the client's
    /// name).
    Mcp,
    /// Entry points for agent hooks. Reads the hook payload on stdin.
    Hook {
        #[command(subcommand)]
        agent: HookAgent,
    },
}

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
pub enum HookAgent {
    /// Claude Code hooks (see docs/agents.md).
    ClaudeCode {
        #[arg(value_enum)]
        event: hook::Event,
        /// Say that the analysis was incomplete but do not block on it.
        #[arg(long)]
        allow_incomplete: bool,
    },
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
    /// others: presets name decisions that exist, fix operations name
    /// registered order keys, `extends` names a decision, CEL compiles,
    /// examples are well formed. Exits 1 when anything is wrong.
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
