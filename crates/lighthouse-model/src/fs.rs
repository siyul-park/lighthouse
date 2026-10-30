use std::{
    ffi::OsString,
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{Error, vars};

/// The directory below a project's root that holds what Lighthouse keeps for
/// it: the cache, the store, its own decisions, models and datasets.
pub const STATE_DIR: &str = ".lighthouse";

/// The directory below each XDG base that is Lighthouse's.
const APP: &str = "lighthouse";

/// A kind of user-level file. Where each lives follows the XDG Base Directory
/// Specification: a variable and a default below `$HOME`; a relative value of
/// the variable is invalid and ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// What the user decided: `trust.toml`.
    Config,
    /// What the user installed: plugins.
    Data,
    /// What can be fetched or built again: downloaded model weights.
    Cache,
}

impl Base {
    fn variable(self) -> &'static str {
        match self {
            Self::Config => "XDG_CONFIG_HOME",
            Self::Data => "XDG_DATA_HOME",
            Self::Cache => "XDG_CACHE_HOME",
        }
    }

    fn fallback(self) -> &'static str {
        match self {
            Self::Config => ".config",
            Self::Data => ".local/share",
            Self::Cache => ".cache",
        }
    }
}

/// The first of `names` that is a file in `dir`: how a project's config and a
/// plugin's manifest are found, in the order the names are tried.
pub fn file_in(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    names.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// The text of the file at `path`; `None` when there is no such file.
pub fn read_optional(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        }),
    }
}

/// Writes `text` beside `path`, flushes it to disk, then renames it over
/// `path`, creating the directories above it. Readers see either complete
/// version; a failed write leaves no partial file. Refuses a symlink rather
/// than following or replacing it.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), Error> {
    write_atomic_guarded(path, text, &|| Ok(()))
}

/// Like [`write_atomic`], but runs `guard` after the temporary file is
/// complete and before replacement. A guard error abandons the write.
pub fn write_atomic_guarded(
    path: &Path,
    text: &str,
    guard: &dyn Fn() -> Result<(), String>,
) -> Result<(), Error> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let io = |error: std::io::Error| Error::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    };
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(Error::Io {
            path: path.display().to_string(),
            message: "is a symlink; edit its target by hand".to_owned(),
        });
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        guard().map_err(std::io::Error::other)?;
        fs::rename(&tmp, path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io(error));
    }
    Ok(())
}

/// `$XDG_<KIND>_HOME/lighthouse`, else `$HOME/<default>/lighthouse`; `None`
/// when neither is usable.
pub fn xdg_dir(base: Base) -> Option<PathBuf> {
    xdg_dir_in(base, vars::var_os)
}

/// [`xdg_dir`] reading the variables through `var`.
pub fn xdg_dir_in(base: Base, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| var(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let root = absolute(base.variable())
        .or_else(|| absolute("HOME").map(|home| home.join(base.fallback())))?;
    Some(root.join(APP))
}

/// The plugin directory of the user: `<data>/plugins`.
pub fn xdg_plugins() -> Option<PathBuf> {
    xdg_dir(Base::Data).map(|data| data.join("plugins"))
}
