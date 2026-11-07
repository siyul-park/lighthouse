//! Starting a program, and the handle the run waits on and signals. A program
//! leads a process group of its own on unix, which is how its whole tree is
//! stopped. The macOS sandbox gives it a session as well, so that it has no
//! terminal to write to; a session leader leads its group, so both are stopped
//! the same way. (Bubblewrap makes the session itself.)

use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
};

/// A started program: what is waited for, asked about and killed.
pub(crate) trait Running {
    fn id(&self) -> u32;
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
    fn wait(&mut self) -> io::Result<ExitStatus>;
    fn kill(&mut self) -> io::Result<()>;
}

impl Running for Child {
    fn id(&self) -> u32 {
        Self::id(self)
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Self::try_wait(self)
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        Self::wait(self)
    }

    fn kill(&mut self) -> io::Result<()> {
        Self::kill(self)
    }
}

/// The ends of a program's standard streams that are ours.
pub(crate) struct Pipes {
    pub(crate) stdin: Box<dyn Write + Send>,
    pub(crate) stdout: Box<dyn Read + Send>,
    pub(crate) stderr: Box<dyn Read + Send>,
}

/// Starts `argv` in `dir` with exactly `env`, its streams piped. With
/// `session` (the macOS sandbox) the program also gets a session of its own,
/// and `argv[0]` is the path of a program, not a name to look up.
pub(crate) fn start(
    argv: &[String],
    dir: &Path,
    env: &BTreeMap<OsString, OsString>,
    session: bool,
) -> io::Result<(Box<dyn Running>, Pipes)> {
    #[cfg(target_os = "macos")]
    if session {
        let (spawned, pipes) = posix::spawn(argv, dir, env)?;
        return Ok((Box::new(spawned), pipes));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = session;
    let (launcher, args) = argv.split_first().ok_or(io::ErrorKind::InvalidInput)?;
    let mut command = Command::new(launcher);
    command
        .args(args)
        .current_dir(dir)
        .env_clear()
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let pipes = Pipes {
        stdin: Box::new(child.stdin.take().ok_or(io::ErrorKind::BrokenPipe)?),
        stdout: Box::new(child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?),
        stderr: Box::new(child.stderr.take().ok_or(io::ErrorKind::BrokenPipe)?),
    };
    Ok((Box::new(child), pipes))
}

/// `posix_spawn` with `POSIX_SPAWN_SETSID`: the program gets its session in
/// the spawn, so a process with a large memory is not forked to give it one.
#[cfg(target_os = "macos")]
mod posix {
    use super::{Pipes, Running};
    use std::{
        collections::BTreeMap,
        ffi::{CString, OsString},
        io,
        os::{
            fd::{AsRawFd, OwnedFd},
            unix::{ffi::OsStrExt, process::ExitStatusExt},
        },
        path::Path,
        process::ExitStatus,
        ptr,
    };

    /// `POSIX_SPAWN_SETSID` of `<sys/spawn.h>`, which the `libc` crate lacks:
    /// macOS 10.15 and later.
    const SETSID: libc::c_int = 0x0400;

    /// A session; no descriptor but those of the file actions; no signal
    /// blocked, and SIGPIPE back to its default, which Rust ignores in its own
    /// process and `exec` keeps.
    const FLAGS: libc::c_short = (SETSID
        | libc::POSIX_SPAWN_CLOEXEC_DEFAULT
        | libc::POSIX_SPAWN_SETSIGDEF
        | libc::POSIX_SPAWN_SETSIGMASK) as libc::c_short;

    /// A program started by [`spawn`], collected with `waitpid`.
    pub(super) struct Spawned {
        pid: libc::pid_t,
        status: Option<ExitStatus>,
    }

    impl Spawned {
        /// Collects the program, or, with `WNOHANG`, finds it still running.
        fn collect(&mut self, options: libc::c_int) -> io::Result<Option<ExitStatus>> {
            if self.status.is_some() {
                return Ok(self.status);
            }
            let mut raw = 0;
            loop {
                // SAFETY: waitpid writes only `raw`, for the child `pid`.
                match unsafe { libc::waitpid(self.pid, &mut raw, options) } {
                    0 => return Ok(None),
                    n if n > 0 => break,
                    _ => match io::Error::last_os_error() {
                        e if e.kind() == io::ErrorKind::Interrupted => {}
                        e => return Err(e),
                    },
                }
            }
            self.status = Some(ExitStatus::from_raw(raw));
            Ok(self.status)
        }
    }

    impl Running for Spawned {
        fn id(&self) -> u32 {
            self.pid.unsigned_abs()
        }

        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.collect(libc::WNOHANG)
        }

        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.collect(0)?.ok_or_else(io::Error::last_os_error)
        }

        fn kill(&mut self) -> io::Result<()> {
            if self.status.is_some() {
                return Ok(());
            }
            // SAFETY: kill only signals the child that has not been collected.
            match unsafe { libc::kill(self.pid, libc::SIGKILL) } {
                0 => Ok(()),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }

    /// Spawns `argv` in `dir` with exactly `env`: a session of its own, the
    /// three standard streams piped (the child's ends are closed here once it
    /// has them), every other descriptor closed.
    pub(super) fn spawn(
        argv: &[String],
        dir: &Path,
        env: &BTreeMap<OsString, OsString>,
    ) -> io::Result<(Spawned, Pipes)> {
        let args = strings(argv.iter().map(|a| a.as_bytes().to_vec()))?;
        let vars = strings(
            env.iter()
                .map(|(k, v)| [k.as_bytes(), b"=", v.as_bytes()].concat()),
        )?;
        let dir = strings([dir.as_os_str().as_bytes().to_vec()])?;
        let (stdin, input) = io::pipe()?;
        let (output, stdout) = io::pipe()?;
        let (error, stderr) = io::pipe()?;
        let ours: [OwnedFd; 3] = [stdin.into(), stdout.into(), stderr.into()];
        let pid = spawn_with(&args, &vars, &dir[0], &ours)?;
        let pipes = Pipes {
            stdin: Box::new(input),
            stdout: Box::new(output),
            stderr: Box::new(error),
        };
        Ok((Spawned { pid, status: None }, pipes))
    }

    /// Runs `posix_spawn` of the first of `args`, giving the child `streams` as
    /// descriptors 0, 1 and 2.
    fn spawn_with(
        args: &[CString],
        vars: &[CString],
        dir: &CString,
        streams: &[OwnedFd; 3],
    ) -> io::Result<libc::pid_t> {
        let program = args.first().ok_or(io::ErrorKind::InvalidInput)?;
        let mut attr: libc::posix_spawnattr_t = ptr::null_mut();
        let mut actions: libc::posix_spawn_file_actions_t = ptr::null_mut();
        // SAFETY: init fills a handle that destroy releases; a null one is refused.
        let spawned = check(unsafe { libc::posix_spawnattr_init(&mut attr) })
            .and_then(|()| check(unsafe { libc::posix_spawn_file_actions_init(&mut actions) }))
            .and_then(|()| configure(&mut attr, &mut actions, dir, streams))
            .and_then(|()| {
                let mut pid = 0;
                // SAFETY: the strings outlive the call; the arrays end in null.
                check(unsafe {
                    libc::posix_spawn(
                        &mut pid,
                        program.as_ptr(),
                        &actions,
                        &attr,
                        terminated(args).as_ptr(),
                        terminated(vars).as_ptr(),
                    )
                })
                .map(|()| pid)
            });
        // SAFETY: the handles are not used again.
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attr);
        }
        spawned
    }

    /// The attributes of [`FLAGS`], the working directory `dir`, and `streams`
    /// as descriptors 0, 1 and 2.
    fn configure(
        attr: &mut libc::posix_spawnattr_t,
        actions: &mut libc::posix_spawn_file_actions_t,
        dir: &CString,
        streams: &[OwnedFd; 3],
    ) -> io::Result<()> {
        // SAFETY: the handles were initialized by the caller.
        let calls = unsafe {
            [
                libc::posix_spawnattr_setflags(attr, FLAGS),
                libc::posix_spawnattr_setsigmask(attr, &signals(&[])),
                libc::posix_spawnattr_setsigdefault(attr, &signals(&[libc::SIGPIPE])),
                libc::posix_spawn_file_actions_addchdir_np(actions, dir.as_ptr()),
            ]
        };
        // SAFETY: as above; the descriptors are open for the calls.
        let dups = (0..).zip(streams).map(|(target, stream)| unsafe {
            libc::posix_spawn_file_actions_adddup2(actions, stream.as_raw_fd(), target)
        });
        calls.into_iter().chain(dups).try_for_each(check)
    }

    /// The set of `members`.
    fn signals(members: &[libc::c_int]) -> libc::sigset_t {
        let mut set = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: sigemptyset initializes the set, which sigaddset then fills.
        unsafe {
            libc::sigemptyset(set.as_mut_ptr());
            members
                .iter()
                .for_each(|m| _ = libc::sigaddset(set.as_mut_ptr(), *m));
            set.assume_init()
        }
    }

    /// The strings as C strings; one with a NUL byte is an error.
    fn strings(items: impl IntoIterator<Item = Vec<u8>>) -> io::Result<Vec<CString>> {
        items
            .into_iter()
            .map(|item| CString::new(item).map_err(io::Error::other))
            .collect()
    }

    /// The pointers to `strings`, ended by a null pointer, as `argv` and `envp`
    /// are.
    fn terminated(strings: &[CString]) -> Vec<*mut libc::c_char> {
        let pointers = strings.iter().map(|s| s.as_ptr().cast_mut());
        pointers.chain([ptr::null_mut()]).collect()
    }

    /// The `posix_spawn` functions return the error number, not -1.
    fn check(code: libc::c_int) -> io::Result<()> {
        match code {
            0 => Ok(()),
            code => Err(io::Error::from_raw_os_error(code)),
        }
    }
}
