//! The two ends of the way into a namespace that has no network. The host
//! listens on a Unix socket that is bound into the namespace and passes what
//! comes to the proxy ([`Uplink`]); inside, [`bridge`] listens on a loopback
//! port, passes what comes to the socket, and runs the program. This is what
//! `srt` does with socat, in the `lighthouse` binary itself.

use std::{io, path::Path};

use super::relay::Server;

/// The host end: a Unix socket whose connections go to the proxy's port.
pub(super) struct Uplink {
    _server: Server,
}

impl Uplink {
    /// Listens on `socket` and passes every connection to the proxy at `port`.
    #[cfg(unix)]
    pub(super) fn start(socket: &Path, port: u16) -> io::Result<Self> {
        use std::{
            net::TcpStream,
            os::unix::net::{UnixListener, UnixStream},
        };

        use super::relay::relay;

        let listener = UnixListener::bind(socket)?;
        listener.set_nonblocking(true)?;
        let server = Server::spawn(
            move || {
                let (connection, _) = listener.accept()?;
                connection.set_nonblocking(false)?;
                Ok(connection)
            },
            move |inside: UnixStream| {
                if let Ok(proxy) = TcpStream::connect(("127.0.0.1", port)) {
                    relay(inside, proxy);
                }
            },
        );
        Ok(Self { _server: server })
    }

    /// There are no Unix sockets here: no namespace to reach in from.
    #[cfg(not(unix))]
    pub fn start(_socket: &Path, _port: u16) -> io::Result<Self> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

/// Listens on `127.0.0.1:port`, passes each connection to the Unix socket
/// `socket`, and runs `command` until it exits, returning its exit code. This
/// is what the `lighthouse internal bridge` command does inside a namespace
/// without a network.
#[cfg(unix)]
pub fn bridge(socket: &Path, port: u16, command: &[String]) -> io::Result<i32> {
    use std::net::TcpListener;

    if command.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no program"));
    }
    bridge_on(TcpListener::bind(("127.0.0.1", port))?, socket, command)
}

/// Like [`bridge`], on a listener that is already bound: a caller that wants
/// the port the system gives (port 0) binds it itself and reads it, so that
/// no other process can take the port between choosing and listening.
#[cfg(unix)]
pub fn bridge_on(
    listener: std::net::TcpListener,
    socket: &Path,
    command: &[String],
) -> io::Result<i32> {
    use std::{
        net::TcpStream,
        os::unix::{net::UnixStream, process::ExitStatusExt},
        process::Command,
    };

    use super::relay::relay;

    let (program, args) = command
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no program"))?;
    listener.set_nonblocking(true)?;
    let socket = socket.to_owned();
    let _server = Server::spawn(
        move || {
            let (connection, _) = listener.accept()?;
            connection.set_nonblocking(false)?;
            Ok(connection)
        },
        move |program: TcpStream| {
            if let Ok(host) = UnixStream::connect(&socket) {
                relay(program, host);
            }
        },
    );
    let status = Command::new(program).args(args).status()?;
    Ok(status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1))
}

/// There are no Unix sockets here: no namespace to bridge into.
#[cfg(not(unix))]
pub fn bridge(_socket: &Path, _port: u16, _command: &[String]) -> io::Result<i32> {
    Err(io::ErrorKind::Unsupported.into())
}

/// There are no Unix sockets here: no namespace to bridge into.
#[cfg(not(unix))]
pub fn bridge_on(
    _listener: std::net::TcpListener,
    _socket: &Path,
    _command: &[String],
) -> io::Result<i32> {
    Err(io::ErrorKind::Unsupported.into())
}
