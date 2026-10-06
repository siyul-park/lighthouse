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
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        #[arg(long, default_value = "text")]
        format: Format,
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
            allow_incomplete,
            rules,
            config,
        } => check(
            &paths,
            Options {
                format,
                strict,
                allow_incomplete,
            },
            &rules,
            config.as_deref(),
        ),
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

struct Options {
    format: Format,
    strict: bool,
    allow_incomplete: bool,
}

fn check(
    paths: &[PathBuf],
    options: Options,
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
    let mut registry = lighthouse_builtin::registry();
    let plugins = lighthouse_rpc::register(
        &mut registry,
        &config,
        &root,
        &lighthouse_rpc::search_dirs(&root),
    )?;
    let outcome = Engine::new(registry, config, &root)?
        .with_incomplete(plugins.incomplete)
        .check(paths, only)?;
    for notice in &plugins.notices {
        eprintln!("lighthouse: {notice}");
    }
    for notice in &outcome.notices {
        eprintln!("lighthouse: {notice}");
    }
    print!(
        "{}",
        render(options.format, &outcome.diagnostics, &outcome.incomplete)
    );
    Ok(outcome.exit_code(options.strict, options.allow_incomplete))
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
