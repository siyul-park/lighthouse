//! Where the server of a project listens, and the claim that there is one.

use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
};

use lighthouse_model::var_os;

use lighthouse_protocol::hash::Hasher;

use crate::serve::peer;

/// The directory of the sockets below `$XDG_RUNTIME_DIR`.
const DIR: &str = "lighthouse";

/// How much of the digest of the root names the socket: the path of the
/// project would not fit in the 104 bytes a socket path may have.
const NAME: usize = 16;

/// A server's claim on a socket: the lock that makes it the only one, and the
/// listening socket.
pub struct Socket {
    path: PathBuf,
    listener: UnixListener,
    /// Holds the lock for as long as the server lives.
    _lock: File,
}

impl Socket {
    /// Claims the socket at `path`. `None` when another server holds it. A
    /// socket file nobody listens on is replaced.
    pub fn claim(path: &Path) -> io::Result<Option<Self>> {
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("no directory"))?;
        private(dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))?;
        if lock.try_lock().is_err() {
            return Ok(None);
        }
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let listener = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(Some(Self {
            path: path.to_owned(),
            listener,
            _lock: lock,
        }))
    }

    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Takes the socket file away, so that no client finds a server that is
    /// leaving.
    pub fn release(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The socket of the server of the project at `root`: below
/// `$XDG_RUNTIME_DIR/lighthouse`, else `$TMPDIR/lighthouse-<uid>`, named by
/// the root.
pub fn socket_for(root: &Path) -> PathBuf {
    socket_in(&directory(), root)
}

/// The socket of the server of the project at `root` in `directory`.
pub fn socket_in(directory: &Path, root: &Path) -> PathBuf {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let mut hash = Hasher::new();
    hash.update(root.as_os_str().as_encoded_bytes());
    let name = format!("{}.sock", &hash.hex()[..NAME]);
    directory.join(name)
}

fn directory() -> PathBuf {
    match var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
    {
        Some(runtime) => runtime.join(DIR),
        None => {
            let temp = var_os("TMPDIR")
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute())
                .unwrap_or_else(|| PathBuf::from("/tmp"));
            temp.join(format!("{DIR}-{}", peer::own()))
        }
    }
}

/// Makes `dir` a directory only this user can enter, or fails if it is
/// somebody else's or open to others.
fn private(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = fs::metadata(dir)?;
    if meta.uid() != peer::own() || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "{} is not private to this user",
            dir.display()
        )));
    }
    Ok(())
}
