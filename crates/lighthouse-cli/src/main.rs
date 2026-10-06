use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use lighthouse_config::{Config, FILE_NAME};
use lighthouse_engine::Engine;
use lighthouse_report::{Format, render};

mod docs;
mod rules;

const DEFAULT_CONFIG: &str = "plugins = [\"core\"]\nextends = [\"core/recommended\"]\n";

#[derive(Parser)]
#[command(name = "lighthouse", version, about = "Design-quality linter")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check files and print diagnostics.
    Check {
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "text")]
        format: Format,
        /// Fail on warnings too.
        #[arg(long)]
        strict: bool,
        /// Run only these fully qualified rule ids.
        #[arg(long, value_delimiter = ',')]
        rules: Vec<String>,
        /// Use this config file; the project root is then the current directory.
        #[arg(long)]
        config: Option<PathBuf>,
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
    Init,
}

#[derive(Subcommand)]
enum RuleCommand {
    /// List bundled rules.
    List {
        /// Include every catalog pattern with its implementation status.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum DocsCommand {
    /// Write the pattern docs.
    Generate {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
    },
    /// Exit 1 when the pattern docs are stale.
    Check {
        #[arg(long, default_value = "docs")]
        out: PathBuf,
    },
}

type Result<T> = std::result::Result<T, Box<dyn Error>>;

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
            format,
            strict,
            rules,
            config,
        } => check(&paths, format, strict, &rules, config.as_deref()),
        Command::Rule {
            command: RuleCommand::List { all },
        } => rules::list(all),
        Command::Explain { id } => rules::explain(&id),
        Command::Docs { command } => match command {
            DocsCommand::Generate { out } => docs::generate(&out),
            DocsCommand::Check { out } => docs::check(&out),
        },
        Command::Init => init(),
    }
}

fn check(
    paths: &[PathBuf],
    format: Format,
    strict: bool,
    only: &[String],
    config: Option<&Path>,
) -> Result<u8> {
    let (config, root) = match config {
        Some(path) => (Config::load(path)?, env::current_dir()?),
        None => {
            let (path, config) = Config::discover(&env::current_dir()?)?
                .ok_or_else(|| format!("no {FILE_NAME} found (run `lighthouse init`)"))?;
            let root = path.parent().ok_or("config path has no parent")?.to_owned();
            (config, root)
        }
    };
    let outcome = Engine::new(lighthouse_builtin::registry(), config, &root)?.check(paths, only)?;
    for notice in &outcome.notices {
        eprintln!("lighthouse: {notice}");
    }
    print!("{}", render(format, &outcome.diagnostics));
    Ok(outcome.exit_code(strict))
}

fn init() -> Result<u8> {
    let path = env::current_dir()?.join(FILE_NAME);
    if path.exists() {
        return Err(format!("{} already exists", path.display()).into());
    }
    fs::write(&path, DEFAULT_CONFIG)?;
    println!("wrote {}", path.display());
    Ok(0)
}
