use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::Error;

/// Writes `text` beside `path`, flushes it to disk, then renames it over
/// `path`. Readers see either complete version; a failed write leaves no
/// partial file. Refuses a symlink rather than following or replacing it.
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
        return Err(Error::Invalid {
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
