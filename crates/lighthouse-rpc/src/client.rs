use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use lighthouse_protocol::{Id, Message, read_message, write_message};
use serde::{Serialize, de::DeserializeOwned};

use crate::manifest::PluginManifest;

/// Most recent stderr lines kept, and the bytes kept of each.
const STDERR_LINES: usize = 50;
const STDERR_LINE_BYTES: usize = 2048;
/// Responses buffered between the reader thread and the caller.
const EVENT_QUEUE: usize = 16;
const EXIT_GRACE: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(10);

/// Why a call failed: the plugin answered with an error and stays usable, or
/// the stream is broken and the plugin is gone.
enum Failure {
    Rejected(String),
    Broken(String),
}

enum Event {
    Response(Message),
    Closed(String),
}

struct Session {
    child: Child,
    /// Frames for the writer thread, so a plugin that stops reading cannot
    /// block the caller past its deadline.
    writer: Sender<Vec<u8>>,
    events: Receiver<Event>,
    next_id: i64,
}

#[derive(Default)]
struct Stderr {
    lines: VecDeque<String>,
    dropped: usize,
}

/// A running plugin process spoken to over stdio. Calls are serialized; once
/// the process crashes, times out or sends something malformed its whole
/// process group is killed and every later call fails with the original reason.
pub(crate) struct Client {
    plugin: String,
    timeout: Duration,
    session: Mutex<Result<Session, String>>,
    stderr: Arc<Mutex<Stderr>>,
}

impl Client {
    pub(crate) fn spawn(
        manifest: &PluginManifest,
        dir: &Path,
        cwd: &Path,
        timeout: Duration,
    ) -> Result<Self, String> {
        let command = manifest.command_path(dir);
        let mut process = Command::new(&command);
        process
            .args(&manifest.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut process, 0);
        let mut child = process
            .spawn()
            .map_err(|e| format!("cannot start `{}`: {e}", command.display()))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr_pipe = child.stderr.take().ok_or("no stderr")?;

        let (writer, frames) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            for frame in frames {
                if stdin
                    .write_all(&frame)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });

        let (tx, events) = mpsc::sync_channel(EVENT_QUEUE);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let event = match read_message(&mut reader) {
                    Ok(Some(m)) if m.method.is_none() && m.id.is_some() => Event::Response(m),
                    Ok(Some(_)) => continue,
                    Ok(None) => Event::Closed("closed its output".to_owned()),
                    Err(e) => Event::Closed(e.to_string()),
                };
                let last = matches!(event, Event::Closed(_));
                if tx.send(event).is_err() || last {
                    break;
                }
            }
        });

        let stderr = Arc::new(Mutex::new(Stderr::default()));
        let sink = Arc::clone(&stderr);
        thread::spawn(move || collect_stderr(BufReader::new(stderr_pipe), &sink));

        Ok(Self {
            plugin: manifest.id.clone(),
            timeout,
            session: Mutex::new(Ok(Session {
                child,
                writer,
                events,
                next_id: 1,
            })),
            stderr,
        })
    }

    /// Sends a request and waits for its response. The `Err` side is a reason
    /// the plugin is unusable or refused, already prefixed with its id.
    pub(crate) fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, String> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| "client poisoned".to_owned())?;
        let session = guard.as_mut().map_err(|reason| reason.clone())?;
        match self.exchange(session, method, params) {
            Ok(result) => Ok(result),
            Err(Failure::Rejected(reason)) => Err(reason),
            Err(Failure::Broken(reason)) => {
                if let Ok(mut dead) = std::mem::replace(&mut *guard, Err(reason.clone())) {
                    reap(&mut dead.child);
                }
                Err(reason)
            }
        }
    }

    fn exchange<P: Serialize, R: DeserializeOwned>(
        &self,
        session: &mut Session,
        method: &str,
        params: P,
    ) -> Result<R, Failure> {
        let id = session.next_id;
        session.next_id += 1;
        let request = Message::request(id, method, params)
            .map_err(|e| Failure::Rejected(self.say(&e.to_string())))?;
        let mut frame = Vec::new();
        write_message(&mut frame, &request)
            .map_err(|e| Failure::Rejected(self.say(&e.to_string())))?;
        let deadline = Instant::now() + self.timeout;
        if session.writer.send(frame).is_err() {
            let note = self.exit_note(session);
            return Err(self.broken(&format!("cannot send `{method}`{note}")));
        }
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match session.events.recv_timeout(left) {
                Ok(Event::Response(message)) if message.id == Some(Id::Number(id)) => {
                    return match message.into_result::<R>() {
                        Ok(Ok(result)) => Ok(result),
                        Ok(Err(e)) => Err(Failure::Rejected(
                            self.say(&format!("rejected `{method}` ({}): {}", e.code, e.message)),
                        )),
                        Err(e) => {
                            Err(self.broken(&format!("sent a malformed `{method}` result: {e}")))
                        }
                    };
                }
                Ok(Event::Response(_)) => {}
                Ok(Event::Closed(reason)) => {
                    let note = self.exit_note(session);
                    return Err(self.broken(&format!("{reason}{note}")));
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(self.broken(&format!(
                        "timed out after {}s waiting for `{method}` (a cold build cache or a \
                         large project can need longer; raise `timeout` in the plugin entry)",
                        self.timeout.as_secs()
                    )));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.broken("closed its output"));
                }
            }
        }
    }

    fn broken(&self, reason: &str) -> Failure {
        Failure::Broken(self.say(reason))
    }

    fn say(&self, reason: &str) -> String {
        format!("plugin `{}` {reason}", self.plugin)
    }

    fn exit_note(&self, session: &mut Session) -> String {
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            match session.child.try_wait() {
                Ok(Some(status)) => return format!(" ({status})"),
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
                _ => return String::new(),
            }
        }
    }

    /// The most recent stderr lines since the last call, as notices, with a
    /// count of what the bounded buffer dropped.
    pub(crate) fn drain_stderr(&self) -> Vec<String> {
        let taken = self
            .stderr
            .lock()
            .map(|mut s| std::mem::take(&mut *s))
            .unwrap_or_default();
        let mut notices = Vec::new();
        if taken.dropped > 0 {
            notices.push(format!(
                "plugin `{}` stderr: {} earlier line(s) dropped",
                self.plugin, taken.dropped
            ));
        }
        notices.extend(
            taken
                .lines
                .iter()
                .map(|line| format!("plugin `{}` stderr: {line}", self.plugin)),
        );
        notices
    }
}

