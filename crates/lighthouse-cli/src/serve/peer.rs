//! Who may talk to the server: the user it runs as, and nobody else.

use std::{io, os::unix::net::UnixStream};

use nix::unistd::getuid;

/// Whether a peer of user `peer` may use a server that runs as `own`.
pub fn permits(peer: u32, own: u32) -> bool {
    peer == own
}

/// The user the process on the other end of `stream` runs as.
#[cfg(target_os = "linux")]
pub(crate) fn uid(stream: &UnixStream) -> io::Result<u32> {
    use nix::sys::socket::{getsockopt, sockopt::PeerCredentials};
    getsockopt(stream, PeerCredentials)
        .map(|credentials| credentials.uid())
        .map_err(io::Error::from)
}

/// The user the process on the other end of `stream` runs as.
#[cfg(not(target_os = "linux"))]
pub(crate) fn uid(stream: &UnixStream) -> io::Result<u32> {
    nix::unistd::getpeereid(stream)
        .map(|(uid, _)| uid.as_raw())
        .map_err(io::Error::from)
}

/// The user this process runs as.
pub(crate) fn own() -> u32 {
    getuid().as_raw()
}
