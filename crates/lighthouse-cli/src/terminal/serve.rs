//! `lighthouse serve`, and the client side of it: `check` and the hooks ask
//! the server of the project first and run themselves when none answers.

use std::{
    env,
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use clap::{CommandFactory, FromArgMatches};

use lighthouse_cli::{
    serve::{self, Output, Refusal, Run, Runner, socket_for},
    surface::Action,
};

use lighthouse_model::Result;

use lighthouse_session::{Warm, config_file};

use serde_json::Value;

use crate::terminal::{
    command::{Cli, Command, ServeCommand},
    hook::{self, Event},
    streams::Io,
};

/// How long the first server of a session serves without a command.
const SESSION_IDLE: &str = "30m";

/// Where the server writes what it has to say, below the project.
const LOG: &str = ".lighthouse/cache/serve.log";

/// What the server runs the commands of its clients with: one project kept
/// loaded.
struct Commands {
    warm: Arc<Mutex<Warm>>,
    execute: Execute,
}

/// How a command is run, given the project the server keeps loaded.
pub type Execute = fn(&Cli, &mut Io<'_>, Option<&Arc<Mutex<Warm>>>) -> u8;

impl Runner for Commands {
    fn run(&self, run: &Run) -> std::result::Result<Output, Refusal> {
        let matches = Cli::command()
            .try_get_matches_from(&run.argv)
            .map_err(|e| Refusal(e.to_string()))?;
        let cli = Cli::from_arg_matches(&matches).map_err(|e| Refusal(e.to_string()))?;
        if !runs_here(&cli.command) {
            return Err(Refusal("the server does not run this command".to_owned()));
        }
        if let Some(name) = run.differs(&Run::decisive()) {
            return Err(Refusal(format!("`{name}` differs from the server's")));
        }
        env::set_current_dir(&run.cwd).map_err(|e| Refusal(e.to_string()))?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let mut io = Io::new(&mut stdout, &mut stderr, run.stdin.clone());
        let exit = (self.execute)(&cli, &mut io, Some(&self.warm));
        Ok(Output {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit,
        })
    }
}

/// Runs `lighthouse serve`, whose server runs its commands with `execute`.
pub fn command(
    config: Option<&Path>,
    (idle, command): (Duration, Option<&ServeCommand>),
    execute: Execute,
) -> Result<u8> {
    let root = lighthouse_session::project_root(config)?;
    match command {
        Some(ServeCommand::Stop) => {
            println!(
                "{}",
                if serve::stop(&root) {
                    "stopped"
                } else {
                    "not serving"
                }
            );
            Ok(0)
        }
        Some(ServeCommand::Status) => match serve::status(&root) {
            Some(hello) => {
                println!("serving {} ({})", root.display(), hello.version);
                Ok(0)
            }
            None => {
                println!("not serving");
                Ok(1)
            }
        },
        None => {
            let commands = Commands {
                warm: Arc::new(Mutex::new(Warm::new())),
                execute,
            };
            match serve::serve(&socket_for(&root), idle, (&serve::identity(), &commands))? {
                serve::Ended::Already => println!("already serving {}", root.display()),
                serve::Ended::Done => {}
            }
            Ok(0)
        }
    }
}

/// Runs the command on the server of its project when one answers: its exit
/// code. `None` when it is to run here, which is when `--no-serve` says so,
/// the command is not one the client forwards, or no server of this build
/// takes it.
pub fn forward(cli: &Cli, args: &[OsString], stdin: Option<&str>) -> Option<u8> {
    if !servable(&cli.command) {
        return None;
    }
    let argv = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    let root = root_of(cli, stdin)?;
    let mut connection = serve::connect(&root)?;
    let output = connection.run(&Run {
        argv,
        cwd: env::current_dir().ok()?,
        env: Run::decisive(),
        stdin: stdin.map(str::to_owned),
    })?;
    let _ = std::io::stderr().write_all(output.stderr.as_bytes());
    let _ = std::io::stdout().write_all(output.stdout.as_bytes());
    Some(output.exit)
}

/// Whether `command` is one the client forwards, and `--no-serve` does not
/// keep it here.
fn servable(command: &Command) -> bool {
    match command {
        Command::Surface(Action::Check(check)) => !check.process.no_serve,
        Command::Hook { no_serve, .. } => !no_serve,
        _ => false,
    }
}

/// After a SessionStart hook has run here, starts the server of the project
/// when none answers, so that the commands after it find one.
pub fn keep_warm(event: Event, payload: &str) {
    if event != Event::SessionStart {
        return;
    }
    let Some(root) = serde_json::from_str::<Value>(payload)
        .ok()
        .and_then(|payload| hook::hook_dir(&payload).ok())
        .and_then(|dir| {
            dir.ancestors()
                .find(|d| config_file(d).is_some())
                .map(Path::to_owned)
        })
    else {
        return;
    };
    if let Err(e) = serve::ensure(&root, SESSION_IDLE, &root.join(LOG)) {
        eprintln!("lighthouse hook: serve: {e}");
    }
}

/// The commands a server runs: `check`, the hooks, and the reads of reviews
/// and decisions.
fn runs_here(command: &Command) -> bool {
    use lighthouse_cli::surface::commands::{DecisionCommand, ReviewCommand};
    match command {
        Command::Surface(Action::Check(_)) | Command::Hook { .. } => true,
        Command::Surface(Action::Review { command }) => {
            matches!(
                command,
                ReviewCommand::List { .. } | ReviewCommand::Show { .. }
            )
        }
        Command::Surface(Action::Decision { command }) => {
            matches!(
                command,
                DecisionCommand::List { .. } | DecisionCommand::Show { .. }
            )
        }
        _ => false,
    }
}

/// The project root the command is about: that of its payload for a hook.
fn root_of(cli: &Cli, stdin: Option<&str>) -> Option<PathBuf> {
    let here = match (&cli.command, stdin) {
        (Command::Hook { .. }, Some(payload)) => {
            let payload: Value = serde_json::from_str(payload).ok()?;
            hook::hook_dir(&payload).ok()?
        }
        _ => env::current_dir().ok()?,
    };
    match cli.config {
        Some(_) => Some(here),
        None => here
            .ancestors()
            .find(|dir| config_file(dir).is_some())
            .map(Path::to_owned),
    }
}
