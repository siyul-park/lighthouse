//! The terminal frontend: the command line, its setup and housekeeping
//! commands, the agent hooks, and the printing of what operations return.

use std::{
    env,
    ffi::OsString,
    io,
    process::ExitCode,
    sync::{Arc, Mutex},
};

use clap::{CommandFactory, FromArgMatches};

use streams::Io;

use command::{Agent, CacheCommand, Cli, Command, DocsCommand, InternalCommand, SpecCommand};

use lighthouse_cli::surface::{
    Context, Reply,
    reply::{Body, USAGE},
};

mod color;

mod command;

mod docs;

mod hook;

mod paste;

mod project;

mod reference;

mod serve;

mod setup;

mod spec;

mod streams;

use lighthouse_model::Result;

use lighthouse_session::Warm;

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
    let stdin = payload(&cli);
    if let Some(code) = serve::forward(&cli, &args, stdin.as_deref()) {
        return ExitCode::from(code);
    }
    let (mut out, mut err) = (io::stdout(), io::stderr());
    let mut io = Io::new(&mut out, &mut err, stdin);
    ExitCode::from(execute(&cli, &mut io, None))
}

/// What a hook reads on its standard input; nothing for any other command.
fn payload(cli: &Cli) -> Option<String> {
    matches!(cli.command, Command::Hook { .. })
        .then(|| io::read_to_string(io::stdin()).ok())
        .flatten()
}

/// Runs the command; its exit code. A failure is told on stderr and exits 2.
pub fn execute(cli: &Cli, io: &mut Io<'_>, warm: Option<&Arc<Mutex<Warm>>>) -> u8 {
    match run(cli, io, warm) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(io.err, "lighthouse: {e}");
            USAGE
        }
    }
}

fn run(cli: &Cli, io: &mut Io<'_>, warm: Option<&Arc<Mutex<Warm>>>) -> Result<u8> {
    let config = cli.config.as_deref();
    match &cli.command {
        Command::Surface(action) => {
            let ctx = Context::person(cli.config.clone());
            let ctx = match warm {
                Some(warm) => ctx.warmed(Arc::clone(warm)),
                None => ctx,
            };
            Ok(emit(&lighthouse_cli::surface::run(&ctx, action)?, io))
        }
        Command::Cache {
            command: CacheCommand::Clean,
        } => project::cache_clean(config),
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
        Command::Init { agent } => project::init(config, *agent),
        Command::Trust { revoke, yes } => project::trust(config, *revoke, *yes),
        Command::Internal {
            command:
                InternalCommand::Bridge {
                    socket,
                    port,
                    command,
                },
        } => Ok(u8::try_from(lighthouse_process::bridge(socket, *port, command)?).unwrap_or(1)),
        Command::Mcp => {
            lighthouse_cli::mcp::serve()?;
            Ok(0)
        }
        Command::Serve { idle, command } => {
            serve::command(config, (*idle, command.as_ref()), execute)
        }
        Command::Hook {
            agent: Agent::ClaudeCode,
            event,
            allow_incomplete,
            no_serve,
        } => {
            let payload = io.stdin.take().unwrap_or_default();
            let own = Arc::new(Mutex::new(Warm::new()));
            let mut hook = hook::Hook {
                warm: warm.unwrap_or(&own),
                out: &mut *io.out,
                err: &mut *io.err,
            };
            let code = hook::run(*event, *allow_incomplete, &payload, &mut hook);
            if !no_serve {
                serve::keep_warm(*event, &payload);
            }
            Ok(code)
        }
    }
}

/// Data to stdout, notes to stderr; the exit code of the reply. A closed pipe
/// ends the output quietly, like any filter.
fn emit(reply: &Reply, io: &mut Io<'_>) -> u8 {
    for note in &reply.notes {
        let _ = writeln!(io.err, "{note}");
    }
    let _ = match &reply.body {
        Body::Text(text) => io.out.write_all(text.as_bytes()),
        Body::Json(value) => writeln!(io.out, "{value}"),
    };
    let _ = io.out.flush();
    reply.code
}
