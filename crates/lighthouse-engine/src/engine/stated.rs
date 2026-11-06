//! What a run reads of the project besides the queries of the graph: the
//! files as the model states them, which of them declare test code, and the
//! project's own documents. The planning of the files side by side asks
//! these of many threads, which a view of the graph does not serve.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use lighthouse_model::{
    Document,
    ucm::{File, Model},
};

#[derive(Debug, Default)]
pub(super) struct Stated {
    files: Vec<File>,
    tested: BTreeSet<PathBuf>,
    documents: Vec<Document>,
}

impl Stated {
    /// What `model` states of the files, sorted by path.
    pub(super) fn of(model: &dyn Model) -> Self {
        let files: Vec<File> = model.files().cloned().collect();
        let tested = files
            .iter()
            .filter(|file| {
                file.test
                    || model
                        .symbols_in(&file.path)
                        .any(|symbol| model.in_test(&symbol.id))
            })
            .map(|file| file.path.clone())
            .collect();
        Self {
            files,
            tested,
            documents: Vec::new(),
        }
    }

    /// The same with the project's own documents.
    pub(super) fn with_documents(self, documents: Vec<Document>) -> Self {
        Self { documents, ..self }
    }

    /// The file at this exact project-relative path.
    pub(super) fn file(&self, path: &Path) -> Option<&File> {
        let at = self
            .files
            .binary_search_by(|file| file.path.as_path().cmp(path))
            .ok()?;
        self.files.get(at)
    }

    /// Whether the file declares test code, or is test code.
    pub(super) fn has_test_code(&self, path: &Path) -> bool {
        self.tested.contains(path)
    }

    /// Every file, sorted by path.
    pub(super) fn files(&self) -> &[File] {
        &self.files
    }

    /// The documents of the project that are not code, in file order.
    pub(super) fn documents(&self) -> &[Document] {
        &self.documents
    }
}
