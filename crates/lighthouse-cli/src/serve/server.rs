//! The server: accepts connections of its own user, runs one command at a
//! time, and ends after it has been idle for a while, or when told to.

use std::{
    io::{self, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use lighthouse_protocol::{ErrorObject, Id, Message, read_message, write_message};

use crate::serve::{
    peer::{self, permits},
    socket::Socket,
    wire::{Hello, INITIALIZE, Output, RUN, Refusal, Run, SHUTDOWN},
};

/// How often the server looks at whether it has been idle long enough.
const WATCH: Duration = Duration::from_millis(100);
/// LSP: the method is not one the server runs.
const METHOD_NOT_FOUND: i64 = -32601;
/// LSP: the parameters are not those of the method.
const INVALID_PARAMS: i64 = -32602;
/// LSP: the request failed inside the server.
const INTERNAL_ERROR: i64 = -32603;

/// What a server runs.
pub trait Runner: Sync {
    /// Runs the command, or says why it is not run here.
    ///
    /// # Errors
    ///
    /// The command is not one this server runs.
    fn run(&self, run: &Run) -> Result<Output, Refusal>;
}

/// What a server did until it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Another server holds the socket of the project.
    Already,
    /// It was idle for its time, or told to shut down.
    Done,
}

/// What the connections of a server share.
struct Shared<'r, R> {
    hello: &'r Hello,
    runner: &'r R,
    /// Commands run one at a time.
    serial: Mutex<()>,
    /// Connections that are in the middle of a command.
    running: AtomicUsize,
    stopping: AtomicBool,
    last: Mutex<Instant>,
}

/// Serves the project at `socket` with `runner`, which says it is `hello` to
/// a client, until it has been idle for `idle` or a client tells it to shut
/// down.
///
/// # Errors
///
/// The socket could not be claimed.
pub fn serve<R: Runner>(
    socket: &Path,
    idle: Duration,
    (hello, runner): (&Hello, &R),
) -> io::Result<Ended> {
    let Some(claimed) = Socket::claim(socket)? else {
        return Ok(Ended::Already);
    };
    let shared = Shared {
        hello,
        runner,
        serial: Mutex::new(()),
        running: AtomicUsize::new(0),
        stopping: AtomicBool::new(false),
        last: Mutex::new(Instant::now()),
    };
    thread::scope(|scope| {
        scope.spawn(|| watch(&shared, &claimed, idle));
        for stream in claimed.listener().incoming() {
            if shared.stopping.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            if peer::uid(&stream).is_ok_and(|uid| permits(uid, peer::own())) {
                scope.spawn(|| {
                    let _ = connection(&shared, &claimed, stream);
                });
            }
        }
    });
    claimed.release();
    Ok(Ended::Done)
}

/// Ends the server when it has had no command for `idle`.
fn watch<R: Runner>(shared: &Shared<'_, R>, claimed: &Socket, idle: Duration) {
    while !shared.stopping.load(Ordering::SeqCst) {
        thread::sleep(WATCH);
        let quiet = shared.running.load(Ordering::SeqCst) == 0
            && shared.last.lock().is_ok_and(|last| last.elapsed() >= idle);
        if quiet {
            stop(shared, claimed);
        }
    }
}

/// Ends the server: the accepting loop is woken to see it.
fn stop<R: Runner>(shared: &Shared<'_, R>, claimed: &Socket) {
    shared.stopping.store(true, Ordering::SeqCst);
    let _ = UnixStream::connect(claimed.path());
    claimed.release();
}

fn connection<R: Runner>(
    shared: &Shared<'_, R>,
    claimed: &Socket,
    stream: UnixStream,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    while let Ok(Some(request)) = read_message(&mut reader) {
        let Some(id) = request.id.clone() else {
            continue;
        };
        let answer = match request.method.as_deref() {
            Some(INITIALIZE) => reply(id, shared.hello),
            Some(RUN) => run(shared, id, request),
            Some(SHUTDOWN) => {
                let answer = reply(id, ());
                send(&mut writer, &answer)?;
                stop(shared, claimed);
                return Ok(());
            }
            _ => failure(id, METHOD_NOT_FOUND, "no such method"),
        };
        send(&mut writer, &answer)?;
    }
    Ok(())
}

/// Runs the command of `request` and answers with its output.
fn run<R: Runner>(shared: &Shared<'_, R>, id: Id, request: Message) -> Message {
    let params: Run = match request.into_params() {
        Ok(params) => params,
        Err(e) => return failure(id, INVALID_PARAMS, &e.to_string()),
    };
    shared.running.fetch_add(1, Ordering::SeqCst);
    let ran = {
        let _turn = shared.serial.lock().unwrap_or_else(|e| e.into_inner());
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.runner.run(&params)))
    };
    if let Ok(mut last) = shared.last.lock() {
        *last = Instant::now();
    }
    shared.running.fetch_sub(1, Ordering::SeqCst);
    match ran {
        Ok(Ok(output)) => reply(id, output),
        Ok(Err(Refusal(why))) => failure(id, METHOD_NOT_FOUND, &why),
        Err(_) => failure(id, INTERNAL_ERROR, "the command panicked"),
    }
}

fn reply(id: Id, result: impl serde::Serialize) -> Message {
    Message::response(id.clone(), result)
        .unwrap_or_else(|e| failure(id, INTERNAL_ERROR, &e.to_string()))
}

fn failure(id: Id, code: i64, message: &str) -> Message {
    Message::failure(
        Some(id),
        ErrorObject {
            code,
            message: message.to_owned(),
            data: None,
        },
    )
}

fn send(writer: &mut UnixStream, message: &Message) -> io::Result<()> {
    write_message(writer, message).map_err(io::Error::other)?;
    writer.flush()
}
