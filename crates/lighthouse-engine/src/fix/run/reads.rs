//! What a finding was made of, as the graph recorded it: the files that the
//! rule which found it read, and of those the ones that moved between two
//! checks. A rule that reads one file records that file and no other; one that
//! reads the symbols of the project records them all.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use lighthouse_graph::{Reads, Revision, keys, reads};
use lighthouse_model::ucm::{
    COLLISIONS, DECLARATIONS, DUPLICATED, HOST, INCOMING, PATHS, RESOLVED, SLICE,
};
use lighthouse_plugin::{FINDINGS, JUDGED};
use lighthouse_spec::{CONTENT, OVERLAY};

use crate::Engine;

/// The kinds of the graph whose key is the path of a file.
const OF_A_FILE: [&str; 7] = [
    HOST.name(),
    OVERLAY.name(),
    CONTENT.name(),
    SLICE.name(),
    DECLARATIONS.name(),
    RESOLVED.name(),
    INCOMING.name(),
];

/// The files the node of the rule `rule` for `file` read, or, with `moved` as
/// the revisions of two checks, those of them that moved between the two. `None`
/// when the rule is no node of the graph, or when what it read is not told by
/// files: the project's list of files, or nothing a file moved.
pub(super) fn files(
    engine: &Engine,
    (rule, file): (&str, &Path),
    moved: Option<(Revision, Revision)>,
) -> Option<BTreeSet<PathBuf>> {
    let guard = engine.lock_graph();
    let graph = guard.as_ref()?;
    let name = file.to_string_lossy().into_owned();
    // Which modules and which ids several files declare is read by every
    // slice and symbol, from the declarations of the whole project: a file that
    // is a gap declares nothing, so it is none of those.
    let except: &[&str] = if moved.is_some() {
        &[]
    } else {
        &[COLLISIONS.name(), DUPLICATED.name()]
    };
    let reads = keys(graph, &JUDGED)
        .into_iter()
        .filter(|(id, _)| id == rule)
        .find_map(|(id, variant)| {
            reads(
                graph,
                &FINDINGS,
                &(id, name.clone(), variant),
                (moved, except),
            )
            .ok()?
        })?;
    told_by_files(&reads)
}

fn told_by_files(reads: &Reads) -> Option<BTreeSet<PathBuf>> {
    if !reads.keys::<()>(PATHS.name()).is_empty() {
        return None;
    }
    let files: BTreeSet<PathBuf> = OF_A_FILE
        .iter()
        .flat_map(|kind| reads.keys::<String>(kind))
        .map(PathBuf::from)
        .collect();
    (!files.is_empty()).then_some(files)
}
