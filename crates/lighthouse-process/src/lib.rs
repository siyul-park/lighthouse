//! Running an external program under a time limit and an output cap, inside
//! an OS sandbox. The program leads its own process group on unix, so a
//! timeout takes everything it started down with it.
//!
//! Every program Lighthouse starts through [`run`] is sandboxed, and every
//! caller says how ([`Sandbox`]). Not covered: the long-lived plugin
//! processes of `lighthouse-rpc` (Lighthouse's own binaries), the version
//! control calls of `lighthouse-session`, and test support.

mod format;
mod sandbox;

pub use format::Formatter;
pub use sandbox::{
    Bridge, DENIED, Denied, Plan, Policy, Profile, Proxy, Sandbox, Simulation, allows, bridge,
    bwrap_argv, seatbelt_profile, secrets, simulate_unavailable,
};

use sandbox::{Started, runnable};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often a running program is looked at.
const POLL: Duration = Duration::from_millis(10);
/// How long the output of a program that has exited is waited for.
const DRAIN: Duration = Duration::from_millis(500);

/// What a fixer or formatter may print on each of stdout and stderr.
pub const MAX_OUTPUT: usize = 1 << 20;
/// How long a program that was asked to stop may take to do so before it is
/// killed, unless the caller says otherwise.
pub const GRACE: Duration = Duration::from_secs(2);

/// What a command that checks, fixes or formats a file is given on stdin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Stdin {
    #[default]
    None,
    /// The text of the file.
    File,
}

/// Where a command that fixes or formats a file leaves the new text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Output {
    /// In the scratch copy of the file, which is appended to `argv` (or fills
    /// an `{file}` argument).
    #[default]
    InPlace,
    /// On stdout, which carries only the text.
    Text,
}

/// What a finished program left behind.
#[derive(Debug)]
pub struct Ran {
    pub success: bool,
    /// The exit code; `None` when a signal ended the program.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// More was printed than `max_output` keeps: the rest was dropped, so
    /// what is here is not everything the program said.
    pub truncated: bool,
}

/// What to run and how: no shell, `dir` as the working directory, `stdin` on
/// its input, and the limits it lives under.
pub struct Invocation<'a> {
    pub argv: &'a [String],
    pub dir: &'a Path,
    pub stdin: &'a [u8],
    /// With `Some`, the environment is cleared to exactly these variables;
    /// with `None`, the program inherits ours.
    pub env: Option<&'a [(String, String)]>,
    pub limit: Duration,
    /// How long a program that was asked to stop (SIGTERM) may take to do so
    /// before it is killed.
    pub grace: Duration,
    /// The most each of stdout and stderr keeps; the rest is read and dropped.
    pub max_output: usize,
    /// What the program may touch. A sandbox that cannot be had is an error,
    /// never a program run without one.
    pub sandbox: Sandbox<'a>,
}

/// What a reader thread has collected so far.
struct Captured {
    bytes: Arc<Mutex<Vec<u8>>>,
    overflowed: Arc<AtomicBool>,
    reader: thread::JoinHandle<()>,
}

impl Captured {
    /// What was read, waiting a moment for the reader to reach the end of a
    /// pipe that was closed; a pipe something still holds open does not hold
    /// the run up.
    fn finish(self) -> (Vec<u8>, bool) {
        let waited = Instant::now();
        while !self.reader.is_finished() && waited.elapsed() < DRAIN {
            thread::sleep(POLL);
        }
        let bytes = self.bytes.lock().map(|b| b.clone()).unwrap_or_default();
        // A reader still running after the drain has not seen the end of the
        // output either.
        let cut = self.overflowed.load(Ordering::Relaxed) || !self.reader.is_finished();
        (bytes, cut)
    }
}

