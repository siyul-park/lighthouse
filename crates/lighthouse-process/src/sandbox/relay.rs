//! Moving bytes between two connections, and serving connections on a thread.

use std::{
    io::{self, Read, Write},
    net::{Shutdown, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// How often a server looks for a connection, and for being told to stop.
const POLL: Duration = Duration::from_millis(10);

/// A connection that can be read and written from two threads at once.
pub(super) trait Stream: Send + Sync + Sized + 'static
where
    for<'a> &'a Self: Read + Write,
{
    fn shutdown(&self, how: Shutdown) -> io::Result<()>;
}

impl Stream for TcpStream {
    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        TcpStream::shutdown(self, how)
    }
}

#[cfg(unix)]
impl Stream for std::os::unix::net::UnixStream {
    fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        std::os::unix::net::UnixStream::shutdown(self, how)
    }
}

/// Hands each connection a listener accepts to a thread of its own, until it
/// is dropped.
pub(super) struct Server {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    /// `accept` takes one connection without blocking; `serve` handles it.
    pub(super) fn spawn<S: Send + 'static>(
        accept: impl Fn() -> io::Result<S> + Send + 'static,
        serve: impl Fn(S) + Send + Sync + 'static,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let serve = Arc::new(serve);
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match accept() {
                    Ok(connection) => {
                        let serve = Arc::clone(&serve);
                        thread::spawn(move || serve(connection));
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
                    Err(_) => break,
                }
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Copies each of `a` and `b` to the other until both have closed.
pub(super) fn relay<A: Stream, B: Stream>(a: A, b: B)
where
    for<'a> &'a A: Read + Write,
    for<'b> &'b B: Read + Write,
{
    let (a, b) = (Arc::new(a), Arc::new(b));
    let (from, to) = (Arc::clone(&a), Arc::clone(&b));
    let forward = thread::spawn(move || {
        let _ = io::copy(&mut &*from, &mut &*to);
        let _ = to.shutdown(Shutdown::Write);
    });
    let _ = io::copy(&mut &*b, &mut &*a);
    let _ = a.shutdown(Shutdown::Write);
    let _ = forward.join();
}
