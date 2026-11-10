//! One warm process per project: `lighthouse serve` listens on a Unix socket
//! and runs the commands a client forwards against a project it keeps loaded.
//! The client runs a command here when a server answers and itself when none
//! does, so a command is never lost for lack of a server.
//!
//! Messages are JSON-RPC 2.0 with `Content-Length` framing, as the plugins
//! speak them: `initialize` tells the versions, `lighthouse/run` runs a
//! command and answers its output and exit code, `shutdown` ends the server.

mod client;
mod peer;
mod server;
mod socket;
mod wire;

pub use client::{Connection, connect, ensure, status, stop};
pub use peer::permits;
pub use server::{Ended, Runner, serve};
pub use socket::{Socket, socket_for, socket_in};
pub use wire::{Hello, Output, Refusal, Run, identity};
