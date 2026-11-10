//! What client and server say to each other.

use std::{collections::BTreeMap, env, path::PathBuf};

use lighthouse_process::{INHERITED, PERSONAL};
use serde::{Deserialize, Serialize};

/// The version of the exchange, which both sides must speak.
pub const PROTOCOL: &str = "1";

/// Request: the versions of the server; the first message of a connection.
pub const INITIALIZE: &str = "initialize";

/// Request: run a command.
pub const RUN: &str = "lighthouse/run";

/// Request: end the server.
pub const SHUTDOWN: &str = "shutdown";

/// What a side is: the build of the program and the protocol it speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: String,
    pub protocol: String,
}

/// A command to run: what the process of a command line would have been
/// started with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// The command line, program name first.
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// The variables of the environment that decide a run, as the client has
    /// them: see [`Run::decides`].
    pub env: BTreeMap<String, String>,
    /// What the command reads from its standard input, for the hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin: Option<String>,
}

impl Run {
    /// Whether a variable is one of those that decide a run: Lighthouse's own
    /// (`LIGHTHOUSE_*`, `XDG_*`, `NO_COLOR`) and those a program it starts
    /// inherits.
    pub fn decides(name: &str) -> bool {
        name.starts_with("LIGHTHOUSE_")
            || name.starts_with("XDG_")
            || name == "NO_COLOR"
            || INHERITED.contains(&name)
            || PERSONAL.contains(&name)
    }

    /// The variables of this process that decide a run.
    pub fn decisive() -> BTreeMap<String, String> {
        env::vars_os()
            .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
            .filter(|(name, _)| Self::decides(name))
            .collect()
    }

    /// A variable that decides the run and is not what `own` has, set or
    /// unset alike: the server runs a command only in the environment the
    /// client has.
    pub fn differs(&self, own: &BTreeMap<String, String>) -> Option<String> {
        self.env
            .keys()
            .chain(own.keys())
            .find(|name| self.env.get(*name) != own.get(*name))
            .cloned()
    }
}

/// What a command printed, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit: u8,
}

/// Why the server does not run a command: it is not one the server runs, or
/// it cannot be read as one. The client runs it itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

/// This program as a peer: its build, so that a server of another build is
/// never asked.
pub fn identity() -> Hello {
    Hello {
        version: lighthouse_session::cache::build_identity(),
        protocol: PROTOCOL.to_owned(),
    }
}