/// Runs the program of `invocation`. When it outlives `limit` its process group
/// gets SIGTERM, then SIGKILL after `grace`; that is an error.
pub fn run(invocation: &Invocation) -> Result<Ran, String> {
    let program = invocation.argv.first().ok_or("no program")?;
    let sandboxed = match invocation.sandbox {
        Sandbox::On(profile) => Some(Started::new(profile)?),
        Sandbox::Off => None,
    };
    if sandboxed.is_some() && !runnable(program, invocation.dir, invocation.env) {
        return Err(format!(
            "cannot start `{program}`: No such file or directory (os error 2)"
        ));
    }
    let argv = sandboxed
        .as_ref()
        .map_or_else(|| invocation.argv.to_vec(), |s| s.wrap(invocation.argv));
    let (launcher, args) = argv.split_first().ok_or("no program")?;
    let mut command = Command::new(launcher);
    command
        .args(args)
        .current_dir(invocation.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = invocation.env {
        command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    if let Some(sandboxed) = &sandboxed {
        command.envs(sandboxed.env());
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start `{program}`: {e}"))?;
    let mut input = child.stdin.take().ok_or("no stdin")?;
    let payload = invocation.stdin.to_vec();
    let writer = thread::spawn(move || {
        let _ = input.write_all(&payload);
    });
    let stdout = capture(child.stdout.take(), invocation.max_output);
    let stderr = capture(child.stderr.take(), invocation.max_output);
    let started = Instant::now();
    let status = loop {
        match exited(&mut child) {
            Ok(Some(())) => {
                // The leader has exited but is not yet collected, so its group
                // id is still ours: take down whatever it left running before
                // collecting it, so a daemon holding the pipes cannot hang the
                // run.
                signal_group(&child, true);
                match child.wait() {
                    Ok(status) => break status,
                    Err(e) => return Err(e.to_string()),
                }
            }
            Ok(None) if started.elapsed() >= invocation.limit => {
                stop(&mut child, invocation.grace);
                return Err(format!("timed out after {}s", invocation.limit.as_secs()));
            }
            Ok(None) => thread::sleep(POLL),
            Err(e) => {
                reap(&mut child);
                return Err(e.to_string());
            }
        }
    };
    let _ = writer.join();
    let (stdout, cut_out) = stdout.finish();
    let (mut stderr, cut_err) = stderr.finish();
    if let Some(sandboxed) = &sandboxed {
        stderr.extend(refusals(&sandboxed.blocked()));
    }
    Ok(Ran {
        success: status.success(),
        code: status.code(),
        stdout,
        stderr,
        truncated: cut_out || cut_err,
    })
}

/// Kills a program that is still running together with its process group,
/// then collects it. A program that already exited is only collected: its
/// group id may have been reused.
pub fn reap(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(None)) {
        let _ = child.wait();
        return;
    }
    signal_group(child, true);
    let _ = child.kill();
    let _ = child.wait();
}

/// The note that tells why a request of a sandboxed program failed: the hosts
/// the proxy refused.
fn refusals(hosts: &[String]) -> Vec<u8> {
    if hosts.is_empty() {
        return Vec::new();
    }
    format!(
        "lighthouse sandbox: refused network access to {}\n",
        hosts.join(", ")
    )
    .into_bytes()
}

/// Asks the program's process group to stop, then kills the whole group after
/// `grace` whether or not the leader obeyed, and collects the leader.
fn stop(child: &mut Child, grace: Duration) {
    signal_group(child, false);
    let asked = Instant::now();
    while asked.elapsed() < grace {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break;
        }
        thread::sleep(POLL);
    }
    reap(child);
}

/// Signals the process group the program led: SIGKILL when `kill`, else
/// SIGTERM.
fn signal_group(child: &Child, kill: bool) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        let signal = if kill { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: killpg only signals the group made at spawn.
        unsafe {
            libc::killpg(pid, signal);
        }
    }
    #[cfg(not(unix))]
    let _ = (child, kill);
}

/// Whether the program has exited, without collecting it: its process group
/// stays ours until it is waited for.
fn exited(child: &mut Child) -> std::io::Result<Option<()>> {
    #[cfg(unix)]
    {
        let pid: libc::id_t = child.id();
        // SAFETY: an all-zero siginfo_t is a valid value for waitid to fill.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waitid reads the pid and writes only `info`; WNOWAIT leaves
        // the child waitable.
        let status = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if status != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: after a successful waitid, si_pid says whether a child changed state.
        let changed = unsafe { info.si_pid() } != 0;
        Ok(changed.then_some(()))
    }
    #[cfg(not(unix))]
    {
        child.try_wait().map(|s| s.map(drop))
    }
}

fn capture<R: Read + Send + 'static>(pipe: Option<R>, max: usize) -> Captured {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    let overflowed = Arc::new(AtomicBool::new(false));
    let overflow = Arc::clone(&overflowed);
    let reader = thread::spawn(move || {
        let Some(mut pipe) = pipe else {
            return;
        };
        let mut chunk = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            if let Ok(mut kept) = sink.lock() {
                let room = max.saturating_sub(kept.len());
                if n > room {
                    overflow.store(true, Ordering::Relaxed);
                }
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    });
    Captured {
        bytes,
        overflowed,
        reader,
    }
}
