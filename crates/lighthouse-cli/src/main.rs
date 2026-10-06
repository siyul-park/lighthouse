use std::{env, error::Error, fs, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use lighthouse_config::{Config, FILE_NAME};
use lighthouse_engine::Engine;
use lighthouse_report::{Format, render};

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
    },
    /// Inspect bundled rules.
    Rule {
        #[command(subcommand)]
        command: RuleCommand,
    },
    /// Show a rule's description, requirements and docs.
    Explain { rule_id: String },
    /// Write a default lighthouse.toml in the current directory.
    Init,
}

#[derive(Subcommand)]
enum RuleCommand {
    /// List bundled rules.
    List,
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
        } => check(&paths, format, strict, &rules),
        Command::Rule {
            command: RuleCommand::List,
        } => {
            for rule in lighthouse_builtin::registry().rules() {
                let meta = rule.meta();
                println!("{}\t{}\t{}", meta.id, meta.severity, meta.description);
            }
            Ok(0)
        }
        Command::Explain { rule_id } => explain(&rule_id),
        Command::Init => init(),
    }
}

fn check(paths: &[PathBuf], format: Format, strict: bool, only: &[String]) -> Result<u8> {
    let (path, config) = Config::discover(&env::current_dir()?)?
        .ok_or_else(|| format!("no {FILE_NAME} found (run `lighthouse init`)"))?;
    let root = path.parent().ok_or("config path has no parent")?;
    let outcome = Engine::new(lighthouse_builtin::registry(), config, root)?.check(paths, only)?;
    for notice in &outcome.notices {
        eprintln!("lighthouse: {notice}");
    }
    print!("{}", render(format, &outcome.diagnostics));
    Ok(outcome.exit_code(strict))
}

fn explain(rule_id: &str) -> Result<u8> {
    let registry = lighthouse_builtin::registry();
    let rule = registry
        .rule(rule_id)
        .ok_or_else(|| format!("unknown rule `{rule_id}`"))?;
    let meta = rule.meta();
    println!(
        "{}  (default: {})\n\n{}\n\n{}",
        meta.id, meta.severity, meta.description, meta.docs
    );
    if !meta.analyzers.is_empty() {
        println!("\nAnalyzers: {}", meta.analyzers.join(", "));
    }
    println!("Scope: {:?}", meta.scope);
    if !meta.capabilities.is_empty() {
        let caps: Vec<_> = meta.capabilities.iter().map(ToString::to_string).collect();
        println!("Capabilities: {}", caps.join(", "));
    }
    if let Some(citation) = &meta.citation {
        println!("Method: {citation}");
    }
    Ok(0)
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
