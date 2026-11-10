//! The side of a command line: finds the server of the project, asks it, and
//! leaves the command to the caller when it cannot.

use std::{
    io::{BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

use lighthouse_protocol::{Id, Message, read_message, write_message};

use crate::serve::{
    socket::socket_for,
    wire::{Hello, INITIALIZE, Output, RUN, Run, SHUTDOWN, identity},
};

/// The most the log of a server holds before it is started afresh.
const LOG_CAP: u64 = 64 * 1024;

/// How long the server is waited for to answer; a command can take long, but
/// a server that does not answer `initialize` is not one to use.
const GREETING: Duration = Duration::from_secs(2);

/// A connection to the server of a project, that has told its build and is
/// the same as this one.
pub struct Connection {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: i64,
}

impl Connection {
    /// Runs the command on the server. `None` when the server does not run it
    /// or the exchange broke; the caller runs it then.
    pub fn run(&mut self, run: &Run) -> Option<Output> {
        self.ask(RUN, run).ok()
    }

    /// Tells the server to end.
    pub fn shutdown(mut self) {
        let _: Result<(), _> = self.ask(SHUTDOWN, ());
    }

    fn ask<T: serde::de::DeserializeOwned>(
        &mut self,
        method: &str,
        params: impl serde::Serialize,
    ) -> Result<T, String> {
        self.next += 1;
        let id = self.next;
        let request = Message::request(id, method, params).map_err(|e| e.to_string())?;
        write_message(&mut self.writer, &request).map_err(|e| e.to_string())?;
        self.writer.flush().map_err(|e| e.to_string())?;
        loop {
            let message = read_message(&mut self.reader)
                .map_err(|e| e.to_string())?
                .ok_or("the server closed the connection")?;
            if message.id == Some(Id::Number(id)) {
                return message
                    .into_result()
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.message);
            }
        }
    }
}

/// The server of the project at `root`, when there is one of this build.
/// A server of another build is told to shut down and not used.
pub fn connect(root: &Path) -> Option<Connection> {
    let mut connection = open(root)?;
    let hello: Hello = connection.ask(INITIALIZE, ()).ok()?;
    connection.writer.set_read_timeout(None).ok()?;
    if hello == identity() {
        Some(connection)
    } else {
        connection.shutdown();
        None
    }
}

/// The build of the server that answers for the project at `root`, if one
/// does.
pub fn status(root: &Path) -> Option<Hello> {
    open(root)?.ask(INITIALIZE, ()).ok()
}

/// Ends the server of the project at `root`; whether there was one.
pub fn stop(root: &Path) -> bool {
    open(root).is_some_and(|mut connection| connection.ask::<()>(SHUTDOWN, ()).is_ok())
}

/// Starts a server for the project at `root` when none answers: the program
/// itself, detached, that serves for `idle` and writes what it has to say to
/// `log`.
pub fn ensure(root: &Path, idle: &str, log: &Path) -> std::io::Result<()> {
    if status(root).is_some() {
        return Ok(());
    }
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(log).is_ok_and(|meta| meta.len() > LOG_CAP) {
        std::fs::remove_file(log)?;
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["serve", "--idle", idle])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out);
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().map(drop)
}

/// A connection to whatever listens at the socket of the project at `root`.
fn open(root: &Path) -> Option<Connection> {
    let stream = UnixStream::connect(socket_for(root)).ok()?;
    stream.set_read_timeout(Some(GREETING)).ok()?;
    Some(Connection {
        reader: BufReader::new(stream.try_clone().ok()?),
        writer: stream,
        next: 0,
    })
}
