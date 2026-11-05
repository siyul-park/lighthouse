//! The text of a file, read when something first needs it.

use std::{fs, ops::Deref, path::PathBuf, sync::OnceLock};

/// The text of an input: held when an overlay gave it, else read from the
/// file on first use. Detecting files hashes and checks them without it, and
/// what no stage asks for is not read. A file that cannot be read by then
/// (it vanished since detect) is empty text.
pub(crate) struct Text {
    cell: OnceLock<String>,
    path: Option<PathBuf>,
}

impl Text {
    pub(crate) fn held(text: String) -> Self {
        Self {
            cell: OnceLock::from(text),
            path: None,
        }
    }

    pub(crate) fn at(path: PathBuf) -> Self {
        Self {
            cell: OnceLock::new(),
            path: Some(path),
        }
    }
}

impl Deref for Text {
    type Target = String;

    fn deref(&self) -> &String {
        self.cell.get_or_init(|| {
            self.path
                .as_ref()
                .and_then(|path| fs::read_to_string(path).ok())
                .unwrap_or_default()
        })
    }
}
