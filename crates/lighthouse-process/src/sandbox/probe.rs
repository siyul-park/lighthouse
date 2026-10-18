//! Whether the backend of this machine works: one trivial program, run under
//! it once, the first time a sandbox is needed.

use std::{
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
};

use super::{Plan, backend};

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
pub(super) fn available() -> Result<(), String> {
    let simulated = SIMULATED
        .lock()
        .ok()
        .and_then(|simulated| simulated.clone());
    match simulated {
        Some(reason) => Err(reason),
        None => TRIAL.get_or_init(trial).clone(),
    }
}

fn trial() -> Result<(), String> {
    let wrapped = backend::wrap(&Plan::default(), &["true".to_owned()]);
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
