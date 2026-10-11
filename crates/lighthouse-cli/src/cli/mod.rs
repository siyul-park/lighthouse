//! The terminal frontend: the command line, its setup and housekeeping
//! commands, the agent hooks, and the printing of what operations return.

use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    process::ExitCode,
};

use clap::{CommandFactory, FromArgMatches};
use command::{Cli, Command, DocsCommand, HookAgent, SpecCommand};

use lighthouse_cli::surface::{Context, Reply, reply::USAGE};

mod color;
mod command;
mod docs;
mod hook;
mod project;
mod reference;
mod setup;
mod spec;

type Result<T> = lighthouse_session::Result<T>;

/// Parses the command line and runs it; the exit code of the process.
pub fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().collect();
    let matches = match Cli::command()
        .color(color::choice(&args))
        .try_get_matches_from(&args)
    {
        Ok(matches) => matches,
        Err(e) => e.exit(),
    };
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    match run(&cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("lighthouse: {e}");
            ExitCode::from(USAGE)
        }
    }
}

fn run(cli: &Cli) -> Result<u8> {
    let config = cli.config.as_deref();
    match &cli.command {
        Command::Surface(action) => {
            let ctx = Context::person(cli.config.clone());
            Ok(emit(&lighthouse_cli::surface::run(&ctx, action)?))
        }
        Command::Cache { .. } => project::cache_clean(),
        Command::Spec {
            command: SpecCommand::Validate { paths, examples },
        } => spec::validate(config, paths, *examples),
        Command::Schema { kind, write } => spec::schema(kind.as_deref(), write.as_deref()),
        Command::Docs { command } => match command {
            DocsCommand::Generate { out, skill } => docs::generate(config, out, skill),
            DocsCommand::Check { out, skill } => docs::check(config, out, skill),
        },
        Command::Completions { shell } => {
            reference::completions((*shell).into());
            Ok(0)
        }
        Command::Init { agent } => project::init(*agent),
        Command::Trust { revoke, yes } => project::trust(config, *revoke, *yes),
        Command::Mcp => {
            lighthouse_cli::mcp::serve()?;
            Ok(0)
        }
        Command::Hook {
            agent:
                HookAgent::ClaudeCode {
                    event,
                    allow_incomplete,
                },
        } => Ok(hook::run(*event, *allow_incomplete)),
    }
}

/// Data to stdout, notes to stderr; the exit code of the reply. A closed pipe
/// ends the output quietly, like any filter.
fn emit(reply: &Reply) -> u8 {
    for note in &reply.notes {
        eprintln!("{note}");
    }
    let mut out = io::stdout().lock();
    let _ = out.write_all(reply.stdout.as_bytes());
    let _ = out.flush();
    reply.code
}