/// Keeps the last lines, each cut to a byte limit; reads never buffer more
/// than one limited line.
fn collect_stderr(mut reader: impl BufRead, sink: &Mutex<Stderr>) {
    loop {
        let mut line = Vec::new();
        let read = (&mut reader)
            .take(STDERR_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line);
        if !matches!(read, Ok(n) if n > 0) {
            return;
        }
        let complete = line.ends_with(b"\n");
        if !complete {
            line.truncate(STDERR_LINE_BYTES);
            discard_line(&mut reader);
        }
        let mut text = String::from_utf8_lossy(&line).trim_end().to_owned();
        if !complete {
            text.push_str(" ...");
        }
        let Ok(mut sink) = sink.lock() else { return };
        sink.lines.push_back(text);
        if sink.lines.len() > STDERR_LINES {
            sink.lines.pop_front();
            sink.dropped += 1;
        }
    }
}

fn discard_line(reader: &mut impl BufRead) {
    let mut scrap = Vec::new();
    loop {
        scrap.clear();
        match reader.take(8192).read_until(b'\n', &mut scrap) {
            Ok(n) if n > 0 && !scrap.ends_with(b"\n") => {}
            _ => return,
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let Ok(Ok(session)) = self.session.get_mut() else {
            return;
        };
        let shutdown = Message::request(session.next_id, lighthouse_protocol::SHUTDOWN, ());
        for message in shutdown
            .into_iter()
            .chain([Message::notification(lighthouse_protocol::EXIT)])
        {
            let mut frame = Vec::new();
            if write_message(&mut frame, &message).is_ok() {
                let _ = session.writer.send(frame);
            }
        }
        let deadline = Instant::now() + EXIT_GRACE;
        while matches!(session.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            thread::sleep(POLL);
        }
        reap(&mut session.child);
    }
}

/// Kills a plugin that is still running together with everything it started,
/// then collects it. A plugin that already exited is only collected: its
/// process group id may have been reused.
fn reap(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: killpg only sends a signal to the group made at spawn.
            unsafe {
                libc::killpg(pid, libc::SIGKILL);
            }
        }
        let _ = child.kill();
    }
    let _ = child.wait();
}
