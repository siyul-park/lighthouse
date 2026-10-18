//! What a backend is told: the paths of a profile as they are on disk.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use super::Profile;

/// A path a sandboxed program may not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    pub path: PathBuf,
    /// A directory is hidden as a whole; a file is hidden on its own.
    pub dir: bool,
}

/// How a program in a namespace without a network reaches the proxy: the
/// executable that forwards a local port to the proxy's socket, and the socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub exe: PathBuf,
    pub socket: PathBuf,
}

/// A [`Profile`] resolved against the disk: canonical paths, only those that
/// exist, and the route to the proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Where the program may write.
    pub writes: Vec<PathBuf>,
    /// What the program may not read.
    pub deny: Vec<Denied>,
    /// The loopback port the proxy is reached on; none when the program may
    /// reach nothing.
    pub port: Option<u16>,
    /// Set where the program lives in a namespace of its own.
    pub bridge: Option<Bridge>,
    /// Directories the program sees empty: where other programs of the user
    /// leave their sockets (`/run`, and `/tmp` when the run needs nothing in it).
    pub mask: Vec<PathBuf>,
}

impl Plan {
    /// The plan of `profile` for a run in `dir` whose temporary directory is
    /// `tmp`. A path to write that cannot be made is left out: the program
    /// gets less access, never more.
    pub(super) fn of(
        profile: &Profile,
        tmp: &Path,
        dir: &Path,
        port: Option<u16>,
    ) -> Result<Self, String> {
        let writes: Vec<PathBuf> = profile.writes.iter().filter_map(|w| prepared(w)).collect();
        let needs_tmp = [dir.canonicalize().unwrap_or_else(|_| dir.to_owned())]
            .iter()
            .chain(&writes)
            .any(|path| path.starts_with("/tmp"));
        let mask = ["/run", "/var/run", "/tmp"]
            .into_iter()
            .filter(|place| *place != "/tmp" || !needs_tmp)
            .map(PathBuf::from)
            .filter(|place| place.is_dir() && !place.is_symlink())
            .collect();
        let agent = env::var_os("SSH_AUTH_SOCK")
            .map(PathBuf::from)
            .and_then(|socket| socket.parent().map(Path::to_owned));
        let deny = profile
            .deny
            .iter()
            .chain(&agent)
            .filter_map(|path| hidden(path))
            .collect();
        let writes = prepared(tmp).into_iter().chain(writes).collect();
        let bridge = if cfg!(target_os = "linux") && port.is_some() {
            let exe = env::current_exe().map_err(|e| format!("no bridge executable: {e}"))?;
            Some(Bridge {
                exe,
                socket: tmp.join("proxy.sock"),
            })
        } else {
            None
        };
        Ok(Self {
            writes,
            deny,
            port,
            bridge,
            mask,
        })
    }
}

/// The canonical `path`, made when it is not there yet.
fn prepared(path: &Path) -> Option<PathBuf> {
    fs::create_dir_all(path).ok()?;
    path.canonicalize().ok()
}

/// `path` as a denial, when something is there to hide.
fn hidden(path: &Path) -> Option<Denied> {
    let path = path.canonicalize().ok()?;
    Some(Denied {
        dir: path.is_dir(),
        path,
    })
}
