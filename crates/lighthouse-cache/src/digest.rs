//! The hashes of a merged project that keys are built from, computed once per
//! run from what the merge already holds, the files side by side.
//!
//! Per file there are three. The `slice` covers what the file itself contributes:
//! its record, symbols, function summaries, tests, comments, outgoing edges,
//! the modules and owners of its symbols. The `neighbors` digest covers what
//! its symbols are tied to: each edge that starts or ends at one, the owner of
//! one and its members, each with a description of the symbol at the other end
//! (identity, kind, visibility, owner, file, test and generation flags, the
//! number of its callers, callees, references and members) but not where in
//! its file it lies, so that an edit that only moves a neighbor changes
//! nothing. The `neighbors_at` digest is the same with the place of each
//! neighbor, for the rules that read it. The whole-project digest is made of every slice and what belongs
//! to no file.

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use lighthouse_model::{Document, Project, hash::Hasher};
use rayon::prelude::*;
use serde::Serialize;

use crate::{Error, neighbors::Ties};

/// A SHA-256.
pub type Digest = [u8; 32];

/// What the keys of one file's results are built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileDigest {
    pub slice: Digest,
    pub neighbors: Digest,
    pub neighbors_at: Digest,
}

/// The digests of a project: one per file and one for all of it.
#[derive(Debug, Default)]
pub struct Digests {
    project: Digest,
    files: HashMap<PathBuf, FileDigest>,
}

impl Digests {
    /// Hashes `project`.
    pub fn of(project: &Project) -> Result<Self, Error> {
        let ties = Ties::of(project);
        let mut paths: BTreeSet<&Path> = project.files.iter().map(|f| f.path.as_path()).collect();
        paths.extend(
            project
                .symbols
                .iter()
                .map(|s| s.file.as_path())
                .chain(project.comments.iter().map(|c| c.file.as_path())),
        );
        let paths: Vec<&Path> = paths.into_iter().collect();
        let digests = paths
            .par_iter()
            .map(|path| ties.file(path))
            .collect::<Result<Vec<FileDigest>, Error>>()?;
        let mut whole = Hasher::new();
        for (path, digest) in paths.iter().zip(&digests) {
            whole.update(path.as_os_str().as_encoded_bytes());
            whole.update(digest.slice);
        }
        for module in &project.modules {
            feed(&mut whole, module)?;
        }
        ties.orphans(&mut whole)?;
        Ok(Self {
            project: whole.finish_bytes(),
            files: paths.into_iter().map(Path::to_owned).zip(digests).collect(),
        })
    }

    /// The digest of everything the project holds.
    pub fn project(&self) -> &Digest {
        &self.project
    }

    /// The digests of a file; `None` for a path the project does not know.
    pub fn file(&self, path: &Path) -> Option<&FileDigest> {
        self.files.get(path)
    }
}

/// The digest of the documents of a project, which no file digest covers.
pub fn documents(documents: &[Document]) -> Result<Digest, Error> {
    let mut hasher = Hasher::new();
    for document in documents {
        feed(
            &mut hasher,
            &(
                &document.file,
                document.at,
                &document.kind,
                &document.name,
                &document.uid,
                &document.labels,
            ),
        )?;
    }
    Ok(hasher.finish_bytes())
}

/// The hash of the JSON of `value`.
pub(crate) fn digest_of<T: Serialize + ?Sized>(value: &T) -> Result<Digest, Error> {
    let mut hasher = Hasher::new();
    feed(&mut hasher, value)?;
    Ok(hasher.finish_bytes())
}

/// Feeds the JSON of `value` to `hasher`.
pub(crate) fn feed<T: Serialize + ?Sized>(hasher: &mut Hasher, value: &T) -> Result<(), Error> {
    thread_local! {
        static JSON: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    JSON.with_borrow_mut(|json| {
        json.clear();
        serde_json::to_writer(&mut *json, value)?;
        hasher.update(&*json);
        hasher.update([0]);
        Ok(())
    })
}
