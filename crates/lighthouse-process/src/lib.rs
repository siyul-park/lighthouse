//! Running an external program under a time limit and an output cap. The
//! program leads its own process group on unix, so a timeout takes
//! everything it started down with it.

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

/// What a finished program left behind.
#[derive(Debug)]
pub struct Output {
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
pub struct Spec<'a> {
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

/// Runs the program of `spec`. When it outlives `limit` its process group gets
/// SIGTERM, then SIGKILL after `grace`; that is an error.
pub fn run(spec: &Spec) -> Result<Output, String> {
    let (program, args) = spec.argv.split_first().ok_or("no program")?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(spec.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = spec.env {
        command.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start `{program}`: {e}"))?;
    let mut input = child.stdin.take().ok_or("no stdin")?;
    let payload = spec.stdin.to_vec();
    let writer = thread::spawn(move || {
        let _ = input.write_all(&payload);
    });
    let stdout = capture(child.stdout.take(), spec.max_output);
    let stderr = capture(child.stderr.take(), spec.max_output);
    let started = Instant::now();
    let status = loop {
        match exited(&mut child) {
            Ok(Some(())) => {
                // The leader has exited but is not yet collected, so its group
                // id is still ours: take down whatever it left running before
                // collecting it, so a daemon holding the pipes cannot hang the
                // run.
                kill_group(&child);
                match child.wait() {
                    Ok(status) => break status,
                    Err(e) => return Err(e.to_string()),
                }
            }
            Ok(None) if started.elapsed() >= spec.limit => {
                stop(&mut child, spec.grace);
                return Err(format!("timed out after {}s", spec.limit.as_secs()));
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
    let (stderr, cut_err) = stderr.finish();
    Ok(Output {
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
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: killpg only signals the group made at spawn.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Asks the program's process group to stop, then kills the whole group after
/// `grace` whether or not the leader obeyed, and collects the leader.
fn stop(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: killpg only signals the group made at spawn.
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        let asked = Instant::now();
        while asked.elapsed() < grace {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(POLL);
        }
        // SAFETY: as above.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = grace;
    reap(child);
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

/// Kills the process group the program led.
fn kill_group(child: &Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // SAFETY: killpg only signals the group made at spawn.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = child;
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
