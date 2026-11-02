use std::{
    fs,
    path::{Path, PathBuf},
};

use lighthouse_model::{Document, Error, Format};

use crate::engine::Overlays;

/// Largest file looked at: configuration documents are small, and a data file
/// that happens to be YAML or JSON is not one.
const MAX_BYTES: u64 = 1 << 20;

/// The documents found, and what could not be settled about them.
#[derive(Default)]
pub(crate) struct Documents {
    pub documents: Vec<Document>,
    pub notices: Vec<String>,
}

impl Documents {
    /// The Lighthouse documents of the project (decisions, packs, projects, ...)
    /// found in those of its `files` that are YAML, TOML or JSON, with the line
    /// each name is written on; a file that mentions Lighthouse but cannot be
    /// read or parsed is reported, not skipped silently. `overlays` stand in
    /// for the files of the same path.
    pub(crate) fn scan(root: &Path, files: &[PathBuf], overlays: &Overlays) -> Self {
        let mut found = Self::default();
        for relative in files {
            let path = root.join(relative);
            let Some(format) = Format::of_path(&path) else {
                continue;
            };
            let text = match overlays.get(relative) {
                Some(text) => text.clone(),
                None if fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES) => {
                    match fs::read_to_string(&path) {
                        Ok(text) => text,
                        Err(e) => {
                            found.notices.push(format!(
                                "{}: not read as a document: {e}",
                                relative.display()
                            ));
                            continue;
                        }
                    }
                }
                None => continue,
            };
            if text.contains("lighthouse/") {
                found.read(relative, format, &text);
            }
        }
        found
    }

    /// Adds the Lighthouse documents of one file.
    fn read(&mut self, file: &Path, format: Format, text: &str) {
        let label = file.display().to_string();
        let (documents, errors) = Document::list(format, file, text);
        self.notices.extend(errors.iter().map(|e| match e {
            Error::Unplaced { name, .. } => format!(
                "{label}: could not find where `{name}` is written; it is reported at the top of the file"
            ),
            e => format!("{label}: not read as a document: {e}"),
        }));
        self.documents.extend(documents);
    }
}
