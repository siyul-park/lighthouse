//! Whether the backend of this machine works: one trivial program, run under
//! it once, the first time a sandbox is needed.

use std::{
    fs,
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

use super::{Denied, Plan, backend};

/// What the first trial found, for the life of the process.
static TRIAL: OnceLock<Result<(), String>> = OnceLock::new();

/// A reason the backend is to be found unavailable, while a [`Simulation`]
/// lives.
static SIMULATED: Mutex<Option<String>> = Mutex::new(None);

/// A simulated unavailable backend; the real one is back when it is dropped.
pub struct Simulation;

impl Drop for Simulation {
    fn drop(&mut self) {
        if let Ok(mut simulated) = SIMULATED.lock() {
            *simulated = None;
        }
    }
}

/// Makes every sandboxed run in this process fail with a reason, until it is
/// dropped: the way to see what the callers do without a backend. It is
/// process-wide, so a test that holds one belongs in a test binary of its own.
pub fn simulate_unavailable(reason: &str) -> Simulation {
    if let Ok(mut simulated) = SIMULATED.lock() {
        *simulated = Some(reason.to_owned());
    }
    Simulation
}

/// Whether sandboxed programs can run here, and if not, why.
pub fn available() -> Result<(), String> {
    let simulated = SIMULATED
        .lock()
        .ok()
        .and_then(|simulated| simulated.clone());
    match simulated {
        Some(reason) => Err(reason),
        None => TRIAL.get_or_init(trial).clone(),
    }
}

/// The backend works when a program runs under it and a file it is told to
/// hide cannot be read.
fn trial() -> Result<(), String> {
    let dir = tempfile::tempdir().map_err(|e| format!("no temporary directory: {e}"))?;
    let secret = dir.path().join("secret");
    fs::write(&secret, "x").map_err(|e| e.to_string())?;
    let plan = Plan {
        deny: vec![Denied {
            path: secret.canonicalize().map_err(|e| e.to_string())?,
            dir: false,
        }],
        ..Plan::default()
    };
    under(&plan, &["true"])?;
    let shown = secret.to_string_lossy();
    match under(&plan, &["cat", &shown]) {
        Ok(()) => Err("it does not hide the files it is told to".to_owned()),
        Err(_) => Ok(()),
    }
}

/// Runs `argv` under the backend with `plan`.
fn under(plan: &Plan, argv: &[&str]) -> Result<(), String> {
    let argv: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
    let wrapped = backend::wrap(plan, &argv);
    let Some((program, args)) = wrapped.split_first() else {
        return Err("this OS has no sandbox backend".to_owned());
    };
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("`{program}`: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let first = said.lines().next().unwrap_or("it failed");
    Err(format!("`{program}`: {first}"))
}
