use std::{fs, path::Path};

use ignore::WalkBuilder;
use lighthouse_model::{Document, Error, Format};

use crate::engine::{IGNORE_FILE, Overlays};

/// Largest file looked at: configuration documents are small, and a data file
/// that happens to be YAML or JSON is not one.
const MAX_BYTES: u64 = 1 << 20;

/// Directories no command looks for documents in.
pub const SKIPPED_DIRS: [&str; 4] = [".git", "target", "node_modules", "testdata"];

/// The documents found, and what could not be settled about them.
#[derive(Default)]
pub(crate) struct Documents {
    pub documents: Vec<Document>,
    pub notices: Vec<String>,
}

impl Documents {
    /// The Lighthouse documents of the project (decisions, packs, projects, ...)
    /// found in its YAML, TOML and JSON files, with the line each name is written
    /// on. Files the project ignores and the directories in [`SKIPPED_DIRS`] are
    /// left out; a file that mentions Lighthouse but cannot be read or parsed is
    /// reported, not skipped silently. `overlays` stand in for the files of the
    /// same path.
    pub(crate) fn scan(root: &Path, overlays: &Overlays) -> Self {
        let walk = WalkBuilder::new(root)
            .require_git(false)
            .hidden(false)
            .add_custom_ignore_filename(IGNORE_FILE)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !SKIPPED_DIRS.contains(&name))
            })
            .build();
        let mut found = Self::default();
        for entry in walk.flatten() {
            let path = entry.path();
            let Some(format) = Format::of_path(path) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let text = match overlays.get(relative) {
                Some(text) => text.clone(),
                None if entry
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES) =>
                {
                    match fs::read_to_string(path) {
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
