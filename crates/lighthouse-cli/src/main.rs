use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use lighthouse_config::FILE_NAME;
use lighthouse_engine::Engine;
use lighthouse_report::{Format, render};
use session::Session;

mod docs;
mod rules;
mod scope;
mod session;

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

struct Options {
    format: Format,
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
                strict,
                allow_incomplete,
            },
            &rules,
            config.as_deref(),
        ),
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
            DocsCommand::Generate { out } => docs::generate(&out),
            DocsCommand::Check { out } => docs::check(&out),
        },
        Command::Init => init(),
    }
}

fn check(
    paths: &[PathBuf],
    reported: Reported,
    options: Options,
    only: &[String],
    config: Option<&Path>,
) -> Result<u8> {
    let session = Session::load(config)?;
    let (registry, plugins) = session.registry()?;
    let root = session.root.clone();
    let engine = Engine::new(registry, session.config, &root)?.with_incomplete(plugins.incomplete);
    let files = match (reported.changed, reported.diff) {
        (true, _) => Some(scope::changed(&root)?),
        (_, Some(base)) => Some(scope::since(&root, base)?),
        _ => None,
    };
    let outcome = match files {
        Some(files) => {
            let files = within(&root, paths, files)?;
            eprintln!(
                "lighthouse: reporting {} changed file(s); the whole project is analyzed",
                files.len()
            );
            engine.check_files(&files, only)?
        }
        None => {
            let default = [PathBuf::from(".")];
            let paths = if paths.is_empty() {
                &default[..]
            } else {
                paths
            };
            engine.check(paths, only)?
        }
    };
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

/// The changed files that also lie under the given paths, when there are any.
fn within(root: &Path, paths: &[PathBuf], files: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Ok(files);
    }
    let root = root.canonicalize()?;
    let mut scopes = Vec::new();
    for path in paths {
        let absolute = path.canonicalize()?;
        scopes.push(
            absolute
                .strip_prefix(&root)
                .map(Path::to_owned)
                .map_err(|_| {
                    format!(
                        "{} is outside the project root {}",
                        path.display(),
                        root.display()
                    )
                })?,
        );
    }
    Ok(files
        .into_iter()
        .filter(|f| scopes.iter().any(|s| f.starts_with(s)))
        .collect())
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
