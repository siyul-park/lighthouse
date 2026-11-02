//! Copying a file the cheapest way the file system has: a clone that shares
//! the blocks (APFS `clonefile`, a Linux reflink), else a copy.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
};

use crate::is_project_secret;

/// Makes `to`, which must not exist, a copy of `from`. The clone shares the
/// blocks until either file is written, so a copy of a project costs its
/// directory entries and not its bytes where the file system can clone.
pub fn clone_file(from: &Path, to: &Path) -> io::Result<()> {
    if cloned(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).map(drop)
}

/// Copies the `files` of the project at `root` (project-relative paths, as the
/// project lists them: what its ignore rules keep, not what the user's machine
/// does, so the copy is the same on every machine), except its secrets (the
/// copy is writable and its programs may read it). Writes `texts` over the
/// files. The paths of the files placed.
pub fn copy_project(
    root: &Path,
    files: &[PathBuf],
    into: &Path,
    texts: &BTreeMap<PathBuf, String>,
) -> io::Result<BTreeSet<PathBuf>> {
    let mut placed = BTreeSet::new();
    for relative in files {
        if is_project_secret(relative) {
            continue;
        }
        place(&into.join(relative), |target| {
            clone_file(&root.join(relative), target)
        })?;
        placed.insert(relative.clone());
    }
    for (path, text) in texts {
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(io::Error::other(format!(
                "`{}` is not a relative, normalized path",
                path.display()
            )));
        }
        place(&into.join(path), |target| fs::write(target, text))?;
        placed.insert(path.clone());
    }
    Ok(placed)
}

#[cfg(target_os = "linux")]
fn cloned(from: &Path, to: &Path) -> io::Result<()> {
    use std::{fs::OpenOptions, os::fd::AsRawFd};
    /// `_IOW(0x94, 9, int)`: the ioctl that makes a file share the blocks of another.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let source = fs::File::open(from)?;
    let target = OpenOptions::new().write(true).create_new(true).open(to)?;
    // SAFETY: both descriptors are open for the call.
    let status = unsafe { libc::ioctl(target.as_raw_fd(), FICLONE, source.as_raw_fd()) };
    if status == 0 {
        fs::set_permissions(to, source.metadata()?.permissions())
    } else {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(to);
        Err(error)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn cloned(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(target_os = "macos")]
fn cloned(from: &Path, to: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let path = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(io::Error::other);
    let (from, to) = (path(from)?, path(to)?);
    // SAFETY: both are NUL-terminated paths that live through the call.
    let status = unsafe { libc::clonefile(from.as_ptr(), to.as_ptr(), 0) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Creates the directory of `target` and fills the file by `write`.
fn place(target: &Path, write: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    write(target)
}
